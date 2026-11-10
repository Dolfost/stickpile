//! Integration-style tests against the in-process mock server
//! ([`crate::mock::MockServer`]).

mod cache_sync;
mod client;
mod session;
mod usage;

use std::io::Cursor;
use std::path::Path;
use std::time::Duration;

use stickpile_api::{ItemSummary, Visibility};

use crate::mock::MockServer;
use crate::{token, ApiClient, ClientConfig, Session};

pub(crate) fn png_bytes(w: u32, h: u32, seed: u8) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([x as u8 ^ seed, y as u8, seed, 255]));
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

pub(crate) fn webp_bytes(w: u32, h: u32, seed: u8) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([seed, x as u8, y as u8, 255]));
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::WebP).unwrap();
    out.into_inner()
}

pub(crate) fn gif_bytes(frames: usize, seed: u8) -> Vec<u8> {
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

pub(crate) fn mp4_bytes(len: usize) -> Vec<u8> {
    let mut v = b"\0\0\0\x18ftypmp42\0\0\0\0mp42isom".to_vec();
    v.resize(len.max(v.len()), 7);
    v
}

pub(crate) fn config_for(url: &str, dir: &Path) -> ClientConfig {
    ClientConfig {
        server_url: url.to_string(),
        token_file: Some(dir.join("state/token")),
        cache_dir: Some(dir.join("cache")),
        timeout_secs: 2,
        ..ClientConfig::default()
    }
}

/// Mock with alice + a public set of three items and a private set of one.
pub(crate) fn seeded_server() -> (MockServer, Vec<ItemSummary>) {
    let server = MockServer::start();
    let items = {
        let mut st = server.state();
        let cats = st.add_set("alice", "cats", "Cats", Visibility::Public);
        let memes = st.add_set("alice", "memes", "Memes", Visibility::Private);
        vec![
            st.add_item(&cats.id, "grumpy cat", &["cat", "angry"], png_bytes(20, 20, 1)),
            st.add_item(&cats.id, "naahh", &["nope", "cat"], gif_bytes(3, 2)),
            st.add_item(&cats.id, "sleepy", &["tired"], webp_bytes(16, 16, 3)),
            st.add_item(&memes.id, "this is fine", &["fire", "dog"], png_bytes(24, 18, 4)),
        ]
    };
    (server, items)
}

pub(crate) fn login_token(server: &MockServer, dir: &Path) -> String {
    let token = server.state().issue_token("alice", "test");
    token::write_token(&dir.join("state/token"), &token).unwrap();
    token
}

pub(crate) async fn session_for(server: &MockServer, dir: &Path) -> Session {
    login_token(server, dir);
    Session::open(config_for(&server.url, dir)).await.unwrap()
}

pub(crate) fn client_for(url: &str, token: Option<String>) -> ApiClient {
    ApiClient::new(url, Duration::from_secs(2)).unwrap().with_token(token)
}

#[test]
fn session_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Session>();
    assert_send_sync::<crate::cache::Cache>();
    assert_send_sync::<ApiClient>();
    assert_send_sync::<crate::usage::UsageQueue>();
}
