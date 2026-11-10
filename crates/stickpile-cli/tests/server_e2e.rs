//! The real `stickpile` binary against the real server, started in-process
//! on 127.0.0.1:0 with a temporary data directory. No display, no network,
//! temporary XDG directories.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use stickpile_api::Role;
use stickpile_server::app::{spawn_for_tests, TestServerHandle};
use stickpile_server::service::users::{self, NewUser};

const PASSWORD: &str = "correct horse battery staple";

struct Env {
    dir: tempfile::TempDir,
    config: PathBuf,
}

impl Env {
    fn new(server_url: &str) -> Env {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("xdg/config/stickpile/config.toml");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(
            &config,
            format!(
                "server_url = {server_url:?}\ntoken_file = {:?}\ncache_dir = {:?}\ntimeout_secs = 30\n",
                dir.path().join("xdg/state/stickpile/token"),
                dir.path().join("xdg/cache/stickpile"),
            ),
        )
        .unwrap();
        Env { dir, config }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    fn run(&self, args: &[&str]) -> Output {
        let x = |rel: &str| self.path(rel);
        Command::new(env!("CARGO_BIN_EXE_stickpile"))
            .arg("--config")
            .arg(&self.config)
            .args(args)
            .env("HOME", self.dir.path())
            .env("XDG_CONFIG_HOME", x("xdg/config"))
            .env("XDG_CACHE_HOME", x("xdg/cache"))
            .env("XDG_STATE_HOME", x("xdg/state"))
            .env("XDG_DATA_HOME", x("xdg/data"))
            .env("XDG_RUNTIME_DIR", x("xdg/run"))
            // No clipboard and no real display in tests.
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
        let out = self.ok(&a);
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"))
    }
}

fn png(seed: u8) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(24, 24, |x, y| image::Rgba([x as u8 * seed, y as u8 * 5, seed, 255]));
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

fn write_set_dir(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("001-grumpy cat.png"), png(3)).unwrap();
    std::fs::write(dir.join("002-naahh.gif"), gif()).unwrap();
    std::fs::write(
        dir.join("set.toml"),
        "version = 2\nname = \"My cats\"\nslug = \"my-cats\"\nvisibility = \"public\"\nlicense = \"CC0\"\n\n\
         [items.\"001-grumpy cat.png\"]\nname = \"grumpy cat\"\nkeywords = [\"cat\", \"angry\"]\nemojis = [\"😾\"]\n\n\
         [items.\"002-naahh.gif\"]\nkeywords = [\"nope\", \"cat\"]\nvibe = \"sarcastic\"\nvibe_source = \"ai\"\n",
    )
    .unwrap();
}

/// Media files of a set directory in order, decoded to RGBA frames
/// (stored originals are re-encoded losslessly, so bytes may differ).
fn pixels(dir: &Path) -> Vec<Vec<Vec<u8>>> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n != "set.toml")
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|n| {
            let bytes = std::fs::read(dir.join(&n)).unwrap();
            let frames = if n.ends_with(".gif") {
                use image::AnimationDecoder;
                image::codecs::gif::GifDecoder::new(Cursor::new(bytes))
                    .unwrap()
                    .into_frames()
                    .map(|f| f.unwrap().into_buffer().into_raw())
                    .collect()
            } else {
                vec![image::load_from_memory(&bytes).unwrap().to_rgba8().into_raw()]
            };
            frames
        })
        .collect()
}

fn manifest(dir: &Path) -> stickpile_setfmt::Manifest {
    stickpile_setfmt::Manifest::parse(&std::fs::read_to_string(dir.join("set.toml")).unwrap()).unwrap()
}

struct Server {
    rt: tokio::runtime::Runtime,
    handle: Option<TestServerHandle>,
    _data: tempfile::TempDir,
}

impl Server {
    fn start() -> Server {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let data = tempfile::tempdir().unwrap();
        let mut cfg = stickpile_server::Config::default();
        cfg.server.listen = "127.0.0.1:0".parse().unwrap();
        cfg.server.data_dir = data.path().join("data");
        cfg.server.secure_cookies = false;
        cfg.media.workers = 1;
        let handle = rt.block_on(spawn_for_tests(cfg)).unwrap();
        rt.block_on(users::create(
            &handle.state.db,
            NewUser {
                username: "alice".into(),
                display_name: None,
                password: Some(PASSWORD.into()),
                role: Role::User,
                tailscale_login: None,
            },
        ))
        .unwrap();
        Server {
            rt,
            handle: Some(handle),
            _data: data,
        }
    }

    fn url(&self) -> String {
        self.handle.as_ref().unwrap().url.clone()
    }

    fn scalar(&self, sql: &str) -> i64 {
        let db = self.handle.as_ref().unwrap().state.db.clone();
        self.rt
            .block_on(sqlx::query_scalar::<_, i64>(sql).fetch_one(&db))
            .unwrap()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        drop(self.handle.take());
    }
}

#[test]
fn cli_against_the_real_server() {
    let server = Server::start();
    let env = Env::new(&server.url());

    // Not logged in yet.
    let out = env.run(&["whoami"]);
    assert!(!out.status.success());

    // login (password from a file)
    std::fs::write(env.path("pw"), format!("{PASSWORD}\n")).unwrap();
    let pw = env.path("pw");
    let out = env.ok(&[
        "login",
        "--username",
        "alice",
        "--password-file",
        pw.to_str().unwrap(),
        "--token-name",
        "e2e",
    ]);
    assert!(out.contains("Logged in"), "{out}");
    let token_file = env.path("xdg/state/stickpile/token");
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&token_file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(env.json(&["whoami"])["username"], "alice");
    assert_eq!(
        server.scalar("SELECT COUNT(*) FROM api_tokens WHERE name = 'e2e' AND revoked_at IS NULL"),
        1
    );
    // A wrong password is refused.
    std::fs::write(env.path("badpw"), "nope nope nope\n").unwrap();
    let bad = env.path("badpw");
    assert!(!env
        .run(&["login", "-u", "alice", "--password-file", bad.to_str().unwrap()])
        .status
        .success());

    // sets create + upload
    env.ok(&["sets", "create", "inbox", "--name", "Inbox"]);
    std::fs::write(env.path("wave.png"), png(9)).unwrap();
    let out = env.ok(&["upload", "inbox", env.path("wave.png").to_str().unwrap()]);
    assert!(out.contains("wave"), "{out}");

    // import a set directory
    let src = env.path("src/my-cats");
    write_set_dir(&src);
    let out = env.ok(&["import", src.to_str().unwrap()]);
    assert!(out.contains("2 created"), "{out}");
    let sets = env.json(&["sets", "list", "--scope", "mine"]);
    let slugs: Vec<&str> = sets
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["slug"].as_str().unwrap())
        .collect();
    assert!(slugs.contains(&"my-cats") && slugs.contains(&"inbox"), "{slugs:?}");

    // search (JSON)
    let res = env.json(&["search", "cat"]);
    assert_eq!(res["offline"], false);
    let hits = res["hits"].as_array().unwrap();
    let names: Vec<&str> = hits.iter().map(|h| h["item"]["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"grumpy cat") && names.contains(&"naahh"), "{names:?}");
    let naahh_id = hits.iter().find(|h| h["item"]["name"] == "naahh").unwrap()["item"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // copy without a clipboard: print the materialised file; usage recorded
    let copied = PathBuf::from(env.ok(&["copy", "naahh", "--print-path"]).trim());
    assert!(copied.starts_with(env.path("xdg/cache/stickpile")), "{copied:?}");
    assert_eq!(copied.file_name().unwrap(), "naahh.gif");
    let copied_bytes = std::fs::read(&copied).unwrap();
    assert!(copied_bytes.starts_with(b"GIF8"));
    assert_eq!(
        server.scalar("SELECT COUNT(*) FROM usage_events"),
        1,
        "flushed before exit"
    );
    let f = env.json(&["flush"]);
    assert_eq!(f["accepted"].as_u64().unwrap_or(0), 0, "nothing left queued: {f}");

    // like
    let liked = env.json(&["like", &naahh_id]);
    assert_eq!(liked["liked"], true);
    assert_eq!(liked["likes"], 1);

    // popular: the copy and the like show up, as separate lists
    let pop = env.json(&["popular", "--window", "all"]);
    let first = |list: &str| pop[list][0]["item"]["name"].as_str().map(str::to_string);
    assert_eq!(first("most_copied").as_deref(), Some("naahh"), "{pop}");
    assert_eq!(pop["most_copied"][0]["value"], 1);
    assert_eq!(first("most_liked").as_deref(), Some("naahh"), "{pop}");

    // export and compare with the imported directory
    let out_dir = env.path("export/my-cats");
    let out = env.ok(&["export", "my-cats", out_dir.to_str().unwrap()]);
    assert!(out.contains("2 items"), "{out}");
    let mut files: Vec<String> = std::fs::read_dir(&out_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(files, ["001-grumpy-cat.png", "002-naahh.gif", "set.toml"]);
    assert!(pixels(&out_dir) == pixels(&src), "same items, order and pixels");
    let (mi, me) = (manifest(&src), manifest(&out_dir));
    assert_eq!((me.name.as_str(), me.slug.as_deref()), ("My cats", Some("my-cats")));
    assert_eq!(me.visibility, mi.visibility);
    assert_eq!(me.extra, mi.extra, "unknown keys survive");
    assert_eq!(me.version, mi.version);
    let items_in: Vec<_> = mi.items.values().cloned().collect();
    let items_out: Vec<_> = me.items.values().cloned().collect();
    let strip = |mut v: Vec<stickpile_setfmt::ItemMeta>| {
        // The import names the unnamed item after its file.
        for m in &mut v {
            if m.name.is_empty() {
                m.name = "naahh".into();
            }
        }
        v
    };
    assert_eq!(strip(items_out), strip(items_in));
    assert_eq!(std::fs::read(out_dir.join("002-naahh.gif")).unwrap(), copied_bytes);

    // Re-importing the export into the same set changes nothing, and a
    // second export is byte-identical.
    let out = env.ok(&["import", out_dir.to_str().unwrap(), "--into", "my-cats"]);
    assert!(out.contains("0 created, 0 updated, 2 unchanged, 0 removed"), "{out}");
    let again = env.path("export/again");
    env.ok(&["export", "my-cats", again.to_str().unwrap()]);
    for name in ["set.toml", "001-grumpy-cat.png", "002-naahh.gif"] {
        assert_eq!(
            std::fs::read(again.join(name)).unwrap(),
            std::fs::read(out_dir.join(name)).unwrap(),
            "{name}"
        );
    }

    // logout revokes the token and deletes the file
    let out = env.ok(&["logout"]);
    assert!(out.contains("Logged out"), "{out}");
    assert!(!token_file.exists());
    assert_eq!(
        server.scalar("SELECT COUNT(*) FROM api_tokens WHERE name = 'e2e' AND revoked_at IS NULL"),
        0
    );
    assert!(!env.run(&["sets", "list"]).status.success());
}
