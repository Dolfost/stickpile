//! Focused-window detection, used to pick the target chat app when the popup
//! opens. The shell must call [`detect`] *before* it shows its own window,
//! otherwise the compositor reports the popup itself.
//!
//! Backends, tried in this order when their environment is present:
//!
//! * **Hyprland**: `$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket.sock`,
//!   request `j/activewindow` (the same JSON `hyprctl -j activewindow` prints).
//! * **Sway** (and other i3-ipc compositors): `$SWAYSOCK`, `GET_TREE`, the
//!   focused node's `app_id` or `window_properties.class`.
//! * **X11** (feature `x11`): `_NET_ACTIVE_WINDOW` on the root window, then
//!   `WM_CLASS` and `_NET_WM_NAME`. Only used in X11 sessions: under XWayland it
//!   would only see X clients.
//!
//! * **macOS**: the app reports the frontmost application itself (its
//!   bundle id as the class, its name as the title); nothing here asks.
//!
//! Parsers are pure functions over the captured replies; the socket I/O is a
//! few small functions with a short timeout so a hung compositor cannot block
//! the popup.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How long a compositor IPC round trip may take before we give up.
pub const IPC_TIMEOUT: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusBackend {
    Hyprland,
    Sway,
    X11,
    /// Given by the macOS app (`NSWorkspace.frontmostApplication`).
    MacOs,
}

impl FocusBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            FocusBackend::Hyprland => "hyprland",
            FocusBackend::Sway => "sway",
            FocusBackend::X11 => "x11",
            FocusBackend::MacOs => "macos",
        }
    }
}

/// The focused toplevel window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowInfo {
    /// Wayland app id or X11 `WM_CLASS` class, e.g. `com.ayugram.desktop`, `vesktop`.
    pub class: String,
    pub title: String,
    pub backend: FocusBackend,
}

impl WindowInfo {
    /// True if this is the window of the application `app_id` (the popup
    /// itself), compared case-insensitively.
    pub fn is_app(&self, app_id: &str) -> bool {
        self.class.eq_ignore_ascii_case(app_id)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FocusError {
    #[error("{} IPC: {source}", backend.as_str())]
    Io { backend: FocusBackend, source: io::Error },
    #[error("{} reply: {message}", backend.as_str())]
    Parse { backend: FocusBackend, message: String },
    #[error("x11: {0}")]
    X11(String),
    #[error("x11 support not compiled in (feature `x11`)")]
    X11Disabled,
}

/// The environment variables detection depends on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FocusEnv {
    pub xdg_runtime_dir: Option<PathBuf>,
    pub hyprland_instance_signature: Option<String>,
    pub swaysock: Option<PathBuf>,
    pub wayland_display: Option<String>,
    pub display: Option<String>,
    /// `XDG_SESSION_TYPE` (`wayland`, `x11`, `tty`).
    pub session_type: Option<String>,
}

impl FocusEnv {
    pub fn from_process() -> FocusEnv {
        FocusEnv::from_vars(|k| std::env::var(k).ok())
    }

    /// Build from a variable lookup; empty values count as unset.
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> FocusEnv {
        let get = |k: &str| get(k).filter(|v| !v.is_empty());
        FocusEnv {
            xdg_runtime_dir: get("XDG_RUNTIME_DIR").map(PathBuf::from),
            hyprland_instance_signature: get("HYPRLAND_INSTANCE_SIGNATURE"),
            swaysock: get("SWAYSOCK").map(PathBuf::from),
            wayland_display: get("WAYLAND_DISPLAY"),
            display: get("DISPLAY"),
            session_type: get("XDG_SESSION_TYPE"),
        }
    }

    /// Backends worth trying, in order.
    pub fn backends(&self) -> Vec<FocusBackend> {
        let mut out = Vec::new();
        if self.hyprland_socket().is_some() {
            out.push(FocusBackend::Hyprland);
        }
        if self.swaysock.is_some() {
            out.push(FocusBackend::Sway);
        }
        let x11_session = match self.session_type.as_deref() {
            Some(t) => t.eq_ignore_ascii_case("x11"),
            None => self.wayland_display.is_none(),
        };
        if self.display.is_some() && x11_session {
            out.push(FocusBackend::X11);
        }
        out
    }

    /// `$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket.sock`
    pub fn hyprland_socket(&self) -> Option<PathBuf> {
        let sig = self.hyprland_instance_signature.as_deref()?;
        if sig.contains('/') {
            return None;
        }
        Some(
            self.xdg_runtime_dir
                .as_ref()?
                .join("hypr")
                .join(sig)
                .join(".socket.sock"),
        )
    }
}

/// Detect the focused window from the process environment.
pub fn detect() -> Result<Option<WindowInfo>, FocusError> {
    detect_with(&FocusEnv::from_process())
}

/// Try each available backend in order. The first backend that answers wins,
/// even if it reports no focused window; errors fall through to the next
/// backend and the last error is returned if none answered. No backend at all
/// (GNOME, KDE Wayland, a tty) is `Ok(None)`.
pub fn detect_with(env: &FocusEnv) -> Result<Option<WindowInfo>, FocusError> {
    let mut last_err = None;
    for backend in env.backends() {
        let result = match backend {
            FocusBackend::Hyprland => {
                let path = env.hyprland_socket().expect("listed only with a socket path");
                hyprland_active_window(&path)
            }
            FocusBackend::Sway => sway_focused_window(env.swaysock.as_deref().expect("listed only with SWAYSOCK")),
            FocusBackend::X11 => x11_active_window(env.display.as_deref()),
            // Never listed by `backends()`: the macOS app asks the system itself.
            FocusBackend::MacOs => Ok(None),
        };
        match result {
            Ok(w) => return Ok(w),
            Err(e) => last_err = Some(e),
        }
    }
    match last_err {
        Some(e) => Err(e),
        None => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// Hyprland
// ---------------------------------------------------------------------------

/// Send one request (e.g. `j/activewindow`) to Hyprland's command socket and
/// read the whole reply (Hyprland closes the connection after answering).
pub fn hyprland_request(socket: &Path, request: &str) -> io::Result<String> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(IPC_TIMEOUT))?;
    stream.set_write_timeout(Some(IPC_TIMEOUT))?;
    stream.write_all(request.as_bytes())?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf)?;
    String::from_utf8(buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub fn hyprland_active_window(socket: &Path) -> Result<Option<WindowInfo>, FocusError> {
    let reply = hyprland_request(socket, "j/activewindow").map_err(|source| FocusError::Io {
        backend: FocusBackend::Hyprland,
        source,
    })?;
    parse_hyprland_activewindow(&reply)
}

/// Parse the reply of `j/activewindow`. Hyprland answers `{}` when nothing is
/// focused (empty workspace). `class` falls back to `initialClass`.
pub fn parse_hyprland_activewindow(json: &str) -> Result<Option<WindowInfo>, FocusError> {
    let parse_err = |message: String| FocusError::Parse {
        backend: FocusBackend::Hyprland,
        message,
    };
    let v: Value = serde_json::from_str(json.trim()).map_err(|e| parse_err(e.to_string()))?;
    let obj = v
        .as_object()
        .ok_or_else(|| parse_err("expected a JSON object".into()))?;
    let s = |k: &str| obj.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let mut class = s("class");
    if class.is_empty() {
        class = s("initialClass");
    }
    let mut title = s("title");
    if title.is_empty() {
        title = s("initialTitle");
    }
    if class.is_empty() && title.is_empty() {
        return Ok(None);
    }
    Ok(Some(WindowInfo {
        class,
        title,
        backend: FocusBackend::Hyprland,
    }))
}

// ---------------------------------------------------------------------------
// Sway / i3 IPC
// ---------------------------------------------------------------------------

pub const I3_MAGIC: &[u8; 6] = b"i3-ipc";
pub const I3_GET_TREE: u32 = 4;
/// Replies larger than this are treated as corrupt.
const I3_MAX_REPLY: usize = 64 * 1024 * 1024;

/// `"i3-ipc" <u32 length> <u32 type> <payload>`, integers in native byte order.
pub fn encode_i3_message(kind: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(14 + payload.len());
    out.extend_from_slice(I3_MAGIC);
    out.extend_from_slice(&(payload.len() as u32).to_ne_bytes());
    out.extend_from_slice(&kind.to_ne_bytes());
    out.extend_from_slice(payload);
    out
}

/// Read one i3-ipc message: `(type, payload)`.
pub fn read_i3_message(r: &mut impl Read) -> io::Result<(u32, Vec<u8>)> {
    let mut header = [0u8; 14];
    r.read_exact(&mut header)?;
    if &header[..6] != I3_MAGIC {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad i3-ipc magic"));
    }
    let len = u32::from_ne_bytes(header[6..10].try_into().expect("4 bytes")) as usize;
    let kind = u32::from_ne_bytes(header[10..14].try_into().expect("4 bytes"));
    if len > I3_MAX_REPLY {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "i3-ipc reply too large"));
    }
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    Ok((kind, payload))
}

/// `GET_TREE` over the i3-ipc socket.
pub fn sway_get_tree(socket: &Path) -> io::Result<String> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(IPC_TIMEOUT))?;
    stream.set_write_timeout(Some(IPC_TIMEOUT))?;
    stream.write_all(&encode_i3_message(I3_GET_TREE, b""))?;
    let (kind, payload) = read_i3_message(&mut stream)?;
    if kind != I3_GET_TREE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected i3-ipc reply type {kind}"),
        ));
    }
    String::from_utf8(payload).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub fn sway_focused_window(socket: &Path) -> Result<Option<WindowInfo>, FocusError> {
    let tree = sway_get_tree(socket).map_err(|source| FocusError::Io {
        backend: FocusBackend::Sway,
        source,
    })?;
    parse_sway_tree(&tree)
}

/// Find the focused window in a `GET_TREE` reply. A focused workspace (no
/// window) yields `None`.
pub fn parse_sway_tree(json: &str) -> Result<Option<WindowInfo>, FocusError> {
    let v: Value = serde_json::from_str(json).map_err(|e| FocusError::Parse {
        backend: FocusBackend::Sway,
        message: e.to_string(),
    })?;
    let Some(node) = find_focused(&v) else { return Ok(None) };
    let kind = node.get("type").and_then(Value::as_str).unwrap_or("");
    if kind != "con" && kind != "floating_con" {
        return Ok(None);
    }
    let props = node.get("window_properties");
    let prop = |k: &str| {
        props
            .and_then(|p| p.get(k))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    };
    let class = node
        .get("app_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| prop("class"))
        .or_else(|| prop("instance"))
        .unwrap_or("")
        .to_string();
    let title = node
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| prop("title"))
        .unwrap_or("")
        .to_string();
    if class.is_empty() && title.is_empty() {
        return Ok(None);
    }
    Ok(Some(WindowInfo {
        class,
        title,
        backend: FocusBackend::Sway,
    }))
}

fn find_focused(node: &Value) -> Option<&Value> {
    if node.get("focused").and_then(Value::as_bool) == Some(true) {
        return Some(node);
    }
    ["nodes", "floating_nodes"]
        .iter()
        .filter_map(|k| node.get(*k).and_then(Value::as_array))
        .flatten()
        .find_map(find_focused)
}

// ---------------------------------------------------------------------------
// X11
// ---------------------------------------------------------------------------

/// Split a `WM_CLASS` property (`instance\0class\0`) into `(instance, class)`.
pub fn parse_wm_class(bytes: &[u8]) -> Option<(String, String)> {
    let mut parts = bytes
        .split(|b| *b == 0)
        .map(|p| String::from_utf8_lossy(p).into_owned());
    let instance = parts.next().unwrap_or_default();
    let class = parts.next().unwrap_or_default();
    if instance.is_empty() && class.is_empty() {
        None
    } else {
        Some((instance, class))
    }
}

#[cfg(feature = "x11")]
pub fn x11_active_window(display: Option<&str>) -> Result<Option<WindowInfo>, FocusError> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, Window};

    let err = |e: &dyn std::fmt::Display| FocusError::X11(e.to_string());
    let (conn, screen) = x11rb::connect(display).map_err(|e| err(&e))?;
    let root = conn.setup().roots[screen].root;
    let atom = |name: &[u8]| -> Result<u32, FocusError> {
        Ok(conn
            .intern_atom(false, name)
            .map_err(|e| err(&e))?
            .reply()
            .map_err(|e| err(&e))?
            .atom)
    };
    let net_active = atom(b"_NET_ACTIVE_WINDOW")?;
    let reply = conn
        .get_property(false, root, net_active, AtomEnum::WINDOW, 0, 1)
        .map_err(|e| err(&e))?
        .reply()
        .map_err(|e| err(&e))?;
    let Some(window): Option<Window> = reply.value32().and_then(|mut it| it.next()).filter(|w| *w != 0) else {
        return Ok(None);
    };
    let prop = |property: u32, ty: u32| -> Result<Vec<u8>, FocusError> {
        Ok(conn
            .get_property(false, window, property, ty, 0, 4096)
            .map_err(|e| err(&e))?
            .reply()
            .map_err(|e| err(&e))?
            .value)
    };
    let class = parse_wm_class(&prop(AtomEnum::WM_CLASS.into(), AtomEnum::STRING.into())?)
        .map(|(instance, class)| if class.is_empty() { instance } else { class })
        .unwrap_or_default();
    let utf8 = atom(b"UTF8_STRING")?;
    let mut title = String::from_utf8_lossy(&prop(atom(b"_NET_WM_NAME")?, utf8)?).into_owned();
    if title.is_empty() {
        title = String::from_utf8_lossy(&prop(AtomEnum::WM_NAME.into(), AtomEnum::ANY.into())?).into_owned();
    }
    if class.is_empty() && title.is_empty() {
        return Ok(None);
    }
    Ok(Some(WindowInfo {
        class,
        title,
        backend: FocusBackend::X11,
    }))
}

#[cfg(not(feature = "x11"))]
pub fn x11_active_window(_display: Option<&str>) -> Result<Option<WindowInfo>, FocusError> {
    Err(FocusError::X11Disabled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::os::unix::net::UnixListener;

    /// Captured from Hyprland (Lua config era) with `hyprctl -j activewindow`.
    const HYPR_ACTIVE: &str = r#"{
        "address": "0x5e5ee6b31ee0", "mapped": true, "hidden": false, "visible": true,
        "at": [550, 12], "size": [1358, 1056],
        "workspace": { "id": -95, "name": "special:work" },
        "floating": false, "monitor": 0,
        "class": "element-desktop", "title": "Element [1] | T3.5 Leadership",
        "initialClass": "element-desktop", "initialTitle": "Element",
        "pid": 2823, "xwayland": false, "pinned": false, "fullscreen": 0,
        "grouped": [], "tags": [], "swallowing": "0x0", "focusHistoryID": 0,
        "xdgTag": "", "xdgDescription": "", "contentType": "none", "stableId": "18000013"
    }"#;

    const SWAY_TREE: &str = r#"{
      "id": 1, "type": "root", "name": "root", "focused": false,
      "nodes": [
        { "id": 3, "type": "output", "name": "eDP-1", "focused": false, "nodes": [
          { "id": 4, "type": "workspace", "name": "1", "focused": false,
            "nodes": [
              { "id": 7, "type": "con", "name": "Firefox", "app_id": "firefox", "focused": false, "nodes": [] }
            ],
            "floating_nodes": [
              { "id": 9, "type": "floating_con", "name": "AyuGram", "app_id": null,
                "window_properties": { "class": "AyuGramDesktop", "instance": "ayugramdesktop", "title": "AyuGram" },
                "focused": true, "nodes": [] }
            ] }
        ] }
      ]
    }"#;

    fn env(vars: &[(&str, &str)]) -> FocusEnv {
        let map: HashMap<String, String> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        FocusEnv::from_vars(|k| map.get(k).cloned())
    }

    #[test]
    fn hyprland_reply_is_parsed() {
        let w = parse_hyprland_activewindow(HYPR_ACTIVE).unwrap().unwrap();
        assert_eq!(w.class, "element-desktop");
        assert_eq!(w.title, "Element [1] | T3.5 Leadership");
        assert_eq!(w.backend, FocusBackend::Hyprland);
    }

    #[test]
    fn hyprland_empty_object_means_no_window() {
        assert_eq!(parse_hyprland_activewindow("{}").unwrap(), None);
        assert_eq!(parse_hyprland_activewindow(" {}\n").unwrap(), None);
    }

    #[test]
    fn hyprland_falls_back_to_initial_class() {
        let w = parse_hyprland_activewindow(r#"{"class":"","initialClass":"com.ayugram.desktop","title":"Chat"}"#)
            .unwrap()
            .unwrap();
        assert_eq!(w.class, "com.ayugram.desktop");
    }

    #[test]
    fn hyprland_garbage_is_an_error() {
        assert!(parse_hyprland_activewindow("unknown request").is_err());
        assert!(parse_hyprland_activewindow("[1,2]").is_err());
    }

    #[test]
    fn sway_finds_focused_floating_x11_window() {
        let w = parse_sway_tree(SWAY_TREE).unwrap().unwrap();
        assert_eq!(w.class, "AyuGramDesktop");
        assert_eq!(w.title, "AyuGram");
        assert_eq!(w.backend, FocusBackend::Sway);
    }

    #[test]
    fn sway_prefers_app_id() {
        let tree = SWAY_TREE.replace(
            r#""focused": false, "nodes": [] }"#,
            r#""focused": true, "nodes": [] }"#,
        );
        // Now the tiled Firefox node is focused too and comes first in the walk.
        let w = parse_sway_tree(&tree).unwrap().unwrap();
        assert_eq!(w.class, "firefox");
    }

    #[test]
    fn sway_focused_workspace_has_no_window() {
        let tree =
            r#"{"type":"root","focused":false,"nodes":[{"type":"workspace","name":"2","focused":true,"nodes":[]}]}"#;
        assert_eq!(parse_sway_tree(tree).unwrap(), None);
        assert_eq!(parse_sway_tree(r#"{"type":"root","focused":false}"#).unwrap(), None);
    }

    #[test]
    fn i3_message_round_trip() {
        let msg = encode_i3_message(I3_GET_TREE, b"{}");
        assert_eq!(&msg[..6], b"i3-ipc");
        assert_eq!(msg.len(), 16);
        let (kind, payload) = read_i3_message(&mut &msg[..]).unwrap();
        assert_eq!(kind, I3_GET_TREE);
        assert_eq!(payload, b"{}");
        let mut bad = msg.clone();
        bad[0] = b'x';
        assert!(read_i3_message(&mut &bad[..]).is_err());
        assert!(read_i3_message(&mut &msg[..10]).is_err());
    }

    #[test]
    fn wm_class_split() {
        assert_eq!(
            parse_wm_class(b"vesktop\0vesktop\0"),
            Some(("vesktop".into(), "vesktop".into()))
        );
        assert_eq!(
            parse_wm_class(b"Navigator\0firefox\0"),
            Some(("Navigator".into(), "firefox".into()))
        );
        assert_eq!(parse_wm_class(b""), None);
    }

    #[test]
    fn backend_selection_from_env() {
        let hypr = env(&[
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
            ("HYPRLAND_INSTANCE_SIGNATURE", "abc_1"),
            ("WAYLAND_DISPLAY", "wayland-1"),
            ("DISPLAY", ":0"),
            ("XDG_SESSION_TYPE", "wayland"),
        ]);
        assert_eq!(hypr.backends(), vec![FocusBackend::Hyprland]);
        assert_eq!(
            hypr.hyprland_socket().unwrap(),
            PathBuf::from("/run/user/1000/hypr/abc_1/.socket.sock")
        );

        let sway = env(&[
            ("SWAYSOCK", "/run/user/1000/sway-ipc.sock"),
            ("WAYLAND_DISPLAY", "wayland-1"),
        ]);
        assert_eq!(sway.backends(), vec![FocusBackend::Sway]);

        let x11 = env(&[("DISPLAY", ":0"), ("XDG_SESSION_TYPE", "x11")]);
        assert_eq!(x11.backends(), vec![FocusBackend::X11]);
        let x11_no_session_type = env(&[("DISPLAY", ":0")]);
        assert_eq!(x11_no_session_type.backends(), vec![FocusBackend::X11]);

        let gnome_wayland = env(&[
            ("WAYLAND_DISPLAY", "wayland-0"),
            ("DISPLAY", ":0"),
            ("XDG_SESSION_TYPE", "wayland"),
        ]);
        assert!(gnome_wayland.backends().is_empty());
        assert_eq!(detect_with(&gnome_wayland).unwrap(), None);

        // Empty values and path tricks in the signature are ignored.
        let odd = env(&[
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
            ("HYPRLAND_INSTANCE_SIGNATURE", "../x"),
        ]);
        assert!(odd.hyprland_socket().is_none());
        let empty = env(&[("HYPRLAND_INSTANCE_SIGNATURE", ""), ("SWAYSOCK", "")]);
        assert_eq!(empty, FocusEnv::default());
    }

    #[test]
    fn hyprland_socket_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let hypr_dir = dir.path().join("hypr").join("sig_1");
        std::fs::create_dir_all(&hypr_dir).unwrap();
        let listener = UnixListener::bind(hypr_dir.join(".socket.sock")).unwrap();
        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut buf = [0u8; 64];
            let n = conn.read(&mut buf).unwrap();
            assert_eq!(&buf[..n], b"j/activewindow");
            conn.write_all(HYPR_ACTIVE.as_bytes()).unwrap();
        });
        let e = FocusEnv {
            xdg_runtime_dir: Some(dir.path().to_path_buf()),
            hyprland_instance_signature: Some("sig_1".into()),
            ..FocusEnv::default()
        };
        let w = detect_with(&e).unwrap().unwrap();
        server.join().unwrap();
        assert_eq!(w.class, "element-desktop");
    }

    #[test]
    fn sway_socket_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sway.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let (kind, payload) = read_i3_message(&mut conn).unwrap();
            assert_eq!(kind, I3_GET_TREE);
            assert!(payload.is_empty());
            conn.write_all(&encode_i3_message(I3_GET_TREE, SWAY_TREE.as_bytes()))
                .unwrap();
        });
        let w = sway_focused_window(&path).unwrap().unwrap();
        server.join().unwrap();
        assert_eq!(w.class, "AyuGramDesktop");
    }

    #[test]
    fn unreachable_socket_is_an_error_and_falls_through() {
        let dir = tempfile::tempdir().unwrap();
        let e = FocusEnv {
            xdg_runtime_dir: Some(dir.path().to_path_buf()),
            hyprland_instance_signature: Some("gone".into()),
            swaysock: Some(dir.path().join("missing.sock")),
            ..FocusEnv::default()
        };
        assert_eq!(e.backends(), vec![FocusBackend::Hyprland, FocusBackend::Sway]);
        let err = detect_with(&e).unwrap_err();
        assert!(err.to_string().starts_with("sway IPC"), "{err}");
    }

    #[test]
    fn own_window_is_recognised() {
        let w = WindowInfo {
            class: "io.github.dolfost.Stickpile".into(),
            title: "stickpile".into(),
            backend: FocusBackend::Hyprland,
        };
        assert!(w.is_app("io.github.dolfost.stickpile"));
        assert!(!w.is_app("firefox"));
    }
}
