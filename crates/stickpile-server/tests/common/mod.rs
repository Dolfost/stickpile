//! Test harness: the real router on 127.0.0.1:0 plus the trusted unix socket,
//! a temp data dir and config, helpers for users/tokens/sets/uploads, and
//! generated media fixtures.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;

use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::Value;
use stickpile_api::{routes, Item, Role, Set, UploadResult, Visibility};
use stickpile_server::{AppState, Config, EnricherSource, StartOptions};
use tokio_util::sync::CancellationToken;

pub struct TestServer {
    pub state: AppState,
    pub base: String,
    pub socket: PathBuf,
    pub dir: tempfile::TempDir,
    pub client: reqwest::Client,
    cancel: CancellationToken,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[derive(Default)]
pub struct Opts {
    /// Use `MockEnricher` as the AI backend.
    pub ai: bool,
    /// Load AI as configured (`ai.enabled`, `ai.model_dir`), like production.
    pub ai_from_config: bool,
    /// Spawn the job workers (otherwise tests drain the queue with `run_jobs`).
    pub workers: bool,
}

pub struct TestUser {
    pub id: String,
    pub username: String,
    pub password: String,
    pub token: String,
}

/// A response with the body already read.
pub struct Resp {
    pub status: StatusCode,
    pub headers: reqwest::header::HeaderMap,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn json<T: DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!(
                "bad JSON ({e}) status {}: {}",
                self.status,
                String::from_utf8_lossy(&self.body)
            )
        })
    }
    pub fn value(&self) -> Value {
        self.json()
    }
    pub fn code(&self) -> String {
        self.value()["code"].as_str().unwrap_or_default().to_string()
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    pub fn header(&self, name: &str) -> Option<String> {
        self.headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string)
    }
    #[track_caller]
    pub fn expect(self, status: u16) -> Resp {
        assert_eq!(self.status.as_u16(), status, "unexpected status; body: {}", self.text());
        self
    }
}

pub async fn read(resp: reqwest::Response) -> Resp {
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = resp.bytes().await.expect("body").to_vec();
    Resp { status, headers, body }
}

impl TestServer {
    pub async fn start() -> TestServer {
        Self::start_with(Opts::default(), |_| {}).await
    }

    pub async fn with_config(f: impl FnOnce(&mut Config)) -> TestServer {
        Self::start_with(Opts::default(), f).await
    }

    pub async fn start_with(opts: Opts, f: impl FnOnce(&mut Config)) -> TestServer {
        let dir = tempfile::tempdir().expect("tempdir");
        let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = tcp.local_addr().unwrap();
        let socket = dir.path().join("tailscale.sock");
        let mut cfg = Config::default();
        cfg.server.listen = addr;
        cfg.server.public_url = format!("http://{addr}");
        cfg.server.data_dir = dir.path().join("data");
        cfg.server.secure_cookies = false;
        cfg.trusted_proxy.socket = Some(socket.clone());
        cfg.auth.tailscale_autocreate = false;
        cfg.media.workers = 1;
        f(&mut cfg);
        cfg.validate().expect("valid test config");
        let enricher = if opts.ai {
            EnricherSource::Provided(Arc::new(stickpile_ai::MockEnricher::default()))
        } else if opts.ai_from_config {
            EnricherSource::FromConfig
        } else {
            EnricherSource::Disabled
        };
        let state = stickpile_server::build_state(cfg.clone(), StartOptions { enricher })
            .await
            .expect("start");
        let cancel = CancellationToken::new();
        tokio::spawn(stickpile_server::serve_tcp(state.clone(), tcp, cancel.clone()));
        let unix = stickpile_server::bind_unix(&socket, &cfg.trusted_proxy.socket_mode).expect("bind unix");
        tokio::spawn(stickpile_server::serve_unix(state.clone(), unix, cancel.clone()));
        if opts.workers {
            stickpile_server::spawn_background(&state, &cancel);
        }
        TestServer {
            state,
            base: format!("http://{addr}"),
            socket,
            dir,
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            cancel,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    /// Create a user directly in the database plus an API token.
    pub async fn user(&self, username: &str) -> TestUser {
        self.user_with_role(username, Role::User).await
    }

    pub async fn admin(&self, username: &str) -> TestUser {
        self.user_with_role(username, Role::Admin).await
    }

    pub async fn user_with_role(&self, username: &str, role: Role) -> TestUser {
        let password = format!("password-{username}");
        let row = stickpile_server::service::users::create(
            &self.state.db,
            stickpile_server::service::users::NewUser {
                username: username.into(),
                display_name: None,
                password: Some(password.clone()),
                role,
                tailscale_login: None,
            },
        )
        .await
        .expect("create user");
        let tok = stickpile_server::auth::tokens::create(&self.state.db, &row, "test", None)
            .await
            .expect("token");
        TestUser {
            id: row.id,
            username: username.into(),
            password,
            token: tok.token,
        }
    }

    pub async fn send(&self, user: Option<&TestUser>, method: Method, path: &str, body: Option<Value>) -> Resp {
        let mut rb = self.client.request(method, self.url(path));
        if let Some(u) = user {
            rb = rb.bearer_auth(&u.token);
        }
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        read(rb.send().await.expect("request")).await
    }

    pub async fn get(&self, user: &TestUser, path: &str) -> Resp {
        self.send(Some(user), Method::GET, path, None).await
    }
    pub async fn anon_get(&self, path: &str) -> Resp {
        self.send(None, Method::GET, path, None).await
    }
    pub async fn post(&self, user: &TestUser, path: &str, body: Value) -> Resp {
        self.send(Some(user), Method::POST, path, Some(body)).await
    }
    pub async fn put(&self, user: &TestUser, path: &str, body: Value) -> Resp {
        self.send(Some(user), Method::PUT, path, Some(body)).await
    }
    pub async fn patch(&self, user: &TestUser, path: &str, body: Value) -> Resp {
        self.send(Some(user), Method::PATCH, path, Some(body)).await
    }
    pub async fn delete(&self, user: &TestUser, path: &str) -> Resp {
        self.send(Some(user), Method::DELETE, path, None).await
    }

    pub async fn create_set(&self, user: &TestUser, slug: &str, vis: Visibility) -> Set {
        self.post(
            user,
            routes::SETS,
            serde_json::json!({"slug": slug, "name": slug, "visibility": vis}),
        )
        .await
        .expect(201)
        .json()
    }

    /// Multipart upload of (filename, bytes) files.
    pub async fn upload(
        &self,
        user: &TestUser,
        set_id: &str,
        files: Vec<(&str, Vec<u8>)>,
        meta: Option<Value>,
    ) -> Resp {
        let mut form = reqwest::multipart::Form::new();
        for (name, bytes) in files {
            form = form.part(
                "file",
                reqwest::multipart::Part::bytes(bytes).file_name(name.to_string()),
            );
        }
        if let Some(m) = meta {
            form = form.text("meta", m.to_string());
        }
        let rb = self
            .client
            .post(self.url(&routes::path(routes::SET_ITEMS, &[set_id])))
            .bearer_auth(&user.token);
        read(rb.multipart(form).send().await.expect("upload")).await
    }

    /// PUT a set's cover image (multipart, one `file` part).
    pub async fn put_cover(&self, user: &TestUser, set_id: &str, name: &str, bytes: Vec<u8>) -> Resp {
        let form = reqwest::multipart::Form::new().part(
            "file",
            reqwest::multipart::Part::bytes(bytes).file_name(name.to_string()),
        );
        let rb = self
            .client
            .put(self.url(&routes::path(routes::SET_COVER, &[set_id])))
            .bearer_auth(&user.token);
        read(rb.multipart(form).send().await.expect("cover upload")).await
    }

    /// Upload one file and return the created (or existing) item.
    pub async fn upload_one(&self, user: &TestUser, set_id: &str, name: &str, bytes: Vec<u8>) -> Item {
        let res: Vec<UploadResult> = self
            .upload(user, set_id, vec![(name, bytes)], None)
            .await
            .expect(200)
            .json();
        res.into_iter().next().unwrap().item.expect("item")
    }

    /// Run every queued job now.
    pub async fn run_jobs(&self) -> usize {
        stickpile_server::jobs::run_pending(&self.state)
            .await
            .expect("run jobs")
    }

    /// Raw HTTP/1.1 request over the trusted unix socket.
    pub async fn unix(&self, method: &str, path: &str, headers: &[(&str, &str)], body: Option<&str>) -> Resp {
        unix_request(&self.socket, method, path, headers, body).await
    }
}

pub async fn unix_request(
    socket: &std::path::Path,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> Resp {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::UnixStream::connect(socket)
        .await
        .expect("connect unix socket");
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: stickpile.test\r\nConnection: close\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if let Some(b) = body {
        req.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            b.len()
        ));
    }
    req.push_str("\r\n");
    if let Some(b) = body {
        req.push_str(b);
    }
    s.write_all(req.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).await.unwrap();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").expect("header end");
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let mut body = raw[split + 4..].to_vec();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap();
    let code: u16 = status_line.split_whitespace().nth(1).unwrap().parse().unwrap();
    let mut hm = reqwest::header::HeaderMap::new();
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            hm.append(
                reqwest::header::HeaderName::from_bytes(k.trim().as_bytes()).unwrap(),
                reqwest::header::HeaderValue::from_str(v.trim()).unwrap(),
            );
        }
    }
    if hm.get("transfer-encoding").is_some_and(|v| v == "chunked") {
        body = dechunk(&body);
    }
    Resp {
        status: StatusCode::from_u16(code).unwrap(),
        headers: hm,
        body,
    }
}

fn dechunk(mut b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(nl) = b.windows(2).position(|w| w == b"\r\n") {
        let size = usize::from_str_radix(std::str::from_utf8(&b[..nl]).unwrap().trim(), 16).unwrap();
        if size == 0 {
            break;
        }
        out.extend_from_slice(&b[nl + 2..nl + 2 + size]);
        b = &b[nl + 2 + size + 2..];
    }
    out
}

// ---------------------------------------------------------------------------
// Media fixtures
// ---------------------------------------------------------------------------

fn lcg(seed: u64) -> impl FnMut() -> u8 {
    let mut s = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    move || {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (s >> 33) as u8
    }
}

/// A `w`x`h` RGBA image of 8x8 blocks with seed-dependent brightness: very
/// different pHashes for different seeds. `shift` nudges every pixel a
/// little (same picture, different bytes: a near-duplicate).
pub fn image(w: u32, h: u32, seed: u64, shift: u8) -> image::RgbaImage {
    let mut next = lcg(seed);
    let blocks: Vec<[u8; 3]> = (0..64).map(|_| [next(), next(), next()]).collect();
    image::RgbaImage::from_fn(w, h, |x, y| {
        let b = blocks[((y * 8 / h) * 8 + (x * 8 / w)) as usize];
        image::Rgba([
            b[0].saturating_add(shift),
            b[1].saturating_add(shift),
            b[2].saturating_add(shift),
            255,
        ])
    })
}

pub fn encode(img: &image::RgbaImage, format: image::ImageFormat) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    match format {
        image::ImageFormat::Jpeg => image::DynamicImage::ImageRgba8(img.clone())
            .to_rgb8()
            .write_to(&mut out, format)
            .unwrap(),
        _ => img.write_to(&mut out, format).unwrap(),
    }
    out.into_inner()
}

pub fn png(seed: u64) -> Vec<u8> {
    encode(&image(64, 64, seed, 0), image::ImageFormat::Png)
}

/// Near-duplicate of `png(seed)`: same picture, slightly brighter.
pub fn png_near(seed: u64) -> Vec<u8> {
    encode(&image(64, 64, seed, 3), image::ImageFormat::Png)
}

/// A solid-colour 48x48 PNG (the mock enricher embeds colours).
pub fn solid_png(rgb: [u8; 3], noise_seed: u64) -> Vec<u8> {
    let mut next = lcg(noise_seed);
    let img = image::RgbaImage::from_fn(48, 48, |_, _| {
        let n = next() % 3;
        image::Rgba([
            rgb[0].saturating_sub(n),
            rgb[1].saturating_sub(n),
            rgb[2].saturating_sub(n),
            255,
        ])
    });
    encode(&img, image::ImageFormat::Png)
}

pub fn jpeg(seed: u64) -> Vec<u8> {
    encode(&image(64, 64, seed, 0), image::ImageFormat::Jpeg)
}

/// A 3-frame animated GIF.
pub fn gif_anim(seed: u64) -> Vec<u8> {
    use image::codecs::gif::{GifEncoder, Repeat};
    use image::{Delay, Frame};
    let mut out = Vec::new();
    {
        let mut enc = GifEncoder::new(&mut out);
        enc.set_repeat(Repeat::Infinite).unwrap();
        for i in 0..3 {
            let frame = Frame::from_parts(image(32, 32, seed + i, 0), 0, 0, Delay::from_numer_denom_ms(100, 1));
            enc.encode_frame(frame).unwrap();
        }
    }
    out
}

pub fn ffmpeg_available() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}
