//! X11: XTest key events, which go to the window with the input focus
//! (the caller has checked it is the one to paste into). The keycodes come
//! from the server's keyboard mapping: the key whose first group has the
//! keysym, so a Latin first layout works while a Cyrillic one is active.

use stickpile_core::config::Shortcut;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xproto::{ConnectionExt as _, KEY_PRESS_EVENT, KEY_RELEASE_EVENT};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::CURRENT_TIME;

use super::{keysym, Modifier};

/// The keycode whose keysym list has `sym`: in the first column (group 1,
/// level 1) if any key has it there, else in the second (group 1, shifted),
/// else anywhere. `keysyms` is the reply of `GetKeyboardMapping` from
/// `min_keycode`, `per_keycode` keysyms per key.
pub fn keycode_for(keysyms: &[u32], per_keycode: usize, min_keycode: u8, sym: u32) -> Option<u8> {
    if per_keycode == 0 {
        return None;
    }
    let keys = || keysyms.chunks(per_keycode).enumerate();
    let find = |column: Option<usize>| {
        keys().find_map(|(i, syms)| {
            let hit = match column {
                Some(c) => syms.get(c) == Some(&sym),
                None => syms.contains(&sym),
            };
            hit.then(|| u8::try_from(min_keycode as usize + i).ok()).flatten()
        })
    };
    find(Some(0)).or_else(|| find(Some(1))).or_else(|| find(None))
}

/// Press `s` with XTest on `display`.
pub fn press(display: Option<&str>, s: &Shortcut) -> Result<(), String> {
    let err = |e: &dyn std::fmt::Display| format!("x11: {e}");
    let (conn, screen) = x11rb::connect(display).map_err(|e| err(&e))?;
    if conn
        .extension_information(x11rb::protocol::xtest::X11_EXTENSION_NAME)
        .map_err(|e| err(&e))?
        .is_none()
    {
        return Err("x11: the server has no XTEST extension".into());
    }
    let setup = conn.setup();
    let root = setup.roots[screen].root;
    let (min, max) = (setup.min_keycode, setup.max_keycode);
    let map = conn
        .get_keyboard_mapping(min, max - min + 1)
        .map_err(|e| err(&e))?
        .reply()
        .map_err(|e| err(&e))?;
    let per = map.keysyms_per_keycode as usize;
    let code =
        |sym: u32| keycode_for(&map.keysyms, per, min, sym).ok_or_else(|| format!("x11: no key has keysym {sym:#x}"));
    let mods = Modifier::of(s)
        .into_iter()
        .map(|m| code(m.keysym()))
        .collect::<Result<Vec<u8>, String>>()?;
    let key = code(keysym(s.key))?;
    let mut events: Vec<(u8, u8)> = mods.iter().map(|&c| (KEY_PRESS_EVENT, c)).collect();
    events.push((KEY_PRESS_EVENT, key));
    events.push((KEY_RELEASE_EVENT, key));
    events.extend(mods.iter().rev().map(|&c| (KEY_RELEASE_EVENT, c)));
    for (kind, detail) in events {
        conn.xtest_fake_input(kind, detail, CURRENT_TIME, root, 0, 0, 0)
            .map_err(|e| err(&e))?;
    }
    // A round trip: the server has handled the events when it answers.
    conn.get_input_focus()
        .map_err(|e| err(&e))?
        .reply()
        .map_err(|e| err(&e))?;
    Ok(())
}
