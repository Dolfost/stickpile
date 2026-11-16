//! Hyprland: the `send_shortcut` dispatcher over the command socket
//! (`$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket.sock`), aimed
//! at one window by its address. Hyprland sends the key events to that
//! window's surface itself.
//!
//! The request has two spellings (read from Hyprland 0.55's
//! `src/debug/HyprCtl.cpp`): with a Lua config, `dispatch` takes a Lua
//! expression, `hl.dsp.send_shortcut({ mods = ..., key = ..., window = ... })`;
//! with a hyprlang config (and before 0.55) it takes `sendshortcut MODS,KEY,WINDOW`.
//! The Lua one is sent first; a hyprlang Hyprland answers it with
//! "Invalid dispatcher", and then the old one is sent. Either answers `ok`.
//!
//! The key is given by its keysym name, which Hyprland looks up in the
//! active layout; with a layout active that has no such key (Cyrillic, for
//! a `v`), it answers "key not found", and the key is sent again by its
//! keycode on a US keyboard (`code:55` for V).

use std::path::Path;

use stickpile_core::config::Shortcut;
use stickpile_quick::focus;

use super::{evdev_code, keysym_name, Modifier};

/// The spelling of the `dispatch` request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    /// Hyprland 0.55+ with a Lua config.
    Lua,
    /// hyprlang configs.
    Legacy,
}

/// `CTRL SHIFT`
pub fn mods(s: &Shortcut) -> String {
    Modifier::of(s)
        .into_iter()
        .map(|m| match m {
            Modifier::Ctrl => "CTRL",
            Modifier::Shift => "SHIFT",
            Modifier::Alt => "ALT",
            Modifier::Super => "SUPER",
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The keys to try in turn: the keysym name, then the US keycode
/// (`code:` takes xkb keycodes, evdev + 8).
pub fn keys(s: &Shortcut) -> [String; 2] {
    [keysym_name(s.key), format!("code:{}", evdev_code(s.key) + 8)]
}

/// The request for `send_shortcut` to the window at `address`.
pub fn command(syntax: Syntax, mods: &str, key: &str, address: &str) -> String {
    match syntax {
        Syntax::Lua => format!(
            "dispatch hl.dsp.send_shortcut({{ mods = \"{mods}\", key = \"{key}\", window = \"address:{address}\" }})"
        ),
        Syntax::Legacy => format!("dispatch sendshortcut {mods},{key},address:{address}"),
    }
}

/// A window address as `j/activewindow` gives it: `0x` and hex digits. Anything
/// else is refused before it is put into a request.
pub fn valid_address(address: &str) -> bool {
    address
        .strip_prefix("0x")
        .is_some_and(|hex| !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Send `shortcut` to the window at `address`.
pub fn send_shortcut(socket: &Path, shortcut: &Shortcut, address: &str) -> Result<(), String> {
    send_with(shortcut, address, |request| {
        focus::hyprland_request(socket, request).map_err(|e| e.to_string())
    })
}

/// [`send_shortcut`] over any transport: `request` sends one request and
/// returns the reply.
pub fn send_with(
    shortcut: &Shortcut,
    address: &str,
    mut request: impl FnMut(&str) -> Result<String, String>,
) -> Result<(), String> {
    if !valid_address(address) {
        return Err(format!("hyprland: not a window address: {address:?}"));
    }
    let mods = mods(shortcut);
    let mut syntax = Syntax::Lua;
    let keys = keys(shortcut);
    for (i, key) in keys.iter().enumerate() {
        let mut reply = request(&command(syntax, &mods, key, address))?;
        if syntax == Syntax::Lua && reply.contains("Invalid dispatcher") {
            syntax = Syntax::Legacy;
            reply = request(&command(syntax, &mods, key, address))?;
        }
        if reply.trim() == "ok" {
            return Ok(());
        }
        if i + 1 < keys.len() && reply.contains("key not found") {
            tracing::debug!("hyprland: {key} is not in the active layout, sending its keycode");
            continue;
        }
        return Err(format!("hyprland: {}", reply.trim()));
    }
    unreachable!("the last key returns")
}
