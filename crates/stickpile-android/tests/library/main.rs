//! Host tests of [`Library`] against the in-process mock of the REST API
//! (`stickpile_core::mock`), through the same Rust API the bindings wrap.
//! No network beyond the mock, no display.

mod account;
mod files;
mod reading;
mod sync;
mod writing;

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use stickpile_android::{Library, LibraryDirs, LibraryError, LibraryErrorKind};
use stickpile_api::{ItemSummary, Visibility};
use stickpile_core::mock::MockServer;

pub fn png_bytes(w: u32, h: u32, seed: u8) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([x as u8 ^ seed, y as u8, seed, 255]));
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

pub fn webp_bytes(w: u32, h: u32, seed: u8) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([seed, x as u8, y as u8, 255]));
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::WebP).unwrap();
    out.into_inner()
}

pub fn gif_bytes(frames: usize, seed: u8) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = image::codecs::gif::GifEncoder::new(&mut out);
        enc.set_repeat(image::codecs::gif::Repeat::Infinite).unwrap();
        for i in 0..frames {
            let img = image::RgbaImage::from_pixel(12, 12, image::Rgba([seed, (i * 40) as u8, 0, 255]));
            enc.encode_frame(image::Frame::new(img)).unwrap();
        }
    }
    out
}

/// The library's files inside `dir`.
pub fn dirs(dir: &Path) -> LibraryDirs {
    LibraryDirs {
        config_file: dir.join("config.toml").display().to_string(),
        cache_dir: dir.join("cache").display().to_string(),
        token_file: token_path(dir).display().to_string(),
    }
}

pub fn token_path(dir: &Path) -> PathBuf {
    dir.join("state/token")
}

pub fn config_path(dir: &Path) -> PathBuf {
    dir.join("config.toml")
}

/// A config file for `url` with a short timeout.
pub fn write_config(dir: &Path, url: &str) {
    std::fs::write(
        config_path(dir),
        format!("# written by the test\nserver_url = {url:?}\ntimeout_secs = 2\n"),
    )
    .unwrap();
}

/// A library for the server at `url`, with whatever token `dir` holds.
pub fn open(dir: &Path, url: &str) -> Arc<Library> {
    write_config(dir, url);
    Library::new(dirs(dir)).unwrap()
}

/// Store a token of alice's in `dir`, as a login would.
pub fn store_token(server: &MockServer, dir: &Path) -> String {
    let token = server.state().issue_token("alice", "test");
    stickpile_core::token::write_token(&token_path(dir), &token).unwrap();
    token
}

/// Signed in as alice on `server`.
pub fn signed_in(server: &MockServer, dir: &Path) -> Arc<Library> {
    store_token(server, dir);
    open(dir, &server.url)
}

/// Mock with alice (`secret`), her public set `cats` of three items (a PNG,
/// an animated GIF, a WebP), her private set `memes` of one PNG, and bob
/// with a public set `dogs` of one PNG.
pub fn seeded_server() -> (MockServer, Vec<ItemSummary>) {
    let server = MockServer::start();
    let items = {
        let mut st = server.state();
        st.add_user("bob", "hunter2", stickpile_api::Role::User);
        let cats = st.add_set("alice", "cats", "Cats", Visibility::Public);
        let memes = st.add_set("alice", "memes", "Memes", Visibility::Private);
        let dogs = st.add_set("bob", "dogs", "Dogs", Visibility::Public);
        vec![
            st.add_item(&cats.id, "grumpy cat", &["cat", "angry"], png_bytes(20, 20, 1)),
            st.add_item(&cats.id, "naahh", &["nope", "cat"], gif_bytes(3, 2)),
            st.add_item(&cats.id, "sleepy", &["tired"], webp_bytes(16, 16, 3)),
            st.add_item(&memes.id, "this is fine", &["fire", "dog"], png_bytes(24, 18, 4)),
            st.add_item(&dogs.id, "good boy", &["dog"], png_bytes(10, 12, 5)),
        ]
    };
    (server, items)
}

/// The kind of an error, for `assert_eq!`.
pub fn kind<T: std::fmt::Debug>(r: Result<T, LibraryError>) -> LibraryErrorKind {
    r.unwrap_err().kind()
}

/// The `field` of an `InvalidInput` error.
pub fn invalid_field<T: std::fmt::Debug>(r: Result<T, LibraryError>) -> String {
    match r.unwrap_err() {
        LibraryError::InvalidInput { field, .. } => field,
        other => panic!("not InvalidInput: {other:?}"),
    }
}

/// Requests the mock received whose `METHOD /path` equals `what`.
pub fn requests(server: &MockServer, what: &str) -> usize {
    server.state().requests.iter().filter(|r| *r == what).count()
}

pub async fn wait_until(mut cond: impl FnMut() -> bool) -> bool {
    for _ in 0..300 {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

pub fn ids(items: &[stickpile_android::Item]) -> Vec<String> {
    items.iter().map(|i| i.id.clone()).collect()
}
