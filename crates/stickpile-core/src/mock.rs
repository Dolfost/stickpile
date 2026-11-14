//! In-process mock of the stickpile REST API (axum, in-memory state) for
//! client tests here and in the CLI. Enabled by the `mock` feature.
//!
//! It implements the contract of `stickpile_api::routes` closely enough for
//! the clients: bearer tokens (password + optional TOTP login), sets, uploads,
//! tar import/export via `stickpile-setfmt`, the `GET /items` cache feed with
//! cursors and tombstones, files, set cover images, likes, a naive substring
//! search (with `is:` filters), usage dedup, popular, `/stats/me`, jobs and a
//! few admin routes. Test hooks: a controllable
//! clock, page size, artificial delays and forced failures, and a request log.
//!
//! The server runs on its own thread and tokio runtime, so it works from both
//! async and blocking tests; it stops when the [`MockServer`] is dropped.

// Handlers return `Result<_, Response>` like ordinary axum code; size is irrelevant here.
#![allow(clippy::result_large_err)]

use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Multipart, Path as AxPath, Query as AxQuery, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use sha2::Digest;
use stickpile_api::profiles::{choose_variant, ItemShape};
use stickpile_api::query::Query;
use stickpile_api::routes as r;
use stickpile_api::*;

pub type Shared = Arc<Mutex<MockState>>;

/// A user account of the mock.
#[derive(Debug, Clone)]
pub struct MockUser {
    pub user: User,
    pub password: String,
    /// Required TOTP code, if the account has TOTP.
    pub totp: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MockToken {
    pub secret: String,
    pub info: ApiToken,
    pub user_id: String,
}

#[derive(Debug, Clone)]
pub struct MockItem {
    pub item: Item,
    pub files: HashMap<Variant, Vec<u8>>,
}

/// All mock state; tests may inspect and change it through [`MockServer::state`].
#[derive(Debug)]
pub struct MockState {
    pub clock: i64,
    pub users: Vec<MockUser>,
    pub tokens: Vec<MockToken>,
    pub sets: Vec<Set>,
    pub items: Vec<MockItem>,
    /// (item id, removed at)
    pub tombstones: Vec<(String, i64)>,
    /// Accepted usage events.
    pub usage: Vec<UsageEvent>,
    /// Event id -> id of the user who reported it.
    pub usage_by: HashMap<String, String>,
    /// Set id -> its own cover image (original bytes, also served as the thumb).
    pub cover_images: HashMap<String, Vec<u8>>,
    /// Number of `POST /usage` requests received.
    pub usage_requests: usize,
    pub likes: HashSet<(String, String)>,
    pub jobs: Vec<Job>,
    /// Max items per `GET /items` page.
    pub items_page_size: usize,
    pub search_delay: Option<Duration>,
    pub usage_delay: Option<Duration>,
    /// Answer `POST /usage` with this status (and an ApiError body).
    pub fail_usage: Option<u16>,
    /// Username that requests without a bearer token are authenticated as,
    /// like the trusted `tailscale serve` proxy's identity headers.
    pub tailnet_user: Option<String>,
    /// `METHOD /path` of every request.
    pub requests: Vec<String>,
}

impl Default for MockState {
    fn default() -> Self {
        MockState {
            clock: 1_700_000_000,
            users: Vec::new(),
            tokens: Vec::new(),
            sets: Vec::new(),
            items: Vec::new(),
            tombstones: Vec::new(),
            usage: Vec::new(),
            usage_by: HashMap::new(),
            cover_images: HashMap::new(),
            usage_requests: 0,
            likes: HashSet::new(),
            jobs: Vec::new(),
            items_page_size: 1000,
            search_delay: None,
            usage_delay: None,
            fail_usage: None,
            tailnet_user: None,
            requests: Vec::new(),
        }
    }
}

/// Media facts sniffed from bytes.
#[derive(Debug, Clone, Copy)]
pub struct Sniffed {
    pub kind: MediaKind,
    pub format: MediaFormat,
    pub width: u32,
    pub height: u32,
}

/// Sniff the container format (never from the extension).
pub fn sniff(bytes: &[u8]) -> Option<Sniffed> {
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        return Some(Sniffed {
            kind: MediaKind::Video,
            format: MediaFormat::Mp4,
            width: 0,
            height: 0,
        });
    }
    if bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Some(Sniffed {
            kind: MediaKind::Video,
            format: MediaFormat::Webm,
            width: 0,
            height: 0,
        });
    }
    let fmt = image::guess_format(bytes).ok()?;
    let format = match fmt {
        image::ImageFormat::Png => MediaFormat::Png,
        image::ImageFormat::Jpeg => MediaFormat::Jpeg,
        image::ImageFormat::WebP => MediaFormat::Webp,
        image::ImageFormat::Gif => MediaFormat::Gif,
        _ => return None,
    };
    let (width, height) = image::ImageReader::with_format(Cursor::new(bytes), fmt)
        .into_dimensions()
        .ok()?;
    let kind = if crate::clipboard::is_animated(bytes, fmt) {
        MediaKind::Animation
    } else {
        MediaKind::Still
    };
    Some(Sniffed {
        kind,
        format,
        width,
        height,
    })
}

fn thumb_for(bytes: &[u8]) -> Vec<u8> {
    let img = image::load_from_memory(bytes)
        .map(|i| i.thumbnail(64, 64))
        .unwrap_or_else(|_| image::DynamicImage::new_rgba8(1, 1));
    let mut out = Cursor::new(Vec::new());
    img.to_rgba8()
        .write_to(&mut out, image::ImageFormat::WebP)
        .expect("encode thumb");
    out.into_inner()
}

impl MockState {
    pub fn tick(&mut self) -> i64 {
        self.clock += 1;
        self.clock
    }

    pub fn add_user(&mut self, username: &str, password: &str, role: Role) -> User {
        let user = User {
            id: crate::new_id(),
            username: username.into(),
            display_name: username.into(),
            role,
            totp_enabled: false,
            tailscale_login: None,
            created_at: self.clock,
            telegram_user_id: None,
            theme: Default::default(),
            language: None,
        };
        self.users.push(MockUser {
            user: user.clone(),
            password: password.into(),
            totp: None,
        });
        user
    }

    /// Issue a token for an existing user without a login request.
    pub fn issue_token(&mut self, username: &str, name: &str) -> String {
        let user_id = self.user_by_name(username).expect("user").user.id.clone();
        self.new_token(&user_id, name).token
    }

    fn new_token(&mut self, user_id: &str, name: &str) -> TokenCreated {
        let secret = format!("spk_{}", crate::new_id());
        let info = ApiToken {
            id: crate::new_id(),
            name: name.into(),
            prefix: secret[..8].to_string(),
            created_at: self.clock,
            last_used_at: None,
            expires_at: None,
            revoked: false,
        };
        self.tokens.push(MockToken {
            secret: secret.clone(),
            info: info.clone(),
            user_id: user_id.into(),
        });
        let user = self
            .users
            .iter()
            .find(|u| u.user.id == user_id)
            .expect("user")
            .user
            .clone();
        TokenCreated {
            token: secret,
            info,
            user,
        }
    }

    pub fn user_by_name(&self, username: &str) -> Option<&MockUser> {
        self.users.iter().find(|u| u.user.username == username)
    }

    pub fn add_set(&mut self, owner: &str, slug: &str, name: &str, visibility: Visibility) -> Set {
        let owner = self.user_by_name(owner).expect("owner").user.clone();
        let now = self.tick();
        let set = Set {
            id: crate::new_id(),
            owner_id: owner.id,
            owner_username: owner.username,
            slug: slug.into(),
            name: name.into(),
            description: String::new(),
            visibility,
            aspect_ratio: None,
            recommended_px: None,
            version: 1,
            source: SetSource::Local,
            git: None,
            telegram_pack: None,
            cover_item_id: None,
            cover_image: None,
            cover_url: None,
            item_count: 0,
            editable: true,
            role: stickpile_api::SetRole::Owner,
            permissions: stickpile_api::Permissions::ALL,
            created_at: now,
            updated_at: now,
        };
        self.sets.push(set.clone());
        set
    }

    /// Add an item from file bytes (sniffed; `original` + `thumb` variants).
    pub fn add_item(&mut self, set_id: &str, name: &str, keywords: &[&str], bytes: Vec<u8>) -> ItemSummary {
        let sn = sniff(&bytes).expect("unsupported media in mock");
        let sha = hex::encode(sha2::Sha256::digest(&bytes));
        let now = self.tick();
        let position = self.items.iter().filter(|i| i.item.set_id == set_id).count() as u32;
        let thumb = thumb_for(&bytes);
        let variants = vec![
            VariantInfo {
                variant: Variant::Original,
                mime: sn.format.mime().into(),
                width: sn.width,
                height: sn.height,
                size_bytes: bytes.len() as u64,
            },
            VariantInfo {
                variant: Variant::Thumb,
                mime: "image/webp".into(),
                width: 64,
                height: 64,
                size_bytes: thumb.len() as u64,
            },
        ];
        let media = MediaInfo {
            id: format!("m{}", &sha[..25]),
            sha256: sha.clone(),
            kind: sn.kind,
            format: sn.format,
            width: sn.width,
            height: sn.height,
            duration_ms: None,
            frame_count: None,
            fps: None,
            has_alpha: false,
            size_bytes: bytes.len() as u64,
            phash: sha[..16].to_string(),
            dhash: sha[16..32].to_string(),
            quality: Some(80),
            variants,
        };
        let item = Item {
            id: crate::new_id(),
            set_id: set_id.into(),
            position,
            filename: format!("{:03}-{}", position + 1, stickpile_setfmt_slug(name)),
            name: name.into(),
            name_source: FieldSource::User,
            keywords: keywords.iter().map(|s| s.to_string()).collect(),
            keywords_source: FieldSource::User,
            emojis: Vec::new(),
            emojis_source: FieldSource::User,
            vibe: None,
            vibe_source: FieldSource::User,
            tags: Vec::new(),
            added_by_username: None,
            media,
            likes: 0,
            liked_by_me: false,
            copies: 0,
            created_at: now,
            updated_at: now,
        };
        let mut files = HashMap::new();
        files.insert(Variant::Original, bytes);
        files.insert(Variant::Thumb, thumb);
        self.items.push(MockItem { item, files });
        self.refresh_set(set_id, now);
        self.summary(&self.items.last().expect("pushed").item)
    }

    /// Add (or replace) a variant file of an item.
    pub fn add_variant(&mut self, item_id: &str, variant: Variant, mime: &str, bytes: Vec<u8>) {
        let now = self.tick();
        let it = self.items.iter_mut().find(|i| i.item.id == item_id).expect("item");
        it.item.media.variants.retain(|v| v.variant != variant);
        it.item.media.variants.push(VariantInfo {
            variant,
            mime: mime.into(),
            width: it.item.media.width,
            height: it.item.media.height,
            size_bytes: bytes.len() as u64,
        });
        it.files.insert(variant, bytes);
        it.item.updated_at = now;
    }

    pub fn delete_item(&mut self, item_id: &str) {
        let now = self.tick();
        if let Some(pos) = self.items.iter().position(|i| i.item.id == item_id) {
            let set_id = self.items.remove(pos).item.set_id;
            self.tombstones.push((item_id.into(), now));
            self.refresh_set(&set_id, now);
        }
    }

    pub fn rename_item(&mut self, item_id: &str, name: &str) {
        let now = self.tick();
        let it = self.items.iter_mut().find(|i| i.item.id == item_id).expect("item");
        it.item.name = name.into();
        it.item.updated_at = now;
    }

    fn refresh_set(&mut self, set_id: &str, now: i64) {
        let count = self.items.iter().filter(|i| i.item.set_id == set_id).count() as u64;
        if let Some(s) = self.sets.iter_mut().find(|s| s.id == set_id) {
            s.item_count = count;
            s.updated_at = now;
            s.version += 1;
        }
    }

    pub fn set(&self, id: &str) -> Option<&Set> {
        self.sets.iter().find(|s| s.id == id)
    }

    pub fn summary(&self, item: &Item) -> ItemSummary {
        let set = self.set(&item.set_id).expect("set of item");
        ItemSummary {
            id: item.id.clone(),
            set_id: item.set_id.clone(),
            set_slug: set.slug.clone(),
            set_name: set.name.clone(),
            owner_username: set.owner_username.clone(),
            name: item.name.clone(),
            keywords: item.keywords.clone(),
            emojis: item.emojis.clone(),
            vibe: item.vibe.clone(),
            tags: item.tags.clone(),
            kind: item.media.kind,
            format: item.media.format,
            width: item.media.width,
            height: item.media.height,
            quality: item.media.quality,
            likes: item.likes,
            copies: item.copies,
            media_id: item.media.id.clone(),
            sha256: item.media.sha256.clone(),
            phash: item.media.phash.clone(),
            variants: item.media.variants.clone(),
            updated_at: item.updated_at,
        }
    }

    fn set_summary(&self, s: &Set) -> SetSummary {
        let cover_item_id = self
            .items
            .iter()
            .find(|i| i.item.set_id == s.id)
            .map(|i| i.item.id.clone());
        let cover_url = match (&s.cover_image, &cover_item_id) {
            (Some(image), _) => Some(image.thumb_url.clone()),
            (None, Some(item)) => Some(r::path(r::ITEM_FILE, &[item, Variant::Thumb.as_str()])),
            (None, None) => None,
        };
        SetSummary {
            id: s.id.clone(),
            slug: s.slug.clone(),
            name: s.name.clone(),
            description: s.description.clone(),
            owner_username: s.owner_username.clone(),
            visibility: s.visibility,
            source: s.source,
            item_count: s.item_count,
            cover_item_id,
            cover_url,
            updated_at: s.updated_at,
        }
    }

    /// Give a set its own cover image (served as both original and thumb),
    /// with URLs that change with the image like the server's.
    pub fn set_cover_image(&mut self, set_id: &str, bytes: Vec<u8>) {
        let sn = sniff(&bytes).expect("unsupported media in mock");
        let sha = hex::encode(sha2::Sha256::digest(&bytes));
        let url = |variant: Variant| {
            format!(
                "{}?v={}",
                r::path(r::SET_COVER_FILE, &[set_id, variant.as_str()]),
                &sha[..16]
            )
        };
        let image = CoverImage {
            thumb_url: url(Variant::Thumb),
            url: url(Variant::Original),
            format: sn.format,
            width: sn.width,
            height: sn.height,
            sha256: sha.clone(),
        };
        let now = self.tick();
        let set = self.sets.iter_mut().find(|s| s.id == set_id).expect("set");
        set.cover_image = Some(image);
        set.updated_at = now;
        self.cover_images.insert(set_id.to_string(), bytes);
    }

    /// Read access by id (own, unlisted, public).
    fn can_read(&self, user: &User, set_id: &str) -> bool {
        self.set(set_id)
            .is_some_and(|s| s.owner_id == user.id || s.visibility != Visibility::Private)
    }

    /// Search / cache visibility (own + public).
    fn searchable(&self, user: &User, set_id: &str) -> bool {
        self.set(set_id)
            .is_some_and(|s| s.owner_id == user.id || s.visibility == Visibility::Public)
    }

    fn owns(&self, user: &User, set_id: &str) -> bool {
        self.set(set_id).is_some_and(|s| s.owner_id == user.id)
    }

    fn new_job(&mut self, kind: JobKind, subject: Option<String>) -> Job {
        let job = Job {
            id: crate::new_id(),
            kind,
            status: JobStatus::Queued,
            attempts: 0,
            error: None,
            subject,
            result: None,
            created_at: self.clock,
            started_at: None,
            finished_at: None,
        };
        self.jobs.push(job.clone());
        job
    }
}

fn stickpile_setfmt_slug(s: &str) -> String {
    stickpile_setfmt::slugify(s)
}

/// Running mock server.
pub struct MockServer {
    pub url: String,
    addr: std::net::SocketAddr,
    state: Shared,
    running: Option<(tokio::sync::oneshot::Sender<()>, std::thread::JoinHandle<()>)>,
}

fn run(
    state: Shared,
    addr: std::net::SocketAddr,
) -> (
    std::net::SocketAddr,
    tokio::sync::oneshot::Sender<()>,
    std::thread::JoinHandle<()>,
) {
    let app = router(state);
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let (addr_tx, addr_rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("runtime");
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
            addr_tx.send(listener.local_addr().expect("addr")).expect("send addr");
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = rx.await;
                })
                .await;
        });
        // Dropping the runtime here aborts connections that are still open.
    });
    let addr = addr_rx.recv().expect("mock server address");
    (addr, tx, thread)
}

impl MockServer {
    /// Start with one user `alice` / `secret` (role user).
    pub fn start() -> MockServer {
        let mut st = MockState::default();
        st.add_user("alice", "secret", Role::User);
        MockServer::start_with(st)
    }

    pub fn start_with(state: MockState) -> MockServer {
        let state: Shared = Arc::new(Mutex::new(state));
        let (addr, tx, thread) = run(state.clone(), "127.0.0.1:0".parse().expect("addr"));
        MockServer {
            url: format!("http://{addr}"),
            addr,
            state,
            running: Some((tx, thread)),
        }
    }

    pub fn state(&self) -> MutexGuard<'_, MockState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn shared(&self) -> Shared {
        self.state.clone()
    }

    /// Stop serving (connections are refused) but keep the state.
    pub fn stop(&mut self) {
        if let Some((tx, thread)) = self.running.take() {
            let _ = tx.send(());
            let _ = thread.join();
        }
    }

    /// Serve again on the same address with the same state.
    pub fn restart(&mut self) {
        self.stop();
        let (_, tx, thread) = run(self.state.clone(), self.addr);
        self.running = Some((tx, thread));
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// An address nothing listens on (for offline tests).
pub fn dead_url() -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = l.local_addr().expect("addr");
    drop(l);
    format!("http://{addr}")
}

// ---------------------------------------------------------------------------

type HResult<T> = std::result::Result<T, Response>;

fn api_err(status: u16, code: &str, message: &str) -> Response {
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (
        status,
        Json(ApiError {
            code: code.into(),
            message: message.into(),
        }),
    )
        .into_response()
}

fn not_found() -> Response {
    api_err(404, "not_found", "not found")
}

fn lock(s: &Shared) -> MutexGuard<'_, MockState> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

fn auth(st: &MockState, headers: &HeaderMap) -> HResult<User> {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let Some(token) = bearer else {
        return match &st.tailnet_user {
            Some(name) => Ok(st.user_by_name(name).expect("tailnet user").user.clone()),
            None => Err(api_err(401, "unauthorized", "missing token")),
        };
    };
    let t = st
        .tokens
        .iter()
        .find(|t| t.secret == token && !t.info.revoked)
        .ok_or_else(|| api_err(401, "unauthorized", "invalid or revoked token"))?;
    Ok(st
        .users
        .iter()
        .find(|u| u.user.id == t.user_id)
        .expect("token user")
        .user
        .clone())
}

async fn log_requests(State(s): State<Shared>, req: Request, next: Next) -> Response {
    lock(&s).requests.push(format!("{} {}", req.method(), req.uri().path()));
    next.run(req).await
}

pub fn router(state: Shared) -> Router {
    Router::new()
        .route(r::HEALTH, get(|| async { "ok" }))
        .route(r::AUTH_TOKEN, post(login))
        .route(r::ME, get(me).patch(patch_me))
        .route(r::TOKENS, get(list_tokens).post(create_token))
        .route(r::TOKEN, axum::routing::delete(revoke_token))
        .route(r::SETS, get(list_sets).post(create_set))
        .route(r::SET, get(get_set).delete(delete_set).patch(update_set))
        .route(r::SET_ITEMS, post(upload))
        .route(r::SET_EXPORT, get(export_set))
        .route(r::SET_COVER_FILE, get(cover_file))
        .route(r::SETS_IMPORT, post(import_set))
        .route(r::SETS_GITPACK, post(gitpack))
        .route(r::SET_PULL, post(pull))
        .route(r::SET_TELEGRAM_EXPORT, post(tg_export))
        .route(r::TELEGRAM_IMPORT, post(tg_import))
        .route(r::ITEMS, get(list_items))
        .route(r::ITEM, get(get_item).delete(delete_item))
        .route(r::ITEM_FILE, get(item_file))
        .route(r::ITEM_LIKE, put(like).delete(unlike))
        .route(r::SEARCH, get(search))
        .route(r::USAGE, post(usage))
        .route(r::POPULAR, get(popular))
        .route(r::STATS_ME, get(stats_me))
        .route(r::JOB, get(job))
        .route(r::ADMIN_USERS, get(admin_users))
        .route(r::ADMIN_STORAGE, get(admin_storage))
        .route(r::ADMIN_JOBS, get(admin_jobs))
        .route(r::ADMIN_AUDIT, get(admin_audit))
        .route(r::ADMIN_INVITATIONS, post(admin_invite))
        .layer(DefaultBodyLimit::max(512 * 1024 * 1024))
        .layer(middleware::from_fn_with_state(state.clone(), log_requests))
        .with_state(state)
}

async fn login(State(s): State<Shared>, Json(req): Json<TokenRequest>) -> HResult<Json<TokenCreated>> {
    let mut st = lock(&s);
    let u = st
        .users
        .iter()
        .find(|u| u.user.username == req.username && u.password == req.password)
        .cloned()
        .ok_or_else(|| api_err(401, "unauthorized", "invalid username or password"))?;
    if let Some(code) = &u.totp {
        match &req.totp {
            None => return Err(api_err(401, "totp_required", "a TOTP code is required")),
            Some(c) if c != code => return Err(api_err(401, "unauthorized", "invalid TOTP code")),
            _ => {}
        }
    }
    Ok(Json(st.new_token(&u.user.id, &req.token_name)))
}

async fn me(State(s): State<Shared>, h: HeaderMap) -> HResult<Json<User>> {
    Ok(Json(auth(&lock(&s), &h)?))
}

/// The language only (what the clients change).
async fn patch_me(
    State(s): State<Shared>,
    h: HeaderMap,
    Json(req): Json<stickpile_api::UpdateMe>,
) -> HResult<Json<User>> {
    let mut st = lock(&s);
    let u = auth(&st, &h)?;
    let user = st.users.iter_mut().find(|m| m.user.id == u.id).expect("authenticated");
    if let Some(lang) = req.language {
        user.user.language = lang;
    }
    Ok(Json(user.user.clone()))
}

async fn list_tokens(State(s): State<Shared>, h: HeaderMap) -> HResult<Json<Vec<ApiToken>>> {
    let st = lock(&s);
    let u = auth(&st, &h)?;
    Ok(Json(
        st.tokens
            .iter()
            .filter(|t| t.user_id == u.id)
            .map(|t| t.info.clone())
            .collect(),
    ))
}

async fn create_token(
    State(s): State<Shared>,
    h: HeaderMap,
    Json(req): Json<CreateToken>,
) -> HResult<Json<TokenCreated>> {
    let mut st = lock(&s);
    let u = auth(&st, &h)?;
    let mut created = st.new_token(&u.id, &req.name);
    created.info.expires_at = req.expires_at;
    Ok(Json(created))
}

async fn revoke_token(State(s): State<Shared>, h: HeaderMap, AxPath(id): AxPath<String>) -> HResult<StatusCode> {
    let mut st = lock(&s);
    let u = auth(&st, &h)?;
    let t = st
        .tokens
        .iter_mut()
        .find(|t| t.info.id == id && t.user_id == u.id)
        .ok_or_else(not_found)?;
    t.info.revoked = true;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_sets(
    State(s): State<Shared>,
    h: HeaderMap,
    AxQuery(q): AxQuery<HashMap<String, String>>,
) -> HResult<Json<Vec<SetSummary>>> {
    let st = lock(&s);
    let u = auth(&st, &h)?;
    let scope = q.get("scope").map(String::as_str).unwrap_or("all");
    let sets = st
        .sets
        .iter()
        .filter(|set| match scope {
            "mine" => set.owner_id == u.id,
            "public" => set.visibility == Visibility::Public,
            _ => set.owner_id == u.id || set.visibility == Visibility::Public,
        })
        .map(|set| st.set_summary(set))
        .collect();
    Ok(Json(sets))
}

async fn create_set(State(s): State<Shared>, h: HeaderMap, Json(req): Json<CreateSet>) -> HResult<Json<Set>> {
    let mut st = lock(&s);
    let u = auth(&st, &h)?;
    if st.sets.iter().any(|x| x.owner_id == u.id && x.slug == req.slug) {
        return Err(api_err(409, "conflict", "slug already used"));
    }
    let mut set = st.add_set(&u.username, &req.slug, &req.name, req.visibility);
    let s2 = st.sets.iter_mut().find(|x| x.id == set.id).expect("set");
    s2.description = req.description;
    s2.aspect_ratio = req.aspect_ratio;
    s2.recommended_px = req.recommended_px;
    set = s2.clone();
    Ok(Json(set))
}

async fn get_set(State(s): State<Shared>, h: HeaderMap, AxPath(id): AxPath<String>) -> HResult<Json<SetDetail>> {
    let st = lock(&s);
    let u = auth(&st, &h)?;
    if !st.can_read(&u, &id) {
        return Err(not_found());
    }
    let mut set = st.set(&id).expect("set").clone();
    set.editable = set.owner_id == u.id;
    if !set.editable {
        set.role = stickpile_api::SetRole::Viewer;
        set.permissions = stickpile_api::Permissions::NONE;
    }
    let mut items: Vec<Item> = st
        .items
        .iter()
        .filter(|i| i.item.set_id == id)
        .map(|i| i.item.clone())
        .collect();
    items.sort_by_key(|i| i.position);
    Ok(Json(SetDetail { set, items }))
}

async fn update_set(
    State(s): State<Shared>,
    h: HeaderMap,
    AxPath(id): AxPath<String>,
    Json(req): Json<UpdateSet>,
) -> HResult<Json<Set>> {
    let mut st = lock(&s);
    let u = auth(&st, &h)?;
    if !st.owns(&u, &id) {
        return Err(not_found());
    }
    let now = st.tick();
    let set = st.sets.iter_mut().find(|x| x.id == id).expect("set");
    if let Some(n) = req.name {
        set.name = n;
    }
    if let Some(v) = req.visibility {
        set.visibility = v;
    }
    set.updated_at = now;
    Ok(Json(set.clone()))
}

async fn delete_set(State(s): State<Shared>, h: HeaderMap, AxPath(id): AxPath<String>) -> HResult<StatusCode> {
    let mut st = lock(&s);
    let u = auth(&st, &h)?;
    if !st.owns(&u, &id) {
        return Err(not_found());
    }
    let ids: Vec<String> = st
        .items
        .iter()
        .filter(|i| i.item.set_id == id)
        .map(|i| i.item.id.clone())
        .collect();
    for i in ids {
        st.delete_item(&i);
    }
    st.sets.retain(|x| x.id != id);
    Ok(StatusCode::NO_CONTENT)
}

async fn read_parts(mut mp: Multipart) -> HResult<Vec<(String, Option<String>, Vec<u8>)>> {
    let mut parts = Vec::new();
    while let Some(field) = mp
        .next_field()
        .await
        .map_err(|e| api_err(400, "bad_request", &e.to_string()))?
    {
        let name = field.name().unwrap_or_default().to_string();
        let file_name = field.file_name().map(str::to_string);
        let bytes = field
            .bytes()
            .await
            .map_err(|e| api_err(400, "bad_request", &e.to_string()))?;
        parts.push((name, file_name, bytes.to_vec()));
    }
    Ok(parts)
}

async fn upload(
    State(s): State<Shared>,
    h: HeaderMap,
    AxPath(id): AxPath<String>,
    mp: Multipart,
) -> HResult<Json<Vec<UploadResult>>> {
    {
        let st = lock(&s);
        let u = auth(&st, &h)?;
        if !st.owns(&u, &id) {
            return Err(not_found());
        }
    }
    let parts = read_parts(mp).await?;
    let mut st = lock(&s);
    let u = auth(&st, &h)?;
    let metas: Vec<UploadMeta> = parts
        .iter()
        .find(|p| p.0 == "meta")
        .and_then(|p| serde_json::from_slice(&p.2).ok())
        .unwrap_or_default();
    let mut results = Vec::new();
    for (idx, (_, file_name, bytes)) in parts.into_iter().filter(|p| p.0 == "file").enumerate() {
        let file = file_name.unwrap_or_else(|| format!("file{idx}"));
        if sniff(&bytes).is_none() {
            results.push(UploadResult {
                file,
                item: None,
                error: Some("unsupported media type".into()),
                already_in_set: false,
                near_duplicates: vec![],
            });
            continue;
        }
        let sha = hex::encode(sha2::Sha256::digest(&bytes));
        if let Some(existing) = st
            .items
            .iter()
            .find(|i| i.item.set_id == id && i.item.media.sha256 == sha)
        {
            results.push(UploadResult {
                file,
                item: Some(existing.item.clone()),
                error: None,
                already_in_set: true,
                near_duplicates: vec![],
            });
            continue;
        }
        let near: Vec<ItemSummary> = st
            .items
            .iter()
            .filter(|i| i.item.media.sha256 == sha && st.owns(&u, &i.item.set_id))
            .map(|i| st.summary(&i.item))
            .collect();
        let meta = metas.get(idx).cloned().unwrap_or_default();
        let name = meta.name.clone().unwrap_or_else(|| {
            file.rsplit_once('.')
                .map(|(a, _)| a.to_string())
                .unwrap_or(file.clone())
        });
        let kw: Vec<String> = meta.keywords.clone().unwrap_or_default();
        let kw_refs: Vec<&str> = kw.iter().map(String::as_str).collect();
        let summary = st.add_item(&id, &name, &kw_refs, bytes);
        let item = st
            .items
            .iter()
            .find(|i| i.item.id == summary.id)
            .expect("item")
            .item
            .clone();
        let near_duplicates = near
            .into_iter()
            .map(|existing| NearDuplicate {
                item_id: item.id.clone(),
                existing,
                distance: 0,
                same_content: true,
            })
            .collect();
        results.push(UploadResult {
            file,
            item: Some(item),
            error: None,
            already_in_set: false,
            near_duplicates,
        });
    }
    Ok(Json(results))
}

async fn export_set(State(s): State<Shared>, h: HeaderMap, AxPath(id): AxPath<String>) -> HResult<Response> {
    let st = lock(&s);
    let u = auth(&st, &h)?;
    if !st.can_read(&u, &id) {
        return Err(not_found());
    }
    let set = st.set(&id).expect("set").clone();
    let mut manifest = stickpile_setfmt::Manifest::new(set.name.clone());
    manifest.version = set.version;
    manifest.slug = Some(set.slug.clone());
    manifest.description = set.description.clone();
    manifest.visibility = set.visibility;
    manifest.aspect_ratio = set.aspect_ratio.map(|(w, h)| format!("{w}:{h}"));
    manifest.recommended_px = set.recommended_px;
    let mut items: Vec<&MockItem> = st.items.iter().filter(|i| i.item.set_id == id).collect();
    items.sort_by_key(|i| i.item.position);
    let total = items.len();
    let mut files = Vec::new();
    for (pos, it) in items.iter().enumerate() {
        let filename =
            stickpile_setfmt::export_filename(pos + 1, total, &it.item.name, it.item.media.format.extension());
        let meta = stickpile_setfmt::ItemMeta {
            name: it.item.name.clone(),
            keywords: it.item.keywords.clone(),
            emojis: it.item.emojis.clone(),
            vibe: it.item.vibe.clone(),
            tags: it.item.tags.clone(),
            ..Default::default()
        };
        manifest.items.insert(filename.clone(), meta.clone());
        files.push(stickpile_setfmt::SetFile {
            filename,
            meta,
            bytes: it.files[&Variant::Original].clone(),
        });
    }
    let bundle = stickpile_setfmt::SetBundle {
        manifest,
        files,
        cover_image: None,
        warnings: vec![],
    };
    let tar = stickpile_setfmt::write_tar(&bundle, &set.slug).map_err(|e| api_err(500, "internal", &e.to_string()))?;
    Ok(([(header::CONTENT_TYPE, "application/x-tar")], Body::from(tar)).into_response())
}

async fn cover_file(
    State(s): State<Shared>,
    h: HeaderMap,
    AxPath((id, variant)): AxPath<(String, String)>,
) -> HResult<Response> {
    let st = lock(&s);
    let u = auth(&st, &h)?;
    if !st.can_read(&u, &id) || !matches!(variant.as_str(), "original" | "thumb") {
        return Err(not_found());
    }
    let bytes = st.cover_images.get(&id).ok_or_else(not_found)?.clone();
    let mime = st
        .set(&id)
        .and_then(|set| set.cover_image.as_ref())
        .map(|c| c.format.mime())
        .unwrap_or("application/octet-stream");
    Ok(([(header::CONTENT_TYPE, mime)], Body::from(bytes)).into_response())
}

async fn import_set(
    State(s): State<Shared>,
    h: HeaderMap,
    AxQuery(q): AxQuery<HashMap<String, String>>,
    mp: Multipart,
) -> HResult<Json<ImportReport>> {
    {
        let st = lock(&s);
        auth(&st, &h)?;
    }
    let parts = read_parts(mp).await?;
    let archive = parts
        .into_iter()
        .find(|p| p.0 == "archive")
        .ok_or_else(|| api_err(400, "bad_request", "missing archive part"))?;
    let bundle =
        stickpile_setfmt::read_tar(&archive.2, 1 << 30).map_err(|e| api_err(400, "bad_request", &e.to_string()))?;
    let mut st = lock(&s);
    let u = auth(&st, &h)?;
    let set_id = match q.get("into") {
        Some(into) => {
            if !st.owns(&u, into) {
                return Err(not_found());
            }
            into.clone()
        }
        None => {
            let base = bundle
                .manifest
                .slug
                .clone()
                .unwrap_or_else(|| stickpile_setfmt::slugify(&bundle.manifest.name));
            let mut slug = base.clone();
            let mut n = 2;
            while st.sets.iter().any(|x| x.owner_id == u.id && x.slug == slug) {
                slug = format!("{base}-{n}");
                n += 1;
            }
            let set = st.add_set(&u.username, &slug, &bundle.manifest.name, bundle.manifest.visibility);
            let s2 = st.sets.iter_mut().find(|x| x.id == set.id).expect("set");
            s2.description = bundle.manifest.description.clone();
            s2.aspect_ratio = bundle.manifest.aspect();
            s2.recommended_px = bundle.manifest.recommended_px;
            set.id
        }
    };
    let mut report = ImportReport::default();
    for f in bundle.files {
        if sniff(&f.bytes).is_none() {
            report.errors.push(ImportError {
                file: f.filename,
                message: "unsupported media type".into(),
            });
            continue;
        }
        let sha = hex::encode(sha2::Sha256::digest(&f.bytes));
        if st
            .items
            .iter()
            .any(|i| i.item.set_id == set_id && i.item.media.sha256 == sha)
        {
            report.unchanged += 1;
            continue;
        }
        let name = if f.meta.name.is_empty() {
            stickpile_setfmt::display_stem(&f.filename)
        } else {
            f.meta.name.clone()
        };
        let kw: Vec<&str> = f.meta.keywords.iter().map(String::as_str).collect();
        let summary = st.add_item(&set_id, &name, &kw, f.bytes);
        let it = st.items.iter_mut().find(|i| i.item.id == summary.id).expect("item");
        it.item.emojis = f.meta.emojis.clone();
        it.item.tags = f.meta.tags.clone();
        it.item.vibe = f.meta.vibe.clone();
        report.created += 1;
    }
    report.set = st.set(&set_id).cloned();
    Ok(Json(report))
}

async fn gitpack(
    State(s): State<Shared>,
    h: HeaderMap,
    Json(req): Json<CreateGitPack>,
) -> HResult<Json<GitPackCreated>> {
    let mut st = lock(&s);
    let u = auth(&st, &h)?;
    let slug = req
        .slug
        .clone()
        .unwrap_or_else(|| stickpile_setfmt::slugify(req.url.rsplit('/').next().unwrap_or("pack")));
    let set = st.add_set(&u.username, &slug, &slug, Visibility::Private);
    let s2 = st.sets.iter_mut().find(|x| x.id == set.id).expect("set");
    s2.source = SetSource::Git;
    s2.git = Some(GitBinding {
        url: req.url.clone(),
        branch: req.branch.clone(),
        mode: req.mode,
        last_commit: None,
        last_pull_at: None,
        last_error: None,
        web_url: None,
    });
    let set = s2.clone();
    let job = st.new_job(JobKind::GitPull, Some(slug));
    Ok(Json(GitPackCreated {
        webhook_url: format!("/api/v1/hooks/git/{}", set.id),
        webhook_secret: "whsec".into(),
        set,
        job,
    }))
}

async fn pull(State(s): State<Shared>, h: HeaderMap, AxPath(id): AxPath<String>) -> HResult<Json<Job>> {
    let mut st = lock(&s);
    let u = auth(&st, &h)?;
    if !st.owns(&u, &id) {
        return Err(not_found());
    }
    Ok(Json(st.new_job(JobKind::GitPull, Some(id))))
}

async fn tg_export(
    State(s): State<Shared>,
    h: HeaderMap,
    AxPath(id): AxPath<String>,
    Json(_req): Json<TelegramExport>,
) -> HResult<Json<Job>> {
    let mut st = lock(&s);
    let u = auth(&st, &h)?;
    if !st.owns(&u, &id) {
        return Err(not_found());
    }
    Ok(Json(st.new_job(JobKind::TelegramExport, Some(id))))
}

async fn tg_import(State(s): State<Shared>, h: HeaderMap, Json(req): Json<TelegramImport>) -> HResult<Json<Job>> {
    let mut st = lock(&s);
    auth(&st, &h)?;
    Ok(Json(st.new_job(JobKind::TelegramImport, Some(req.pack_name))))
}

async fn list_items(
    State(s): State<Shared>,
    h: HeaderMap,
    AxQuery(q): AxQuery<HashMap<String, String>>,
) -> HResult<Json<ItemPage>> {
    let st = lock(&s);
    let u = auth(&st, &h)?;
    let since: Option<i64> = q.get("updated_since").and_then(|v| v.parse().ok());
    let offset: usize = q.get("cursor").and_then(|v| v.parse().ok()).unwrap_or(0);
    let limit: usize = q
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(500)
        .min(st.items_page_size)
        .max(1);
    let mut all: Vec<&MockItem> = st
        .items
        .iter()
        .filter(|i| st.searchable(&u, &i.item.set_id))
        .filter(|i| since.is_none_or(|t| i.item.updated_at >= t))
        .collect();
    all.sort_by(|a, b| (a.item.updated_at, &a.item.id).cmp(&(b.item.updated_at, &b.item.id)));
    let page: Vec<ItemSummary> = all
        .iter()
        .skip(offset)
        .take(limit)
        .map(|i| st.summary(&i.item))
        .collect();
    let next_cursor = (offset + page.len() < all.len()).then(|| (offset + page.len()).to_string());
    let removed = match since {
        Some(t) if offset == 0 => st
            .tombstones
            .iter()
            .filter(|(_, at)| *at >= t)
            .map(|(id, _)| id.clone())
            .collect(),
        _ => Vec::new(),
    };
    Ok(Json(ItemPage {
        items: page,
        removed,
        next_cursor,
        server_time: st.clock,
    }))
}

async fn get_item(State(s): State<Shared>, h: HeaderMap, AxPath(id): AxPath<String>) -> HResult<Json<ItemDetail>> {
    let st = lock(&s);
    let u = auth(&st, &h)?;
    let it = st.items.iter().find(|i| i.item.id == id).ok_or_else(not_found)?;
    if !st.can_read(&u, &it.item.set_id) {
        return Err(not_found());
    }
    let mut item = it.item.clone();
    item.liked_by_me = st.likes.contains(&(u.id.clone(), id.clone()));
    let set = st.set_summary(st.set(&item.set_id).expect("set"));
    Ok(Json(ItemDetail {
        item,
        set,
        quality: None,
        near_duplicates: vec![],
        incompatible: vec![],
    }))
}

async fn delete_item(State(s): State<Shared>, h: HeaderMap, AxPath(id): AxPath<String>) -> HResult<StatusCode> {
    let mut st = lock(&s);
    let u = auth(&st, &h)?;
    let set_id = st
        .items
        .iter()
        .find(|i| i.item.id == id)
        .map(|i| i.item.set_id.clone())
        .ok_or_else(not_found)?;
    if !st.owns(&u, &set_id) {
        return Err(not_found());
    }
    st.delete_item(&id);
    Ok(StatusCode::NO_CONTENT)
}

async fn item_file(
    State(s): State<Shared>,
    h: HeaderMap,
    AxPath((id, variant)): AxPath<(String, String)>,
) -> HResult<Response> {
    let st = lock(&s);
    let u = auth(&st, &h)?;
    let it = st.items.iter().find(|i| i.item.id == id).ok_or_else(not_found)?;
    if !st.can_read(&u, &it.item.set_id) {
        return Err(not_found());
    }
    let v = Variant::parse(&variant).ok_or_else(not_found)?;
    let bytes = it.files.get(&v).ok_or_else(not_found)?.clone();
    let mime = it
        .item
        .media
        .variants
        .iter()
        .find(|x| x.variant == v)
        .map(|x| x.mime.clone())
        .unwrap_or_default();
    Ok(([(header::CONTENT_TYPE, mime)], Body::from(bytes)).into_response())
}

fn set_like(s: &Shared, h: &HeaderMap, id: &str, liked: bool) -> HResult<Json<LikeState>> {
    let mut st = lock(s);
    let u = auth(&st, h)?;
    let set_id = st
        .items
        .iter()
        .find(|i| i.item.id == id)
        .map(|i| i.item.set_id.clone())
        .ok_or_else(not_found)?;
    if !st.can_read(&u, &set_id) {
        return Err(not_found());
    }
    let key = (u.id.clone(), id.to_string());
    if liked {
        st.likes.insert(key);
    } else {
        st.likes.remove(&key);
    }
    let likes = st.likes.iter().filter(|(_, i)| i == id).count() as u64;
    let now = st.tick();
    let it = st.items.iter_mut().find(|i| i.item.id == id).expect("item");
    it.item.likes = likes;
    it.item.updated_at = now;
    Ok(Json(LikeState { liked, likes }))
}

async fn like(State(s): State<Shared>, h: HeaderMap, AxPath(id): AxPath<String>) -> HResult<Json<LikeState>> {
    set_like(&s, &h, &id, true)
}

async fn unlike(State(s): State<Shared>, h: HeaderMap, AxPath(id): AxPath<String>) -> HResult<Json<LikeState>> {
    set_like(&s, &h, &id, false)
}

async fn search(
    State(s): State<Shared>,
    h: HeaderMap,
    AxQuery(q): AxQuery<HashMap<String, String>>,
) -> HResult<Json<SearchResults>> {
    let delay = lock(&s).search_delay;
    if let Some(d) = delay {
        tokio::time::sleep(d).await;
    }
    let st = lock(&s);
    let u = auth(&st, &h)?;
    let text = q.get("q").cloned().unwrap_or_default();
    let limit: usize = q.get("limit").and_then(|v| v.parse().ok()).unwrap_or(60);
    let target = q.get("target").and_then(|t| TargetApp::parse(t));
    let query = Query::parse(&text);
    let mut hits: Vec<SearchHit> = Vec::new();
    for it in st.items.iter().filter(|i| st.searchable(&u, &i.item.set_id)) {
        let sum = st.summary(&it.item);
        if !query.sets.accepts(|v| *v == sum.set_slug) {
            continue;
        }
        let set = st.set(&it.item.set_id).expect("set of item");
        let is = |v: &stickpile_api::query::Is| match v {
            stickpile_api::query::Is::Mine => set.owner_id == u.id,
            stickpile_api::query::Is::Liked => st.likes.contains(&(u.id.clone(), it.item.id.clone())),
            stickpile_api::query::Is::Public => set.visibility == Visibility::Public,
        };
        if !query.is.accepts(is) {
            continue;
        }
        let mut matched = Vec::new();
        let mut ok = true;
        for term in &query.text {
            let mut m = false;
            if sum.name.to_lowercase().contains(term.as_str()) {
                matched.push("name".to_string());
                m = true;
            }
            if sum.keywords.iter().any(|k| k.to_lowercase().contains(term.as_str())) {
                matched.push("keyword".to_string());
                m = true;
            }
            ok &= m;
        }
        if !ok {
            continue;
        }
        matched.dedup();
        let incompatible = target.and_then(|t| {
            let shape = ItemShape {
                kind: sum.kind,
                format: sum.format,
                duration_ms: None,
                variants: &sum.variants,
            };
            choose_variant(t, &shape, false).err().map(|e| e.0)
        });
        hits.push(SearchHit {
            score: matched.len() as f32 + sum.copies as f32 * 0.01,
            item: sum,
            matched,
            incompatible,
        });
    }
    hits.sort_by(|a, b| b.score.total_cmp(&a.score));
    let total = hits.len() as u64;
    hits.truncate(limit);
    Ok(Json(SearchResults {
        hits,
        total,
        query: query.to_string(),
        semantic: false,
    }))
}

async fn usage(State(s): State<Shared>, h: HeaderMap, Json(batch): Json<UsageBatch>) -> HResult<Json<UsageAck>> {
    let (delay, fail) = {
        let mut st = lock(&s);
        auth(&st, &h)?;
        st.usage_requests += 1;
        (st.usage_delay, st.fail_usage)
    };
    if let Some(d) = delay {
        tokio::time::sleep(d).await;
    }
    if let Some(status) = fail {
        return Err(api_err(status, "internal", "usage failure injected"));
    }
    let mut st = lock(&s);
    let u = auth(&st, &h)?;
    let mut ack = UsageAck {
        accepted: 0,
        duplicates: 0,
        rejected: vec![],
    };
    for ev in batch.events {
        if st.usage.iter().any(|e| e.id == ev.id) {
            ack.duplicates += 1;
            continue;
        }
        let Some(set_id) = st
            .items
            .iter()
            .find(|i| i.item.id == ev.item_id)
            .map(|i| i.item.set_id.clone())
        else {
            ack.rejected.push(ev.id);
            continue;
        };
        if !st.can_read(&u, &set_id) {
            ack.rejected.push(ev.id);
            continue;
        }
        let now = st.tick();
        if let Some(it) = st.items.iter_mut().find(|i| i.item.id == ev.item_id) {
            it.item.copies += 1;
            it.item.updated_at = now;
        }
        st.usage_by.insert(ev.id.clone(), u.id.clone());
        st.usage.push(ev);
        ack.accepted += 1;
    }
    Ok(Json(ack))
}

async fn stats_me(State(s): State<Shared>, h: HeaderMap) -> HResult<Json<UserStats>> {
    let st = lock(&s);
    let u = auth(&st, &h)?;
    let mine: Vec<&UsageEvent> = st
        .usage
        .iter()
        .filter(|e| st.usage_by.get(&e.id) == Some(&u.id))
        .collect();
    // Like the server: the caller's own copies, top 10, of items they can still read.
    let mut counts: HashMap<&str, u64> = HashMap::new();
    for e in &mine {
        *counts.entry(e.item_id.as_str()).or_default() += 1;
    }
    let mut top: Vec<(&str, u64)> = counts.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    let top_items = top
        .into_iter()
        .filter_map(|(id, n)| {
            let it = st.items.iter().find(|i| i.item.id == id)?;
            st.can_read(&u, &it.item.set_id).then(|| PopularEntry {
                item: st.summary(&it.item),
                value: n,
            })
        })
        .take(10)
        .collect();
    let own_sets: HashSet<&str> = st
        .sets
        .iter()
        .filter(|x| x.owner_id == u.id)
        .map(|x| x.id.as_str())
        .collect();
    Ok(Json(UserStats {
        sets: own_sets.len() as u64,
        items: st
            .items
            .iter()
            .filter(|i| own_sets.contains(i.item.set_id.as_str()))
            .count() as u64,
        storage_bytes: 0,
        copies_total: mine.len() as u64,
        copies_last_7d: mine.len() as u64,
        likes_given: st.likes.iter().filter(|(user, _)| *user == u.id).count() as u64,
        likes_received: 0,
        top_items,
        top_targets: vec![],
        daily_copies: vec![0; 30],
    }))
}

async fn popular(
    State(s): State<Shared>,
    h: HeaderMap,
    AxQuery(q): AxQuery<HashMap<String, String>>,
) -> HResult<Json<Popular>> {
    let st = lock(&s);
    let u = auth(&st, &h)?;
    let limit: usize = q.get("limit").and_then(|v| v.parse().ok()).unwrap_or(20);
    let mut items: Vec<ItemSummary> = st
        .items
        .iter()
        .filter(|i| st.searchable(&u, &i.item.set_id))
        .map(|i| st.summary(&i.item))
        .collect();
    let entries = |items: &[ItemSummary], f: fn(&ItemSummary) -> u64| -> Vec<PopularEntry> {
        items
            .iter()
            .filter(|i| f(i) > 0)
            .take(limit)
            .map(|i| PopularEntry {
                item: i.clone(),
                value: f(i),
            })
            .collect()
    };
    items.sort_by_key(|i| std::cmp::Reverse(i.copies));
    let most_copied = entries(&items, |i| i.copies);
    items.sort_by_key(|i| std::cmp::Reverse(i.likes));
    let most_liked = entries(&items, |i| i.likes);
    Ok(Json(Popular {
        trending: most_copied.clone(),
        most_copied,
        most_liked,
    }))
}

async fn job(State(s): State<Shared>, h: HeaderMap, AxPath(id): AxPath<String>) -> HResult<Json<Job>> {
    let mut st = lock(&s);
    auth(&st, &h)?;
    let now = st.clock;
    let job = st.jobs.iter_mut().find(|j| j.id == id).ok_or_else(not_found)?;
    // Every poll advances the job: queued -> running -> done.
    job.status = match job.status {
        JobStatus::Queued => JobStatus::Running,
        _ => JobStatus::Done,
    };
    if job.status == JobStatus::Done {
        job.finished_at = Some(now);
    }
    Ok(Json(job.clone()))
}

fn require_admin(st: &MockState, h: &HeaderMap) -> HResult<User> {
    let u = auth(st, h)?;
    if u.role != Role::Admin {
        return Err(api_err(403, "forbidden", "admin only"));
    }
    Ok(u)
}

async fn admin_users(State(s): State<Shared>, h: HeaderMap) -> HResult<Json<Vec<AdminUser>>> {
    let st = lock(&s);
    require_admin(&st, &h)?;
    Ok(Json(
        st.users
            .iter()
            .map(|u| AdminUser {
                user: u.user.clone(),
                disabled: false,
                has_password: true,
                last_login_at: None,
                sets: st.sets.iter().filter(|x| x.owner_id == u.user.id).count() as u64,
                items: 0,
                storage_bytes: 0,
            })
            .collect(),
    ))
}

async fn admin_storage(State(s): State<Shared>, h: HeaderMap) -> HResult<Json<StorageUsage>> {
    let st = lock(&s);
    require_admin(&st, &h)?;
    let media_bytes = st.items.iter().map(|i| i.item.media.size_bytes).sum();
    Ok(Json(StorageUsage {
        media_bytes,
        database_bytes: 4096,
        media_count: st.items.len() as u64,
        item_count: st.items.len() as u64,
        per_user: vec![],
    }))
}

async fn admin_jobs(State(s): State<Shared>, h: HeaderMap) -> HResult<Json<Vec<Job>>> {
    let st = lock(&s);
    require_admin(&st, &h)?;
    Ok(Json(st.jobs.clone()))
}

async fn admin_audit(State(s): State<Shared>, h: HeaderMap) -> HResult<Json<Vec<AuditEntry>>> {
    let st = lock(&s);
    let u = require_admin(&st, &h)?;
    Ok(Json(vec![AuditEntry {
        id: 1,
        at: st.clock,
        actor_id: Some(u.id),
        actor_username: Some(u.username),
        action: "login".into(),
        target: None,
        ip: Some("127.0.0.1".into()),
        details: None,
    }]))
}

async fn admin_invite(
    State(s): State<Shared>,
    h: HeaderMap,
    Json(req): Json<CreateInvitation>,
) -> HResult<Json<InvitationCreated>> {
    let st = lock(&s);
    let u = require_admin(&st, &h)?;
    let invitation = Invitation {
        id: crate::new_id(),
        role: req.role,
        created_by: u.username,
        created_at: st.clock,
        expires_at: req.expires_in.map(|e| st.clock + e),
        used_by: None,
        used_at: None,
    };
    Ok(Json(InvitationCreated {
        url: format!("http://mock/register?code={}", invitation.id),
        invitation,
    }))
}
