//! Sway (and other compositors with `zwp_virtual_keyboard_v1`): a virtual
//! keyboard on the seat, with a small keymap of its own that has the
//! shortcut's modifiers and key at their US keyboard positions, so the
//! shortcut means the same whatever layout the real keyboard has. The keys
//! go to the window with the keyboard focus, which the caller has checked.

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Instant;

use stickpile_core::config::Shortcut;
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1, zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};

use super::{evdev_code, keysym_name, Modifier};

/// `wl_keyboard.keymap_format.xkb_v1`
const KEYMAP_FORMAT_XKB_V1: u32 = 1;
const PRESSED: u32 = 1;
const RELEASED: u32 = 0;

/// The keymap uploaded for `s`: its key and the four modifiers, each at its
/// US keyboard keycode (xkb keycodes are evdev + 8). Self-contained, so it
/// does not depend on the compositor's xkb include path.
pub fn keymap(s: &Shortcut) -> String {
    let key = keysym_name(s.key);
    // Letters get their capital on the Shift level, as on a real keyboard.
    let symbols = if key.len() == 1 && key.as_bytes()[0].is_ascii_lowercase() {
        format!("{key}, {}", key.to_ascii_uppercase())
    } else {
        key
    };
    let code = |evdev: u32| evdev + 8;
    format!(
        "xkb_keymap {{
xkb_keycodes \"stickpile\" {{
    minimum = 8;
    maximum = 255;
    <LCTL> = {ctrl};
    <LFSH> = {shift};
    <LALT> = {alt};
    <LWIN> = {sup};
    <PSTE> = {key};
}};
xkb_types \"stickpile\" {{
    type \"ONE_LEVEL\" {{
        modifiers = none;
        level_name[Level1] = \"Any\";
    }};
    type \"TWO_LEVEL\" {{
        modifiers = Shift;
        map[Shift] = Level2;
        level_name[Level1] = \"Base\";
        level_name[Level2] = \"Shift\";
    }};
    type \"ALPHABETIC\" {{
        modifiers = Shift + Lock;
        map[Shift] = Level2;
        map[Lock] = Level2;
        level_name[Level1] = \"Base\";
        level_name[Level2] = \"Caps\";
    }};
}};
xkb_compatibility \"stickpile\" {{
    interpret Any + AnyOf(all) {{
        action = SetMods(modifiers = modMapMods, clearLocks);
    }};
}};
xkb_symbols \"stickpile\" {{
    key <LCTL> {{ [ Control_L ] }};
    key <LFSH> {{ [ Shift_L ] }};
    key <LALT> {{ [ Alt_L ] }};
    key <LWIN> {{ [ Super_L ] }};
    key <PSTE> {{ [ {symbols} ] }};
    modifier_map Control {{ <LCTL> }};
    modifier_map Shift {{ <LFSH> }};
    modifier_map Mod1 {{ <LALT> }};
    modifier_map Mod4 {{ <LWIN> }};
}};
}};
",
        ctrl = code(Modifier::Ctrl.evdev()),
        shift = code(Modifier::Shift.evdev()),
        alt = code(Modifier::Alt.evdev()),
        sup = code(Modifier::Super.evdev()),
        key = code(evdev_code(s.key)),
    )
}

/// One key event of the virtual keyboard: `(evdev code, pressed)`, and the
/// modifier mask after it.
pub fn events(s: &Shortcut) -> Vec<(u32, bool, u32)> {
    let mods = Modifier::of(s);
    let mut mask = 0;
    let mut out = Vec::new();
    for m in &mods {
        mask |= m.mask();
        out.push((m.evdev(), true, mask));
    }
    out.push((evdev_code(s.key), true, mask));
    out.push((evdev_code(s.key), false, mask));
    for m in mods.iter().rev() {
        mask &= !m.mask();
        out.push((m.evdev(), false, mask));
    }
    out
}

#[derive(Default)]
struct Globals {
    seat: Option<wl_seat::WlSeat>,
    manager: Option<ZwpVirtualKeyboardManagerV1>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Globals {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_seat" if state.seat.is_none() => {
                    state.seat = Some(registry.bind(name, version.min(1), qh, ()));
                }
                "zwp_virtual_keyboard_manager_v1" => {
                    state.manager = Some(registry.bind(name, 1, qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Globals {
    fn event(_: &mut Self, _: &wl_seat::WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ZwpVirtualKeyboardManagerV1, ()> for Globals {
    fn event(
        _: &mut Self,
        _: &ZwpVirtualKeyboardManagerV1,
        _: <ZwpVirtualKeyboardManagerV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpVirtualKeyboardV1, ()> for Globals {
    fn event(
        _: &mut Self,
        _: &ZwpVirtualKeyboardV1,
        _: <ZwpVirtualKeyboardV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// Press `s` with a virtual keyboard of the compositor at `socket`.
pub fn press(socket: &Path, s: &Shortcut) -> Result<(), String> {
    let err = |what: &str, e: &dyn std::fmt::Display| format!("wayland: {what}: {e}");
    let stream = UnixStream::connect(socket).map_err(|e| err(&socket.display().to_string(), &e))?;
    let conn = Connection::from_socket(stream).map_err(|e| err("connecting", &e))?;
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());
    let mut globals = Globals::default();
    queue.roundtrip(&mut globals).map_err(|e| err("roundtrip", &e))?;
    let (Some(seat), Some(manager)) = (&globals.seat, &globals.manager) else {
        return Err("wayland: the compositor has no seat or no zwp_virtual_keyboard_manager_v1".into());
    };
    let keyboard = manager.create_virtual_keyboard(seat, &qh, ());

    let mut text = keymap(s).into_bytes();
    text.push(0);
    let mut file = tempfile::tempfile().map_err(|e| err("keymap file", &e))?;
    file.write_all(&text).map_err(|e| err("keymap file", &e))?;
    keyboard.keymap(KEYMAP_FORMAT_XKB_V1, file.as_fd(), text.len() as u32);
    // The keymap must be in place before the keys.
    queue.roundtrip(&mut globals).map_err(|e| err("roundtrip", &e))?;

    let start = Instant::now();
    for (code, pressed, mask) in events(s) {
        let time = start.elapsed().as_millis() as u32;
        keyboard.key(time, code, if pressed { PRESSED } else { RELEASED });
        keyboard.modifiers(mask, 0, 0, 0);
    }
    queue.roundtrip(&mut globals).map_err(|e| err("roundtrip", &e))?;
    keyboard.destroy();
    conn.flush().map_err(|e| err("flush", &e))?;
    Ok(())
}
