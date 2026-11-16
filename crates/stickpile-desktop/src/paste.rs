//! Pressing the paste shortcut after a pick (`[paste]`, see
//! docs/desktop.md, "Paste after a pick").
//!
//! When the popup opens it remembers the window it was opened over
//! ([`PasteTarget`], from focus detection). After a pick has been copied and
//! the popup has hidden, [`plan`] decides whether to paste, and [`run`]
//! (blocking: call it off the main thread) waits until that same window has
//! the keyboard focus again and presses the shortcut there. If another
//! window has the focus when the wait ends, nothing is pressed.
//!
//! * **Hyprland**: the `send_shortcut` dispatcher over the IPC socket, aimed
//!   at the window's address ([`hyprland`]).
//! * **Sway**: a virtual keyboard (`zwp_virtual_keyboard_v1`) with a small
//!   keymap of its own ([`wayland`], feature `wayland`); the focus is read
//!   over i3-ipc.
//! * **X11**: XTest ([`x11`], feature `x11`); the focus is
//!   `_NET_ACTIVE_WINDOW`.
//!
//! Elsewhere (GNOME or KDE on Wayland, other compositors) the popup cannot
//! tell which window has the focus, so it only copies ([`available`] is
//! false). The macOS app presses Cmd+V itself, after [`mac_plan`].

use std::fmt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use stickpile_core::config::{PasteConfig, Shortcut, ShortcutKey};
use stickpile_quick::focus::{self, FocusBackend, FocusEnv, WindowInfo};

pub mod hyprland;
#[cfg(feature = "wayland")]
pub mod wayland;
#[cfg(feature = "x11")]
pub mod x11;

/// How often the focus is read while waiting for it.
pub const POLL: Duration = Duration::from_millis(20);

/// The window the popup was opened over, and the environment of the
/// command that opened it (its compositor sockets).
#[derive(Debug, Clone, PartialEq)]
pub struct PasteTarget {
    pub window: WindowInfo,
    pub env: FocusEnv,
}

/// Why nothing is pasted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// `paste.auto` is off.
    Off,
    /// The popup is still shown (`ui.hide_on_copy = false`): the focus is
    /// not going back.
    StillShown,
    /// No window was focused when the popup opened (or it was the popup).
    NoWindow,
    /// Focus detection gave no id for the window.
    NoId,
    /// This session has no way to press keys in that window.
    Unsupported,
    /// A rule says `none` for this window.
    NoShortcut,
}

impl fmt::Display for Skip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Skip::Off => "paste.auto is off",
            Skip::StillShown => "the popup is still shown",
            Skip::NoWindow => "no window to paste into",
            Skip::NoId => "the window has no id",
            Skip::Unsupported => "this session cannot press keys in other windows",
            Skip::NoShortcut => "the paste shortcut for this window is none",
        })
    }
}

/// How keys reach the window, with what that needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Session {
    /// Hyprland's command socket.
    Hyprland { socket: PathBuf },
    /// Sway: i3-ipc for the focus, the Wayland socket for the virtual keyboard.
    Sway { swaysock: PathBuf, wayland: PathBuf },
    /// `$DISPLAY`.
    X11 { display: Option<String> },
}

/// The session `env` describes, if this build can paste in it.
pub fn session(env: &FocusEnv) -> Option<Session> {
    env.backends().into_iter().find_map(|b| session_for(env, b))
}

/// The popup can paste in this session: false on GNOME and KDE Wayland,
/// other compositors, and where the backend is not compiled in.
pub fn available(env: &FocusEnv) -> bool {
    session(env).is_some()
}

fn session_for(env: &FocusEnv, backend: FocusBackend) -> Option<Session> {
    match backend {
        FocusBackend::Hyprland => env.hyprland_socket().map(|socket| Session::Hyprland { socket }),
        FocusBackend::Sway if cfg!(feature = "wayland") => Some(Session::Sway {
            swaysock: env.swaysock.clone()?,
            wayland: wayland_socket(env)?,
        }),
        FocusBackend::X11 if cfg!(feature = "x11") => env.display.is_some().then(|| Session::X11 {
            display: env.display.clone(),
        }),
        _ => None,
    }
}

/// `$WAYLAND_DISPLAY`, relative to `$XDG_RUNTIME_DIR` unless absolute.
pub fn wayland_socket(env: &FocusEnv) -> Option<PathBuf> {
    let name = PathBuf::from(env.wayland_display.as_deref()?);
    if name.is_absolute() {
        return Some(name);
    }
    Some(env.xdg_runtime_dir.as_ref()?.join(name))
}

/// What [`run`] does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub session: Session,
    /// [`WindowInfo::id`] of the window to paste into.
    pub window_id: String,
    pub shortcut: Shortcut,
    pub wait: Duration,
    pub delay: Duration,
}

/// Decide whether to paste after a pick into `target`, the window the popup
/// was opened over. `popup_hidden`: the copy hid the popup.
pub fn plan(cfg: &PasteConfig, target: Option<&PasteTarget>, popup_hidden: bool) -> Result<Plan, Skip> {
    if !cfg.auto {
        return Err(Skip::Off);
    }
    if !popup_hidden {
        return Err(Skip::StillShown);
    }
    let target = target.ok_or(Skip::NoWindow)?;
    let session = session_for(&target.env, target.window.backend).ok_or(Skip::Unsupported)?;
    let window_id = target.window.id.clone().ok_or(Skip::NoId)?;
    let shortcut = cfg.shortcut_for(&target.window.class).ok_or(Skip::NoShortcut)?;
    Ok(Plan {
        session,
        window_id,
        shortcut,
        wait: Duration::from_millis(cfg.wait_ms.into()),
        delay: Duration::from_millis(cfg.delay_ms.into()),
    })
}

/// The window the popup opens over, kept for pasting: the focused window
/// unless it is the popup's own (`app_id`).
pub fn target(window: Option<WindowInfo>, env: &FocusEnv, app_id: &str) -> Option<PasteTarget> {
    let window = window.filter(|w| !w.is_app(app_id))?;
    Some(PasteTarget {
        window,
        env: env.clone(),
    })
}

/// What happened in [`run`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Pasted,
    /// The window did not get the focus back in time: nothing was pressed.
    FocusElsewhere,
}

/// Wait (up to `plan.wait`) for the window to have the focus, pause for
/// `plan.delay`, check again and press the shortcut. Blocking.
pub fn run(plan: &Plan) -> Result<Outcome, String> {
    let focused = || focused_id(&plan.session);
    if !wait_for_focus(&plan.window_id, plan.wait, POLL, focused) {
        return Ok(Outcome::FocusElsewhere);
    }
    if !plan.delay.is_zero() {
        std::thread::sleep(plan.delay);
        if focused().ok().flatten().as_deref() != Some(plan.window_id.as_str()) {
            return Ok(Outcome::FocusElsewhere);
        }
    }
    press(&plan.session, &plan.window_id, &plan.shortcut)?;
    Ok(Outcome::Pasted)
}

/// [`run`], with the outcome logged.
pub fn run_logged(plan: &Plan) {
    match run(plan) {
        Ok(Outcome::Pasted) => tracing::info!("pasted with {} in window {}", plan.shortcut, plan.window_id),
        Ok(Outcome::FocusElsewhere) => tracing::info!(
            "not pasting: window {} did not get the focus back within {} ms",
            plan.window_id,
            plan.wait.as_millis()
        ),
        Err(e) => tracing::warn!("pasting: {e}"),
    }
}

/// Poll `current` (the focused window's id) every `poll` until it is `id`
/// (true) or `wait` has passed (false). Errors count as "not yet".
pub fn wait_for_focus<E: fmt::Display>(
    id: &str,
    wait: Duration,
    poll: Duration,
    mut current: impl FnMut() -> Result<Option<String>, E>,
) -> bool {
    let deadline = Instant::now() + wait;
    loop {
        match current() {
            Ok(Some(now)) if now == id => return true,
            Ok(_) => {}
            Err(e) => tracing::debug!("reading the focus: {e}"),
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        std::thread::sleep(poll.min(deadline - now));
    }
}

/// The id of the focused window.
pub fn focused_id(session: &Session) -> Result<Option<String>, focus::FocusError> {
    let window = match session {
        Session::Hyprland { socket } => focus::hyprland_active_window(socket)?,
        Session::Sway { swaysock, .. } => focus::sway_focused_window(swaysock)?,
        Session::X11 { display } => focus::x11_active_window(display.as_deref())?,
    };
    Ok(window.and_then(|w| w.id))
}

/// Press `shortcut` in the window `window_id`, which has the focus.
pub fn press(session: &Session, window_id: &str, shortcut: &Shortcut) -> Result<(), String> {
    match session {
        Session::Hyprland { socket } => hyprland::send_shortcut(socket, shortcut, window_id),
        #[cfg(feature = "wayland")]
        Session::Sway { wayland, .. } => wayland::press(wayland, shortcut),
        #[cfg(feature = "x11")]
        Session::X11 { display } => x11::press(display.as_deref(), shortcut),
        #[allow(unreachable_patterns)]
        _ => Err("not compiled in".into()),
    }
}

/// What the macOS app needs to press Cmd+V after a pick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacPaste {
    /// The app the popup was opened over: paste only while it is frontmost.
    pub bundle_id: String,
    pub wait_ms: u32,
    pub delay_ms: u32,
}

/// [`plan`] for the macOS app: `front` is the bundle id of the app the popup
/// was opened over. It always presses Cmd+V, but a `paste.rules` entry of
/// `none` for the bundle id still turns pasting off there.
pub fn mac_plan(cfg: &PasteConfig, front: Option<&str>, popup_hidden: bool) -> Result<MacPaste, Skip> {
    if !cfg.auto {
        return Err(Skip::Off);
    }
    if !popup_hidden {
        return Err(Skip::StillShown);
    }
    let bundle_id = front.filter(|b| !b.is_empty()).ok_or(Skip::NoWindow)?;
    cfg.shortcut_for(bundle_id).ok_or(Skip::NoShortcut)?;
    Ok(MacPaste {
        bundle_id: bundle_id.to_string(),
        wait_ms: cfg.wait_ms,
        delay_ms: cfg.delay_ms,
    })
}

/// A modifier of a [`Shortcut`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modifier {
    Ctrl,
    Shift,
    Alt,
    Super,
}

impl Modifier {
    /// The modifiers `s` holds, in this order.
    pub fn of(s: &Shortcut) -> Vec<Modifier> {
        [
            (s.ctrl, Modifier::Ctrl),
            (s.shift, Modifier::Shift),
            (s.alt, Modifier::Alt),
            (s.super_key, Modifier::Super),
        ]
        .into_iter()
        .filter_map(|(on, m)| on.then_some(m))
        .collect()
    }

    /// Linux evdev code of its left key (`input-event-codes.h`).
    pub fn evdev(self) -> u32 {
        match self {
            Modifier::Ctrl => 29,
            Modifier::Shift => 42,
            Modifier::Alt => 56,
            Modifier::Super => 125,
        }
    }

    /// X11 keysym of its left key.
    pub fn keysym(self) -> u32 {
        match self {
            Modifier::Ctrl => 0xffe3,
            Modifier::Shift => 0xffe1,
            Modifier::Alt => 0xffe9,
            Modifier::Super => 0xffeb,
        }
    }

    /// Its xkb real modifier mask (Control, Shift, Mod1, Mod4).
    pub fn mask(self) -> u32 {
        match self {
            Modifier::Shift => 1,
            Modifier::Ctrl => 4,
            Modifier::Alt => 8,
            Modifier::Super => 64,
        }
    }
}

/// Linux evdev code of the key on a US keyboard.
pub fn evdev_code(key: ShortcutKey) -> u32 {
    const LETTERS: [u32; 26] = [
        30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, // a..m
        49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44, // n..z
    ];
    match key {
        ShortcutKey::Letter(c) => LETTERS[(c as u8 - b'a') as usize],
        ShortcutKey::Digit('0') => 11,
        ShortcutKey::Digit(c) => 1 + (c as u8 - b'0') as u32,
        ShortcutKey::Insert => 110,
    }
}

/// The key's keysym name (xkb): `v`, `1`, `Insert`.
pub fn keysym_name(key: ShortcutKey) -> String {
    match key {
        ShortcutKey::Letter(c) | ShortcutKey::Digit(c) => c.to_string(),
        ShortcutKey::Insert => "Insert".into(),
    }
}

/// The key's keysym (X11, xkb): lower-case letters and digits are their
/// ASCII codes.
pub fn keysym(key: ShortcutKey) -> u32 {
    match key {
        ShortcutKey::Letter(c) | ShortcutKey::Digit(c) => c as u32,
        ShortcutKey::Insert => 0xff63,
    }
}

#[cfg(test)]
mod tests;
