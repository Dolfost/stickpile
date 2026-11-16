use std::cell::RefCell;
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::sync::mpsc;

use stickpile_core::ClientConfig;

use super::*;

fn window(backend: FocusBackend, class: &str, id: Option<&str>) -> WindowInfo {
    WindowInfo {
        class: class.into(),
        title: "chat".into(),
        backend,
        id: id.map(Into::into),
    }
}

fn hypr_env(runtime: &std::path::Path) -> FocusEnv {
    FocusEnv {
        xdg_runtime_dir: Some(runtime.to_path_buf()),
        hyprland_instance_signature: Some("sig".into()),
        wayland_display: Some("wayland-1".into()),
        session_type: Some("wayland".into()),
        ..FocusEnv::default()
    }
}

fn sway_env() -> FocusEnv {
    FocusEnv {
        xdg_runtime_dir: Some("/run/user/1000".into()),
        swaysock: Some("/run/user/1000/sway-ipc.sock".into()),
        wayland_display: Some("wayland-1".into()),
        session_type: Some("wayland".into()),
        ..FocusEnv::default()
    }
}

fn x11_env() -> FocusEnv {
    FocusEnv {
        display: Some(":0".into()),
        session_type: Some("x11".into()),
        ..FocusEnv::default()
    }
}

fn gnome_env() -> FocusEnv {
    FocusEnv {
        xdg_runtime_dir: Some("/run/user/1000".into()),
        wayland_display: Some("wayland-0".into()),
        display: Some(":0".into()),
        session_type: Some("wayland".into()),
        ..FocusEnv::default()
    }
}

fn shortcut(text: &str) -> Shortcut {
    Shortcut::parse(text).unwrap().unwrap()
}

#[test]
fn sessions_that_can_paste() {
    let hypr = hypr_env("/run/user/1000".as_ref());
    assert_eq!(
        session(&hypr),
        Some(Session::Hyprland {
            socket: "/run/user/1000/hypr/sig/.socket.sock".into()
        })
    );
    assert!(available(&hypr));
    assert_eq!(
        session(&sway_env()),
        cfg!(feature = "wayland").then(|| Session::Sway {
            swaysock: "/run/user/1000/sway-ipc.sock".into(),
            wayland: "/run/user/1000/wayland-1".into(),
        })
    );
    assert_eq!(
        session(&x11_env()),
        cfg!(feature = "x11").then(|| Session::X11 {
            display: Some(":0".into())
        })
    );
    // GNOME / KDE on Wayland: no focus detection, so no pasting.
    assert_eq!(session(&gnome_env()), None);
    assert!(!available(&gnome_env()));
    assert!(!available(&FocusEnv::default()));
}

#[test]
fn wayland_socket_path() {
    let mut env = sway_env();
    assert_eq!(wayland_socket(&env), Some("/run/user/1000/wayland-1".into()));
    env.wayland_display = Some("/tmp/w/wayland-9".into());
    assert_eq!(wayland_socket(&env), Some("/tmp/w/wayland-9".into()));
    env.wayland_display = None;
    assert_eq!(wayland_socket(&env), None);
    let mut env = sway_env();
    env.xdg_runtime_dir = None;
    assert_eq!(wayland_socket(&env), None);
}

#[test]
fn the_target_is_the_window_the_popup_opened_over() {
    let env = hypr_env("/run".as_ref());
    let chat = window(FocusBackend::Hyprland, "vesktop", Some("0x1"));
    assert_eq!(
        target(Some(chat.clone()), &env, crate::glue::APP_ID),
        Some(PasteTarget {
            window: chat,
            env: env.clone()
        })
    );
    let own = window(FocusBackend::Hyprland, crate::glue::APP_ID, Some("0x2"));
    assert_eq!(target(Some(own), &env, crate::glue::APP_ID), None, "the popup itself");
    assert_eq!(target(None, &env, crate::glue::APP_ID), None);
}

#[test]
fn plan_decides_whether_and_how() {
    let cfg = ClientConfig::default().paste;
    let env = hypr_env("/run".as_ref());
    let over = |class: &str, id: Option<&str>| PasteTarget {
        window: window(FocusBackend::Hyprland, class, id),
        env: env.clone(),
    };

    let p = plan(&cfg, Some(&over("vesktop", Some("0xabc"))), true).unwrap();
    assert_eq!(
        p,
        Plan {
            session: Session::Hyprland {
                socket: "/run/hypr/sig/.socket.sock".into()
            },
            window_id: "0xabc".into(),
            shortcut: shortcut("ctrl+v"),
            wait: Duration::from_millis(1000),
            delay: Duration::from_millis(50),
        }
    );
    let p = plan(&cfg, Some(&over("kitty", Some("0xabc"))), true).unwrap();
    assert_eq!(p.shortcut, shortcut("ctrl+shift+v"), "terminals");

    assert_eq!(
        plan(&cfg, Some(&over("vesktop", Some("0x1"))), false),
        Err(Skip::StillShown)
    );
    assert_eq!(plan(&cfg, None, true), Err(Skip::NoWindow));
    assert_eq!(plan(&cfg, Some(&over("vesktop", None)), true), Err(Skip::NoId));
    let off = PasteConfig {
        auto: false,
        ..cfg.clone()
    };
    assert_eq!(plan(&off, Some(&over("vesktop", Some("0x1"))), true), Err(Skip::Off));
    let none = ClientConfig::from_toml("[[paste.rules]]\ncontains = [\"vesktop\"]\nshortcut = \"none\"\n")
        .unwrap()
        .paste;
    assert_eq!(
        plan(&none, Some(&over("vesktop", Some("0x1"))), true),
        Err(Skip::NoShortcut)
    );

    // The window came from Hyprland, but the command ran without its socket.
    let gone = PasteTarget {
        window: window(FocusBackend::Hyprland, "vesktop", Some("0x1")),
        env: gnome_env(),
    };
    assert_eq!(plan(&cfg, Some(&gone), true), Err(Skip::Unsupported));
    let mac = PasteTarget {
        window: window(FocusBackend::MacOs, "com.hnc.Discord", None),
        env: gnome_env(),
    };
    assert_eq!(plan(&cfg, Some(&mac), true), Err(Skip::Unsupported));
    for skip in [Skip::Off, Skip::Unsupported, Skip::NoShortcut] {
        assert!(!skip.to_string().is_empty());
    }
}

#[test]
fn mac_plan_presses_cmd_v_in_the_front_app() {
    let cfg = ClientConfig::default().paste;
    assert_eq!(
        mac_plan(&cfg, Some("com.hnc.Discord"), true),
        Ok(MacPaste {
            bundle_id: "com.hnc.Discord".into(),
            wait_ms: 1000,
            delay_ms: 50,
        })
    );
    // Terminals too: Cmd+V is their paste on macOS.
    assert!(mac_plan(&cfg, Some("net.kovidgoyal.kitty"), true).is_ok());
    assert_eq!(mac_plan(&cfg, Some("com.hnc.Discord"), false), Err(Skip::StillShown));
    assert_eq!(mac_plan(&cfg, None, true), Err(Skip::NoWindow));
    assert_eq!(mac_plan(&cfg, Some(""), true), Err(Skip::NoWindow));
    let off = PasteConfig {
        auto: false,
        ..cfg.clone()
    };
    assert_eq!(mac_plan(&off, Some("com.hnc.Discord"), true), Err(Skip::Off));
    let none = ClientConfig::from_toml("[[paste.rules]]\ncontains = [\"discord\"]\nshortcut = \"none\"\n")
        .unwrap()
        .paste;
    assert_eq!(mac_plan(&none, Some("com.hnc.Discord"), true), Err(Skip::NoShortcut));
}

#[test]
fn waiting_for_the_focus() {
    // The popup still has the focus twice, then the chat gets it back.
    let replies = RefCell::new(vec![
        Ok(Some("0xpopup".to_string())),
        Err("busy"),
        Ok(None),
        Ok(Some("0xchat".into())),
    ]);
    let calls = RefCell::new(0);
    let next = || {
        *calls.borrow_mut() += 1;
        let mut r = replies.borrow_mut();
        if r.len() > 1 {
            r.remove(0)
        } else {
            r[0].clone()
        }
    };
    assert!(wait_for_focus(
        "0xchat",
        Duration::from_secs(5),
        Duration::from_millis(1),
        next
    ));
    assert_eq!(*calls.borrow(), 4);

    // Another window keeps the focus: give up after the wait, nothing pressed.
    let started = Instant::now();
    let other = || Ok::<_, String>(Some("0xother".to_string()));
    assert!(!wait_for_focus(
        "0xchat",
        Duration::from_millis(60),
        Duration::from_millis(5),
        other
    ));
    let took = started.elapsed();
    assert!(
        took >= Duration::from_millis(60) && took < Duration::from_secs(2),
        "{took:?}"
    );

    // No wait: one look.
    let calls = RefCell::new(0);
    let once = || {
        *calls.borrow_mut() += 1;
        Ok::<_, String>(None)
    };
    assert!(!wait_for_focus(
        "0xchat",
        Duration::ZERO,
        Duration::from_millis(5),
        once
    ));
    assert_eq!(*calls.borrow(), 1);
}

#[test]
fn key_codes() {
    let k = |c| evdev_code(ShortcutKey::Letter(c));
    assert_eq!((k('a'), k('q'), k('v'), k('z'), k('m')), (30, 16, 47, 44, 50));
    assert_eq!(evdev_code(ShortcutKey::Digit('1')), 2);
    assert_eq!(evdev_code(ShortcutKey::Digit('9')), 10);
    assert_eq!(evdev_code(ShortcutKey::Digit('0')), 11);
    assert_eq!(evdev_code(ShortcutKey::Insert), 110);
    assert_eq!(keysym(ShortcutKey::Letter('v')), 0x76);
    assert_eq!(keysym(ShortcutKey::Digit('0')), 0x30);
    assert_eq!(keysym(ShortcutKey::Insert), 0xff63);
    assert_eq!(keysym_name(ShortcutKey::Letter('v')), "v");
    assert_eq!(keysym_name(ShortcutKey::Insert), "Insert");
    let s = shortcut("super+alt+shift+ctrl+v");
    assert_eq!(
        Modifier::of(&s),
        [Modifier::Ctrl, Modifier::Shift, Modifier::Alt, Modifier::Super]
    );
    let mask: u32 = Modifier::of(&s).iter().map(|m| m.mask()).sum();
    assert_eq!(mask, 4 | 1 | 8 | 64);
}

#[test]
fn hyprland_requests() {
    use hyprland::{command, keys, mods, valid_address, Syntax};
    let s = shortcut("ctrl+shift+v");
    assert_eq!(mods(&s), "CTRL SHIFT");
    assert_eq!(keys(&s), ["v".to_string(), "code:55".to_string()]);
    assert_eq!(keys(&shortcut("shift+insert"))[1], "code:118");
    assert_eq!(
        command(Syntax::Lua, "CTRL", "v", "0x5e5ee6b31ee0"),
        r#"dispatch hl.dsp.send_shortcut({ mods = "CTRL", key = "v", window = "address:0x5e5ee6b31ee0" })"#
    );
    assert_eq!(
        command(Syntax::Legacy, "CTRL SHIFT", "v", "0x5e5ee6b31ee0"),
        "dispatch sendshortcut CTRL SHIFT,v,address:0x5e5ee6b31ee0"
    );
    assert!(valid_address("0x5e5ee6b31ee0"));
    for bad in ["", "0x", "5e5e", "0x5e5e\"", "0x1 }) os.exit(", "0xg"] {
        assert!(!valid_address(bad), "{bad:?}");
    }
}

/// Feed `send_with` scripted replies; return the requests it sent.
fn hyprland_exchange(s: &Shortcut, address: &str, replies: &[&str]) -> (Result<(), String>, Vec<String>) {
    let mut sent = Vec::new();
    let mut replies = replies.iter();
    let result = hyprland::send_with(s, address, |request| {
        sent.push(request.to_string());
        Ok(replies.next().expect("a reply for every request").to_string())
    });
    (result, sent)
}

#[test]
fn hyprland_lua_then_legacy_then_keycode() {
    let v = shortcut("ctrl+v");
    // Hyprland 0.55 with a Lua config.
    let (r, sent) = hyprland_exchange(&v, "0x1", &["ok"]);
    assert_eq!(r, Ok(()));
    assert_eq!(sent.len(), 1);
    assert!(sent[0].starts_with("dispatch hl.dsp.send_shortcut("));

    // A hyprlang config: the Lua spelling is an unknown dispatcher.
    let (r, sent) = hyprland_exchange(&v, "0x1", &["Invalid dispatcher", "ok"]);
    assert_eq!(r, Ok(()));
    assert_eq!(sent[1], "dispatch sendshortcut CTRL,v,address:0x1");

    // A Cyrillic layout active: the keysym is not found, the keycode is.
    let (r, sent) = hyprland_exchange(
        &v,
        "0x1",
        &["error: hyprctl eval:1: send_shortcut: key not found", "ok"],
    );
    assert_eq!(r, Ok(()));
    assert!(sent[1].contains(r#"key = "code:55""#), "{}", sent[1]);
    let (r, sent) = hyprland_exchange(&v, "0x1", &["Invalid dispatcher", "sendshortcut: key not found", "ok"]);
    assert_eq!(r, Ok(()));
    assert_eq!(sent[2], "dispatch sendshortcut CTRL,code:55,address:0x1");

    // The window is gone: an error, no retries.
    let (r, sent) = hyprland_exchange(&v, "0x1", &["warn: send_shortcut: window not found"]);
    assert_eq!(r.unwrap_err(), "hyprland: warn: send_shortcut: window not found");
    assert_eq!(sent.len(), 1);

    // A bad address is never sent.
    let (r, sent) = hyprland_exchange(&v, "0x1\" })", &[]);
    assert!(r.is_err());
    assert!(sent.is_empty());
}

/// A fake Hyprland command socket: answers `j/activewindow` with the
/// addresses in `focus` (the last one repeats) and `dispatch` with `ok`.
/// Returns the requests it got.
fn fake_hyprland(
    dir: &std::path::Path,
    focus: Vec<&'static str>,
) -> (std::thread::JoinHandle<()>, mpsc::Receiver<String>) {
    let sock_dir = dir.join("hypr").join("sig");
    std::fs::create_dir_all(&sock_dir).unwrap();
    let listener = UnixListener::bind(sock_dir.join(".socket.sock")).unwrap();
    let (tx, rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let mut focus = focus.into_iter().peekable();
        let mut current = "";
        for conn in listener.incoming() {
            let mut conn = conn.unwrap();
            let mut buf = [0u8; 4096];
            let n = conn.read(&mut buf).unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).into_owned();
            if request == "quit" {
                return;
            }
            let reply = if request == "j/activewindow" {
                if let Some(next) = focus.next() {
                    current = next;
                }
                format!(r#"{{"address": "{current}", "class": "c", "title": "t"}}"#)
            } else {
                "ok".to_string()
            };
            tx.send(request).unwrap();
            conn.write_all(reply.as_bytes()).unwrap();
        }
    });
    (server, rx)
}

fn stop(dir: &std::path::Path, server: std::thread::JoinHandle<()>) {
    let mut s = std::os::unix::net::UnixStream::connect(dir.join("hypr/sig/.socket.sock")).unwrap();
    s.write_all(b"quit").unwrap();
    server.join().unwrap();
}

#[test]
fn run_pastes_once_the_window_is_back() {
    let dir = tempfile::tempdir().unwrap();
    let (server, requests) = fake_hyprland(dir.path(), vec!["0x9090", "0x9090", "0xc4a7"]);
    let env = hypr_env(dir.path());
    let target = PasteTarget {
        window: window(FocusBackend::Hyprland, "kitty", Some("0xc4a7")),
        env,
    };
    let mut p = plan(&ClientConfig::default().paste, Some(&target), true).unwrap();
    p.delay = Duration::from_millis(5);
    assert_eq!(run(&p), Ok(Outcome::Pasted));
    stop(dir.path(), server);
    let got: Vec<String> = requests.try_iter().collect();
    assert_eq!(
        got,
        [
            "j/activewindow",
            "j/activewindow",
            "j/activewindow",
            // After the delay, a second look.
            "j/activewindow",
            r#"dispatch hl.dsp.send_shortcut({ mods = "CTRL SHIFT", key = "v", window = "address:0xc4a7" })"#,
        ]
    );
}

#[test]
fn run_never_pastes_into_another_window() {
    let dir = tempfile::tempdir().unwrap();
    let (server, requests) = fake_hyprland(dir.path(), vec!["0x9090", "0xb40"]);
    let target = PasteTarget {
        window: window(FocusBackend::Hyprland, "vesktop", Some("0xc4a7")),
        env: hypr_env(dir.path()),
    };
    let mut p = plan(&ClientConfig::default().paste, Some(&target), true).unwrap();
    p.wait = Duration::from_millis(80);
    assert_eq!(run(&p), Ok(Outcome::FocusElsewhere));
    stop(dir.path(), server);
    let got: Vec<String> = requests.try_iter().collect();
    assert!(got.len() >= 2);
    assert!(got.iter().all(|r| r == "j/activewindow"), "{got:?}");

    // Back for a moment, gone again after the delay: still nothing.
    let dir = tempfile::tempdir().unwrap();
    let (server, requests) = fake_hyprland(dir.path(), vec!["0xc4a7", "0xb40"]);
    let target = PasteTarget {
        window: window(FocusBackend::Hyprland, "vesktop", Some("0xc4a7")),
        env: hypr_env(dir.path()),
    };
    let p = plan(&ClientConfig::default().paste, Some(&target), true).unwrap();
    assert_eq!(run(&p), Ok(Outcome::FocusElsewhere));
    stop(dir.path(), server);
    assert!(requests.try_iter().all(|r| r == "j/activewindow"));
}

#[test]
fn run_reports_a_dead_socket() {
    let dir = tempfile::tempdir().unwrap();
    let p = Plan {
        session: Session::Hyprland {
            socket: dir.path().join("missing.sock"),
        },
        window_id: "0x1".into(),
        shortcut: shortcut("ctrl+v"),
        wait: Duration::from_millis(30),
        delay: Duration::ZERO,
    };
    // The focus cannot be read, so the window never "comes back".
    assert_eq!(run(&p), Ok(Outcome::FocusElsewhere));
    assert!(press(&p.session, "0x1", &p.shortcut).is_err());
}

#[cfg(feature = "wayland")]
#[test]
fn virtual_keyboard_keymap_and_events() {
    let s = shortcut("ctrl+shift+v");
    let km = wayland::keymap(&s);
    assert!(km.starts_with("xkb_keymap {"));
    assert!(km.contains("<LCTL> = 37;") && km.contains("<LFSH> = 50;") && km.contains("<PSTE> = 55;"));
    assert!(km.contains("key <PSTE> { [ v, V ] };"));
    assert!(km.contains("modifier_map Control { <LCTL> };"));
    assert_eq!(km.matches('{').count(), km.matches('}').count());
    assert!(wayland::keymap(&shortcut("shift+insert")).contains("key <PSTE> { [ Insert ] };"));
    assert!(wayland::keymap(&shortcut("ctrl+1")).contains("<PSTE> = 10;"));
    assert!(km.is_ascii());

    assert_eq!(
        wayland::events(&s),
        [
            (29, true, 4),
            (42, true, 5),
            (47, true, 5),
            (47, false, 5),
            (42, false, 4),
            (29, false, 0),
        ]
    );
    assert_eq!(wayland::events(&shortcut("insert")), [(110, true, 0), (110, false, 0)]);
}

#[cfg(feature = "x11")]
#[test]
fn x11_keycodes_from_the_keyboard_mapping() {
    use x11::keycode_for;
    // From keycode 8, 4 keysyms per key: a us,ua mapping.
    const NO: u32 = 0;
    let map = [
        NO, NO, NO, NO, // 8
        0xffe3, NO, 0xffe3, NO, // 9: Control_L
        0x76, 0x56, 0x6cc, 0x6ec, // 10: v V and Cyrillic
        0x6c1, 0x6e1, 0x31, 0x21, // 11: Cyrillic first, 1 in the second group
        0x32, 0x40, NO, NO, // 12: 2 @
    ];
    assert_eq!(keycode_for(&map, 4, 8, 0xffe3), Some(9));
    assert_eq!(keycode_for(&map, 4, 8, 0x76), Some(10));
    assert_eq!(keycode_for(&map, 4, 8, 0x56), Some(10), "the shifted column");
    assert_eq!(keycode_for(&map, 4, 8, 0x31), Some(11), "anywhere");
    assert_eq!(keycode_for(&map, 4, 8, 0x32), Some(12));
    assert_eq!(keycode_for(&map, 4, 8, 0xff63), None);
    assert_eq!(keycode_for(&map, 0, 8, 0x76), None);
}
