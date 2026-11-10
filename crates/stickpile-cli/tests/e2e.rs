//! Runs the real `stickpile` binary against the in-process mock server from
//! `stickpile-core` (feature `mock`), with a temporary config directory.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use stickpile_core::mock::MockServer;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_stickpile"))
}

struct Env {
    dir: tempfile::TempDir,
    config: PathBuf,
}

impl Env {
    fn new(server_url: &str) -> Env {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config/config.toml");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(
            &config,
            format!(
                "server_url = {server_url:?}\ntoken_file = {:?}\ncache_dir = {:?}\ntimeout_secs = 2\n",
                dir.path().join("state/token"),
                dir.path().join("cache"),
            ),
        )
        .unwrap();
        Env { dir, config }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(bin())
            .arg("--config")
            .arg(&self.config)
            .args(args)
            // No clipboard in tests; make sure nothing reaches a real display.
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY")
            .env_remove("STICKPILE_SERVER")
            .env_remove("STICKPILE_CONFIG")
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "stickpile {args:?} failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        let mut a = vec!["--json"];
        a.extend_from_slice(args);
        serde_json::from_str(&self.ok(&a)).unwrap()
    }
}

fn png(seed: u8) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(16, 16, |x, y| image::Rgba([x as u8 * seed, y as u8, seed, 255]));
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn gif() -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = image::codecs::gif::GifEncoder::new(&mut out);
        for i in 0..3u8 {
            let img = image::RgbaImage::from_pixel(8, 8, image::Rgba([i * 70, 0, 0, 255]));
            enc.encode_frame(image::Frame::new(img)).unwrap();
        }
    }
    out
}

fn write_set_dir(dir: &Path) -> (Vec<u8>, Vec<u8>) {
    std::fs::create_dir_all(dir).unwrap();
    let (a, b) = (png(3), gif());
    std::fs::write(dir.join("001-grumpy cat.png"), &a).unwrap();
    std::fs::write(dir.join("002-naahh.gif"), &b).unwrap();
    std::fs::write(
        dir.join("set.toml"),
        "name = \"My cats\"\nslug = \"my-cats\"\nvisibility = \"public\"\n\n\
         [items.\"001-grumpy cat.png\"]\nname = \"grumpy cat\"\nkeywords = [\"cat\", \"angry\"]\n\n\
         [items.\"002-naahh.gif\"]\nkeywords = [\"nope\", \"cat\"]\n",
    )
    .unwrap();
    (a, b)
}

#[test]
fn config_check_valid_invalid_missing() {
    // No server is running anywhere: `config check` must not need one.
    let env = Env::new("http://127.0.0.1:9");
    let out = env.ok(&["config", "check"]);
    assert!(out.starts_with("OK "), "{out}");

    let good = env.path("hm.toml");
    std::fs::write(
        &good,
        "server_url = \"https://stickers.example.ts.net\"\ncache_max_mb = 64\ndefault_target = \"telegram\"\n\
         [ui]\ncolumns = 8\n[clipboard.targets.telegram]\nstill = [\"text/uri-list\"]\n",
    )
    .unwrap();
    let out = env.ok(&["config", "check", "--file", good.to_str().unwrap()]);
    assert!(out.contains("hm.toml"));
    let parsed = env.json(&["config", "check", "--file", good.to_str().unwrap()]);
    assert_eq!(parsed["default_target"], "telegram");
    assert_eq!(parsed["ui"]["columns"], 8);
    assert_eq!(parsed["ui"]["thumb_px"], 128, "defaults filled in");

    for (name, text, expect) in [
        ("unknown.toml", "serverurl = \"http://x\"\n", "unknown field"),
        (
            "mime.toml",
            "[clipboard.default]\nstill = [\"image/bmp\"]\n",
            "image/bmp",
        ),
        ("url.toml", "server_url = \"ftp://x\"\n", "server_url"),
        ("syntax.toml", "server_url = \n", "parsing"),
    ] {
        let p = env.path(name);
        std::fs::write(&p, text).unwrap();
        let out = env.run(&["config", "check", "--file", p.to_str().unwrap()]);
        assert!(!out.status.success(), "{name} should fail");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(expect), "{name}: {err}");
    }

    let out = env.run(&["config", "check", "--file", env.path("missing.toml").to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("no such file"));

    assert_eq!(env.ok(&["config", "path"]).trim(), env.config.to_str().unwrap());
}

#[test]
fn login_import_search_copy_export_offline_logout() {
    let mut server = MockServer::start();
    let env = Env::new(&server.url);

    // Not logged in yet.
    let out = env.run(&["whoami"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("stickpile login"));

    // login (password from a file)
    std::fs::write(env.path("pw"), "secret\n").unwrap();
    let out = env.ok(&[
        "login",
        "--username",
        "alice",
        "--password-file",
        env.path("pw").to_str().unwrap(),
        "--token-name",
        "e2e",
    ]);
    assert!(out.contains("Logged in"), "{out}");
    let token_file = env.path("state/token");
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&token_file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(env.json(&["whoami"])["username"], "alice");
    assert_eq!(server.state().tokens[0].info.name, "e2e");

    // wrong password
    std::fs::write(env.path("badpw"), "nope\n").unwrap();
    let out = env.run(&[
        "login",
        "-u",
        "alice",
        "--password-file",
        env.path("badpw").to_str().unwrap(),
    ]);
    assert!(!out.status.success());

    // import a set directory
    let src = env.path("my-cats");
    let (png_bytes, gif_bytes) = write_set_dir(&src);
    let out = env.ok(&["import", src.to_str().unwrap()]);
    assert!(out.contains("2 created"), "{out}");
    let sets = env.json(&["sets", "list", "--scope", "mine"]);
    assert_eq!(sets[0]["slug"], "my-cats");
    let show = env.ok(&["sets", "show", "my-cats"]);
    assert!(show.contains("grumpy cat") && show.contains("naahh"), "{show}");

    // upload another file: same content elsewhere -> near-duplicate note
    env.ok(&["sets", "create", "inbox", "--name", "Inbox"]);
    std::fs::write(env.path("dup.png"), &png_bytes).unwrap();
    let out = env.ok(&["upload", "inbox", env.path("dup.png").to_str().unwrap()]);
    assert!(out.contains("you already have the same file in set my-cats"), "{out}");

    // search
    let res = env.json(&["search", "cat", "--target", "discord"]);
    assert_eq!(res["offline"], false);
    let names: Vec<&str> = res["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["item"]["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"grumpy cat") && names.contains(&"naahh"), "{names:?}");
    let table = env.ok(&["search", "naahh"]);
    assert!(table.lines().next().unwrap().starts_with("#"), "{table}");

    // copy without a display: print the materialised file
    let path = env.ok(&["copy", "naahh", "--print-path"]);
    let path = PathBuf::from(path.trim());
    assert!(path.starts_with(env.path("cache/clip")), "{path:?}");
    assert_eq!(path.file_name().unwrap(), "naahh.gif");
    assert_eq!(std::fs::read(&path).unwrap(), gif_bytes);
    assert_eq!(server.state().usage.len(), 1, "usage flushed before exit");
    let out = env.run(&["copy", "naahh"]);
    assert!(!out.status.success(), "no clipboard without a display");
    assert!(String::from_utf8_lossy(&out.stderr).contains("no clipboard available"));

    // export
    let out_dir = env.path("export/my-cats");
    let out = env.ok(&["export", "my-cats", out_dir.to_str().unwrap()]);
    assert!(out.contains("2 items"), "{out}");
    assert_eq!(std::fs::read(out_dir.join("001-grumpy-cat.png")).unwrap(), png_bytes);
    assert_eq!(std::fs::read(out_dir.join("002-naahh.gif")).unwrap(), gif_bytes);
    let manifest =
        stickpile_setfmt::Manifest::parse(&std::fs::read_to_string(out_dir.join("set.toml")).unwrap()).unwrap();
    assert_eq!(manifest.name, "My cats");

    // backup = every own set
    let backup = env.path("backup");
    let out = env.ok(&["backup", backup.to_str().unwrap()]);
    assert!(out.contains("Backed up 2 of 2 sets"), "{out}");
    assert!(backup.join("my-cats/set.toml").exists() && backup.join("inbox/set.toml").exists());

    // sync fills the offline cache; then the server goes away
    let s = env.json(&["sync"]);
    assert_eq!(s["total"], 3);
    server.stop();
    let out = env.run(&["search", "angry"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("grumpy cat"));
    assert!(String::from_utf8_lossy(&out.stderr).contains("offline cache"));
    // copy still works from the cached file; usage is queued
    let again = env.ok(&["copy", "naahh", "--print-path"]);
    assert_eq!(PathBuf::from(again.trim()), path);
    let out = env.run(&["flush"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("hint:"));

    // back online: queued event goes out
    server.restart();
    let f = env.json(&["flush"]);
    assert_eq!(f["accepted"], 1);
    assert_eq!(server.state().usage.len(), 2);

    // logout revokes the token and deletes the file
    let out = env.ok(&["logout"]);
    assert!(out.contains("Logged out"), "{out}");
    assert!(!token_file.exists());
    assert!(server.state().tokens.iter().all(|t| t.info.revoked));
    assert!(!env.run(&["sets", "list"]).status.success());
}

#[test]
fn tailnet_login_needs_no_password() {
    let server = MockServer::start();
    server.state().tailnet_user = Some("alice".into());
    let env = Env::new(&server.url);
    let out = env.ok(&["login", "--token-name", "tailnet-laptop"]);
    assert!(out.contains("as alice (tailnet identity)"), "{out}");
    assert!(env.path("state/token").exists());
    // From now on the stored token is used.
    server.state().tailnet_user = None;
    assert_eq!(env.json(&["whoami"])["username"], "alice");
    assert_eq!(server.state().tokens[0].info.name, "tailnet-laptop");
}

#[test]
fn server_override_and_admin_wrappers() {
    let server = MockServer::start();
    server.state().add_user("root", "pw", stickpile_api::Role::Admin);
    // The config points nowhere; --server wins.
    let env = Env::new("http://127.0.0.1:9");
    std::fs::write(env.path("pw"), "pw").unwrap();
    env.ok(&[
        "--server",
        &server.url,
        "login",
        "-u",
        "root",
        "--password-file",
        env.path("pw").to_str().unwrap(),
    ]);
    let users = env.json(&["--server", &server.url, "admin", "users"]);
    assert_eq!(users.as_array().unwrap().len(), 2);
    let out = env.ok(&["--server", &server.url, "admin", "invite", "--expires-days", "2"]);
    assert!(out.contains("http://mock/register"), "{out}");
    env.ok(&["--server", &server.url, "admin", "storage"]);
    env.ok(&["--server", &server.url, "admin", "audit"]);
    let out = env.ok(&["--server", &server.url, "tokens", "list"]);
    assert!(out.contains("this client"), "{out}");
    let job = env.json(&["--server", &server.url, "telegram", "import", "Animals", "--wait"]);
    assert_eq!(job["status"], "done");
    // Without --server the configured (dead) URL is used.
    let out = env.run(&["whoami"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("server unreachable"));
}

/// `stickpile copy` through the detached clipboard helper. Needs a display, so
/// it only runs when `STICKPILE_TEST_X11_DISPLAY` (e.g. `:99` from Xvfb) or
/// `STICKPILE_TEST_WAYLAND_DISPLAY` (+ `XDG_RUNTIME_DIR`, e.g. headless sway)
/// is set; with `xclip`/`wl-paste` on PATH the clipboard contents are checked.
#[test]
fn copy_to_a_real_clipboard_when_a_test_display_is_given() {
    let x11 = std::env::var("STICKPILE_TEST_X11_DISPLAY").ok();
    let wayland = std::env::var("STICKPILE_TEST_WAYLAND_DISPLAY").ok();
    if x11.is_none() && wayland.is_none() {
        eprintln!("skipped: set STICKPILE_TEST_X11_DISPLAY or STICKPILE_TEST_WAYLAND_DISPLAY");
        return;
    }
    let server = MockServer::start();
    let gif_bytes = {
        let mut st = server.state();
        let set = st.add_set("alice", "cats", "Cats", stickpile_api::Visibility::Private);
        st.add_item(&set.id, "naahh", &["nope"], gif());
        st.items[0].files[&stickpile_api::Variant::Original].clone()
    };
    let env = Env::new(&server.url);
    stickpile_core::token::write_token(&env.path("state/token"), &server.state().issue_token("alice", "t")).unwrap();

    let mut cmd = Command::new(bin());
    cmd.arg("--config")
        .arg(&env.config)
        .args(["copy", "naahh"])
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY");
    if let Some(d) = &wayland {
        cmd.env("WAYLAND_DISPLAY", d);
    } else if let Some(d) = &x11 {
        cmd.env("DISPLAY", d);
    }
    let started = std::time::Instant::now();
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "copy must return promptly"
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("Copied \"naahh\""));

    let paste = |mime: &str| -> Option<Vec<u8>> {
        let mut c = if let Some(d) = &wayland {
            let mut c = Command::new("wl-paste");
            c.args(["-n", "-t", mime]).env("WAYLAND_DISPLAY", d);
            c
        } else {
            let mut c = Command::new("xclip");
            c.args(["-selection", "clipboard", "-t", mime, "-o"])
                .env("DISPLAY", x11.as_deref().unwrap());
            c
        };
        c.output().ok().filter(|o| o.status.success()).map(|o| o.stdout)
    };
    match paste("image/gif") {
        Some(bytes) => {
            assert_eq!(bytes, gif_bytes);
            let uri = String::from_utf8(paste("text/uri-list").unwrap()).unwrap();
            assert!(uri.starts_with("file://") && uri.ends_with("naahh.gif\r\n"), "{uri:?}");
        }
        None => eprintln!("no xclip / wl-paste on PATH: clipboard contents not checked"),
    }
}
