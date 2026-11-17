//! `stickpile offline ...`, `cache clear` and `copy` with the server
//! stopped: the real binary against the in-process mock server from
//! `stickpile-core` (feature `mock`), with a temporary config directory.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use stickpile_api::{ItemSummary, Visibility};
use stickpile_core::mock::MockServer;

/// Without the automatic thumbnails, a scope's estimate is every file
/// wanted.
const NO_AUTO_THUMBNAILS: &str = "[offline.auto]\nthumbnails = false\n";

struct Env {
    dir: tempfile::TempDir,
    config: PathBuf,
}

impl Env {
    fn new(server: &MockServer, extra: &str) -> Env {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config/config.toml");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(
            &config,
            format!(
                "server_url = {:?}\ntoken_file = {:?}\ncache_dir = {:?}\ntimeout_secs = 2\n{extra}",
                server.url,
                dir.path().join("state/token"),
                dir.path().join("cache"),
            ),
        )
        .unwrap();
        let token = server.state().issue_token("alice", "offline-e2e");
        stickpile_core::token::write_token(&dir.path().join("state/token"), &token).unwrap();
        Env { dir, config }
    }

    fn cache(&self) -> PathBuf {
        self.dir.path().join("cache")
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stickpile"))
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
        let out = self.ok(&a);
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"))
    }

    /// The cached files (not the database), with their sizes.
    fn files(&self) -> BTreeMap<String, u64> {
        fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, u64>) {
            let Ok(entries) = std::fs::read_dir(dir) else { return };
            for e in entries {
                let path = e.unwrap().path();
                if path.is_dir() {
                    walk(root, &path, out);
                } else {
                    let rel = path.strip_prefix(root).unwrap().to_string_lossy().into_owned();
                    if !rel.starts_with("cache.sqlite3") {
                        out.insert(rel, std::fs::metadata(&path).unwrap().len());
                    }
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&self.cache(), &self.cache(), &mut out);
        out
    }
}

fn code(out: &Output) -> Option<i32> {
    out.status.code()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
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

/// A file the mock takes for an MP4 video.
fn mp4(len: usize) -> Vec<u8> {
    let mut v = b"\0\0\0\x18ftypmp42\0\0\0\0mp42isom".to_vec();
    v.resize(len.max(v.len()), 7);
    v
}

/// alice's public pack `cats` (a still, a GIF, a video) and her private
/// pack `memes` (a still).
fn library(server: &MockServer) -> (String, [ItemSummary; 4]) {
    let mut st = server.state();
    let cats = st.add_set("alice", "cats", "Cats", Visibility::Public);
    let memes = st.add_set("alice", "memes", "Memes", Visibility::Private);
    let grumpy = st.add_item(&cats.id, "grumpy cat", &["cat"], png(3));
    let naahh = st.add_item(&cats.id, "naahh", &["nope"], gif());
    let clip = st.add_item(&cats.id, "clip", &["video"], mp4(4000));
    let fine = st.add_item(&memes.id, "this is fine", &["fire"], png(9));
    (cats.id, [grumpy, naahh, clip, fine])
}

#[test]
fn keep_a_pack_sync_copy_offline_release_clear_and_remove_all() {
    let mut server = MockServer::start();
    let (cats, [grumpy, _naahh, clip, fine]) = library(&server);
    let env = Env::new(&server, NO_AUTO_THUMBNAILS);
    env.ok(&["sync"]);
    let nothing = env.json(&["offline", "status"]);
    assert_eq!(nothing["keeps"], serde_json::json!([]));
    assert!(env.ok(&["offline", "status"]).contains("Nothing is kept offline"));

    // The estimate, keeping nothing: two items, the video skipped.
    let dry = env.json(&["offline", "status", "--dry-run", "--set", "cats"]);
    let est = &dry["estimate"];
    assert_eq!(dry["scope"]["id"], cats.as_str());
    assert_eq!(dry["scope"]["name"], "Cats");
    assert_eq!(est["items"], 2, "{dry}");
    assert_eq!(est["totals"]["present_files"], 0);
    let est_files = est["totals"]["files"].as_u64().unwrap();
    let est_bytes = est["totals"]["bytes"].as_u64().unwrap();
    assert!(est_files >= 4, "a thumbnail and a rendition each: {dry}");
    let skipped = est["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 1, "{dry}");
    assert_eq!(skipped[0]["item_id"], clip.id.as_str());
    assert_eq!(skipped[0]["reason"]["reason"], "kind_off");
    assert_eq!(dry["fits"], true);
    let human = env.ok(&["offline", "status", "--dry-run", "--set", "cats"]);
    assert!(human.contains("Keeping pack \"Cats\" (cats) offline takes:"), "{human}");
    assert!(human.contains("skipped:      1 item: videos are not kept"), "{human}");
    assert!(env.json(&["offline", "status"])["keeps"].as_array().unwrap().is_empty());

    // Not on a terminal and without --yes: the estimate, then a refusal.
    let out = env.run(&["offline", "keep", "--set", "cats"]);
    assert_eq!(code(&out), Some(1));
    assert!(stdout(&out).contains("Keeping pack \"Cats\" (cats) offline takes:"));
    assert!(
        stderr(&out).contains("refusing to keep pack \"Cats\" (cats) without confirmation; pass --yes"),
        "{}",
        stderr(&out)
    );
    assert!(env.json(&["offline", "status"])["keeps"].as_array().unwrap().is_empty());

    let out = env.ok(&["offline", "keep", "--set", "cats", "--yes"]);
    assert!(out.contains("Kept pack \"Cats\" (cats) offline:"), "{out}");
    assert!(
        out.contains("0 of them on this device; `stickpile offline sync` downloads the rest"),
        "{out}"
    );
    assert!(env
        .ok(&["offline", "keep", "--set", &cats])
        .contains("Already kept offline"));

    // The downloader to the end: exit 0, the estimated files arrive.
    let out = env.run(&["--json", "offline", "sync"]);
    assert_eq!(code(&out), Some(0), "{}", stderr(&out));
    let run: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(run["end"], "complete");
    assert_eq!(run["complete"], true);
    assert_eq!(run["downloaded_files"], est_files);
    assert_eq!(run["status"]["files_done"], est_files);
    let kept_files = env.files();
    assert_eq!(kept_files.len() as u64, est_files, "{kept_files:?}");
    assert_eq!(kept_files.values().sum::<u64>(), est_bytes, "the estimate's bytes");
    assert!(kept_files.keys().all(|p| !p.contains(".part")));

    // Status lists the scope and the sizes.
    let st = env.json(&["offline", "status"]);
    assert_eq!(st["keeps"][0]["scope"], "set");
    assert_eq!(st["keeps"][0]["id"], cats.as_str());
    assert_eq!(st["keeps"][0]["name"], "Cats");
    assert_eq!(st["keeps"][0]["slug"], "cats");
    assert_eq!(st["usage"]["kept_files"], est_files);
    assert_eq!(st["usage"]["kept_bytes"], est_bytes);
    assert_eq!(st["usage"]["budget"], 1024 * 1024 * 1024u64);
    assert_eq!(st["complete"], true);
    assert_eq!(st["missing_files"], 0);
    assert_eq!(st["scopes"][0]["scope"]["kept"]["id"], cats.as_str());
    assert_eq!(st["scopes"][0]["items"], 2);
    assert_eq!(st["scopes"][0]["totals"]["present_files"], est_files);
    assert_eq!(st["skipped"][0]["item_id"], clip.id.as_str());
    let human = env.ok(&["offline", "status"]);
    assert!(human.contains("pack") && human.contains("Cats (cats)"), "{human}");
    assert!(human.contains(&format!("Kept files:   {est_files},")), "{human}");
    assert!(human.contains("Skipped (1):"), "{human}");

    // A copy of an item that is not kept is an ordinary cache file:
    // "Clear cache" takes it and leaves the kept ones.
    let copied = PathBuf::from(env.ok(&["copy", "--id", &fine.id, "--print-path"]).trim());
    assert!(copied.exists());
    let cleared = env.json(&["cache", "clear"]);
    assert!(cleared["removed_files"].as_u64().unwrap() >= 1, "{cleared}");
    assert_eq!(cleared["kept_files"], est_files);
    assert_eq!(cleared["kept_bytes"], est_bytes);
    assert!(!copied.exists());
    assert_eq!(env.files(), kept_files, "the kept files stay");
    let out = env.ok(&["cache", "clear"]);
    assert!(
        out.contains(&format!("{est_files} files")) && out.contains("kept offline stay"),
        "{out}"
    );

    // With the server stopped: a kept sticker copies, an unkept one says
    // why it cannot.
    server.stop();
    let path = PathBuf::from(env.ok(&["copy", "--id", &grumpy.id, "--print-path"]).trim());
    assert!(path.starts_with(env.cache()) && path.exists(), "{path:?}");
    let by_query = PathBuf::from(env.ok(&["copy", "naahh", "--print-path"]).trim());
    assert!(by_query.exists());
    let out = env.run(&["copy", "--id", &fine.id, "--print-path"]);
    assert_eq!(code(&out), Some(1));
    let err = stderr(&out);
    assert!(
        err.contains("error: this is fine: the original file is not on this device"),
        "{err}"
    );
    assert!(
        err.contains(&format!(
            "hint: this sticker is not on this device; keep it offline with `stickpile offline keep {}` while online",
            fine.id
        )),
        "{err}"
    );
    // offline sync cannot sync: incomplete, and says why.
    let out = env.run(&["offline", "sync"]);
    assert_eq!(code(&out), Some(2), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("incomplete: server unreachable: "), "{err}");
    assert!(
        err.contains("; everything kept is on this device (as of the last sync)"),
        "{err}"
    );
    assert!(err.contains("hint: run `stickpile offline sync` again"), "{err}");
    assert!(
        stdout(&out).contains(&format!("{est_files} of {est_files} kept files are on this device")),
        "{}",
        stdout(&out)
    );
    assert_eq!(
        env.json(&["offline", "status"])["complete"],
        true,
        "status works offline"
    );

    // Released: no longer kept, the files stay until evicted.
    server.restart();
    let released = env.json(&["offline", "release", "--set", "cats"]);
    assert_eq!(released["was_kept"], true);
    assert_eq!(released["usage"]["kept_files"], 0);
    assert_eq!(env.files(), kept_files, "release deletes nothing");
    assert!(env.json(&["offline", "status"])["keeps"].as_array().unwrap().is_empty());
    assert!(env
        .ok(&["offline", "release", "--set", "cats"])
        .contains("was not kept offline"));

    // A sticker by itself: no question asked.
    let out = env.ok(&["offline", "keep", &grumpy.id]);
    assert!(out.contains("Kept \"grumpy cat\""), "{out}");
    assert_eq!(code(&env.run(&["offline", "sync"])), Some(0));
    let kept = env.json(&["offline", "status"])["usage"]["kept_files"]
        .as_u64()
        .unwrap();
    assert!(kept >= 2, "its thumbnail and its file");

    // remove-all asks; with --yes it deletes the kept files and forgets.
    let out = env.run(&["offline", "remove-all"]);
    assert_eq!(code(&out), Some(1));
    assert!(stderr(&out).contains("pass --yes"), "{}", stderr(&out));
    assert_eq!(env.json(&["offline", "status"])["usage"]["kept_files"], kept);
    let removed = env.json(&["offline", "remove-all", "--yes"]);
    assert_eq!(removed["removed_files"], kept);
    assert_eq!(removed["forgotten"], 1);
    assert!(!path.exists(), "the kept copy is gone");
    let st = env.json(&["offline", "status"]);
    assert!(st["keeps"].as_array().unwrap().is_empty());
    assert_eq!(st["usage"]["kept_files"], 0);
    assert!(env.ok(&["offline", "remove-all"]).contains("Nothing is kept offline"));
}

/// Ctrl-C (SIGINT) during `offline sync` cancels the run: it ends at once,
/// exits 2, and leaves no part file behind.
#[test]
fn ctrl_c_cancels_the_download_cleanly() {
    use std::io::BufRead;
    use std::time::{Duration, Instant};

    let server = MockServer::start();
    let slow = {
        let mut st = server.state();
        let set = st.add_set("alice", "clips", "Clips", Visibility::Private);
        let item = st.add_item(&set.id, "slow clip", &[], mp4(64 * 1024));
        // 64 KiB at 1 KiB per 100 ms: over six seconds.
        st.slow_files.insert(
            (item.id.clone(), stickpile_api::Variant::Original),
            Duration::from_millis(100),
        );
        item
    };
    let env = Env::new(&server, NO_AUTO_THUMBNAILS);
    env.ok(&["sync"]);
    env.ok(&["offline", "keep", &slow.id]);

    let mut child = Command::new(env!("CARGO_BIN_EXE_stickpile"))
        .arg("--config")
        .arg(&env.config)
        .args(["offline", "sync"])
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let err = child.stderr.take().unwrap();
    let reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(err).lines() {
            let line = line.unwrap();
            let _ = tx.send(line.clone());
        }
    });
    let mut seen = Vec::new();
    loop {
        let line = rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|_| panic!("no progress: {seen:?}"));
        let downloading = line.contains("offline: downloading");
        seen.push(line);
        if downloading {
            break;
        }
    }
    // The download is under way (its part file exists), then Ctrl-C.
    let started = Instant::now();
    while !env.files().keys().any(|p| p.contains(".part")) {
        assert!(started.elapsed() < Duration::from_secs(5), "no part file");
        std::thread::sleep(Duration::from_millis(20));
    }
    let interrupted = Instant::now();
    let kill = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(kill.success());
    let out = child.wait_with_output().unwrap();
    reader.join().unwrap();
    seen.extend(rx.try_iter());
    let err = seen.join("\n");
    assert!(
        interrupted.elapsed() < Duration::from_secs(3),
        "cancelled at once: {:?}",
        interrupted.elapsed()
    );
    assert_eq!(code(&out), Some(2), "{err}");
    assert!(err.contains("Cancelling (Ctrl-C again stops at once)"), "{err}");
    assert!(
        err.contains("incomplete: cancelled; 1 file (64.0 KiB) not on this device"),
        "{err}"
    );
    // Its thumbnail came first; the clip itself did not arrive.
    assert!(
        stdout(&out).contains("1 of 2 kept files are on this device"),
        "{}",
        stdout(&out)
    );
    let files = env.files();
    assert!(
        files.keys().all(|p| p.starts_with("files/thumb/")),
        "no part file left: {files:?}"
    );
}

#[test]
fn the_budget_reached_exits_2_and_keep_all_needs_yes_when_not_interactive() {
    let server = MockServer::start();
    {
        let mut st = server.state();
        let set = st.add_set("alice", "big", "Big", Visibility::Private);
        for i in 0..3 {
            st.add_item(&set.id, &format!("big {i}"), &[], mp4(600 * 1024 + i));
        }
    }
    let env = Env::new(
        &server,
        "[offline]\nmax_mb = 1\nvideos = true\n[offline.auto]\nthumbnails = false\n",
    );
    env.ok(&["sync"]);

    let out = env.run(&["offline", "keep", "--all"]);
    assert_eq!(code(&out), Some(1));
    assert!(
        stderr(&out).contains("refusing to keep the library without confirmation; pass --yes"),
        "{}",
        stderr(&out)
    );
    assert!(
        stdout(&out).contains("over the 1.0 MiB budget (offline.max_mb)"),
        "{}",
        stdout(&out)
    );
    assert!(env.json(&["offline", "status"])["keeps"].as_array().unwrap().is_empty());

    let out = env.run(&["offline", "keep", "--all", "--yes"]);
    assert_eq!(code(&out), Some(0), "{}", stderr(&out));
    assert!(stdout(&out).contains("Kept the library offline:"), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("warning: kept files would take"),
        "{}",
        stderr(&out)
    );
    let st = env.json(&["offline", "status"]);
    assert_eq!(st["keeps"][0]["scope"], "library");
    assert_eq!(st["over_budget"], true);

    let out = env.run(&["offline", "sync"]);
    assert_eq!(code(&out), Some(2), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.contains("incomplete: the offline budget is reached: kept files take"),
        "{err}"
    );
    assert!(err.contains("offline.max_mb is 1.0 MiB"), "{err}");
    assert!(
        err.contains("kept files take 600.1 KiB and offline.max_mb is 1.0 MiB"),
        "{err}"
    );
    // Two originals and a thumbnail or two.
    assert!(
        err.contains(" files (1.2 MiB) not on this device; nothing was deleted"),
        "{err}"
    );
    assert!(err.contains("offline: budget reached: "), "the progress: {err}");
    assert!(err.contains("hint: raise offline.max_mb"), "{err}");
    let originals = |env: &Env| env.files().keys().filter(|p| p.contains(".original/")).count();
    assert_eq!(originals(&env), 1, "one 600 KiB file fits in 1 MiB");

    // Again, with --json: the same end, nothing deleted, nothing more fetched.
    let out = env.run(&["--json", "offline", "sync"]);
    assert_eq!(code(&out), Some(2));
    let run: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(run["end"], "limit_reached");
    assert_eq!(run["complete"], false);
    assert_eq!(run["downloaded_files"], 0);
    assert_eq!(run["status"]["state"], "limit_reached");
    assert_eq!(originals(&env), 1);
}

fn webp(seed: u8) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(16, 16, |x, y| image::Rgba([seed, x as u8, y as u8, 255]));
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::WebP).unwrap();
    out.into_inner()
}

/// A file the mock takes for a WebM video.
fn webm(len: usize) -> Vec<u8> {
    let mut v = vec![0x1A, 0x45, 0xDF, 0xA3];
    v.resize(len.max(v.len()), 9);
    v
}

/// A sticker kept by itself, with the server stopped, copied to a chat app
/// or in a mode whose rendition is not kept (the default target is
/// Generic and nothing was copied elsewhere, so only the originals are
/// kept): a still copies the kept file the app takes too, with a warning; a
/// WebM video for WhatsApp (which takes MP4 only) cannot, and the hint says
/// what keeps that file instead of telling to keep a kept sticker. A kept
/// sticker not downloaded yet says so. (Ported from the behaviour review,
/// branch `review-offline-behaviour`.)
#[test]
fn a_kept_sticker_is_not_told_to_be_kept() {
    let mut server = MockServer::start();
    let (still, video, later) = {
        let mut st = server.state();
        let set = st.add_set("alice", "mixed", "Mixed", Visibility::Public);
        let still = st.add_item(&set.id, "still png", &["still"], png(1));
        st.add_variant(&still.id, stickpile_api::Variant::TgSticker, "image/webp", webp(2));
        let video = st.add_item(&set.id, "webm video", &["video"], webm(3000));
        st.add_variant(&video.id, stickpile_api::Variant::Mp4, "video/mp4", mp4(2500));
        let later = st.add_item(&set.id, "later", &["later"], png(5));
        (still, video, later)
    };
    let env = Env::new(&server, NO_AUTO_THUMBNAILS);
    env.ok(&["sync"]);
    env.ok(&["offline", "keep", &still.id]);
    env.ok(&["offline", "keep", &video.id]);
    let out = env.run(&["offline", "sync"]);
    assert_eq!(code(&out), Some(0), "{}", stderr(&out));
    // Kept now, downloaded never.
    env.ok(&["offline", "keep", &later.id]);
    server.stop();

    let plain = env.run(&["copy", "--id", &still.id, "--print-path"]);
    assert_eq!(
        code(&plain),
        Some(0),
        "the plain copy works offline: {}",
        stderr(&plain)
    );

    // Telegram as a sticker: the sticker WebP is not kept, the original
    // PNG (which Telegram takes) goes instead.
    let out = env.run(&[
        "copy",
        "--id",
        &still.id,
        "--target",
        "telegram",
        "--sticker",
        "--print-path",
    ]);
    let err = stderr(&out);
    println!("{err}");
    assert_eq!(code(&out), Some(0), "{err}");
    assert!(stdout(&out).trim().ends_with("still png.png"), "{}", stdout(&out));
    assert!(
        err.contains("warning: The file for this app is not on this device; another one it takes was copied"),
        "{err}"
    );

    // WhatsApp takes no WebM: nothing on the device fits.
    let out = env.run(&["copy", "--id", &video.id, "--target", "whatsapp", "--print-path"]);
    let err = stderr(&out);
    println!("{err}");
    assert_eq!(code(&out), Some(1));
    assert!(
        env.ok(&["offline", "keep", &video.id]).contains("Already kept offline"),
        "kept by itself"
    );
    assert!(
        !err.contains("keep it offline"),
        "the CLI tells to keep a sticker that is kept: {err}"
    );
    assert!(
        err.contains(
            "hint: this sticker is kept, but not the mp4 file for whatsapp: list `whatsapp` in `offline.targets`, \
             then run `stickpile offline sync` while online"
        ),
        "{err}"
    );

    let out = env.run(&["copy", "--id", &later.id, "--print-path"]);
    let err = stderr(&out);
    assert_eq!(code(&out), Some(1));
    assert!(
        err.contains(
            "hint: this sticker is kept, but the original file for generic is not on this device yet; \
             run `stickpile offline sync` while online"
        ),
        "{err}"
    );
}
