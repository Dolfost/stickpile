//! What the popup needs from the outside world, as an object-safe trait so
//! the GTK layer can run against `stickpile_core::Session` or the
//! [`crate::demo`] directory backend. All futures are `Send + 'static` and run
//! on the tokio runtime; the GTK side awaits their `JoinHandle`s on the GLib
//! main loop.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use stickpile_api::{ItemSummary, TargetApp};
use stickpile_core::{CoreError, PreparedCopy, Session};
use stickpile_quick::model::Results;

pub type BoxFut<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

pub trait Backend: Send + Sync + 'static {
    /// The server rejected our credentials on the last request that reached
    /// it (no token and no tailnet identity, or a revoked token). Having no
    /// token is not enough: tailnet users work without one.
    fn login_required(&self) -> bool;
    fn search(self: Arc<Self>, query: String, target: TargetApp, limit: usize) -> BoxFut<Result<Results, String>>;
    fn recent(self: Arc<Self>, limit: usize) -> BoxFut<Vec<ItemSummary>>;
    fn thumbnail(self: Arc<Self>, item: ItemSummary) -> BoxFut<Result<PathBuf, String>>;
    fn prepare_copy(
        self: Arc<Self>,
        item: ItemSummary,
        target: TargetApp,
        sticker: bool,
    ) -> BoxFut<Result<PreparedCopy, String>>;
    fn record_copy(self: Arc<Self>, item: ItemSummary, prepared: PreparedCopy) -> BoxFut<Result<(), String>>;
    fn sync(self: Arc<Self>) -> BoxFut<Result<(), String>>;
}

/// User-facing text for a core error.
pub fn describe(e: &CoreError) -> String {
    match e {
        CoreError::NotLoggedIn => "Not logged in — run `stickpile login`".into(),
        CoreError::Offline(_) => "Server unreachable and the item is not cached".into(),
        e if e.is_unauthorized() => "The server rejected the token — run `stickpile login`".into(),
        other => other.to_string(),
    }
}

/// Usage events from this client are reported as `desktop/<version>`.
pub fn client_name() -> String {
    format!("desktop/{}", env!("CARGO_PKG_VERSION"))
}

/// The real backend.
pub struct CoreBackend {
    session: Session,
    login_required: AtomicBool,
}

impl CoreBackend {
    /// `login_required`: carry over what the previous session learned (the
    /// shell re-opens the session to pick up a token from `stickpile login`).
    pub fn new(session: Session, login_required: bool) -> CoreBackend {
        CoreBackend {
            session: session.with_client_name(client_name()),
            login_required: AtomicBool::new(login_required),
        }
    }

    pub fn has_token(&self) -> bool {
        self.session.is_logged_in()
    }

    /// Track authorisation from a result: a rejection sets the flag, an
    /// answer from the server (`reached_server`) clears it.
    fn note<T>(&self, r: &Result<T, CoreError>, reached_server: impl FnOnce(&T) -> bool) -> Option<String> {
        match r {
            Ok(v) => {
                if reached_server(v) {
                    self.login_required.store(false, Ordering::Relaxed);
                }
                None
            }
            Err(e) => {
                if e.is_unauthorized() {
                    self.login_required.store(true, Ordering::Relaxed);
                }
                Some(describe(e))
            }
        }
    }
}

impl Backend for CoreBackend {
    fn login_required(&self) -> bool {
        self.login_required.load(Ordering::Relaxed)
    }

    fn search(self: Arc<Self>, query: String, target: TargetApp, limit: usize) -> BoxFut<Result<Results, String>> {
        Box::pin(async move {
            let r = self.session.search(&query, Some(target), limit).await;
            if let Some(e) = self.note(&r, |o| !o.offline) {
                return Err(e);
            }
            let out = r.expect("checked");
            Ok(Results {
                hits: out.hits,
                offline: out.offline,
            })
        })
    }

    fn recent(self: Arc<Self>, limit: usize) -> BoxFut<Vec<ItemSummary>> {
        // Session::recent reads the local SQLite log: keep it off the async workers.
        Box::pin(async move {
            tokio::task::spawn_blocking(move || self.session.recent(limit))
                .await
                .unwrap_or_default()
        })
    }

    fn thumbnail(self: Arc<Self>, item: ItemSummary) -> BoxFut<Result<PathBuf, String>> {
        Box::pin(async move {
            let r = self.session.thumbnail(&item).await;
            match self.note(&r, |_| false) {
                Some(e) => Err(e),
                None => Ok(r.expect("checked")),
            }
        })
    }

    fn prepare_copy(
        self: Arc<Self>,
        item: ItemSummary,
        target: TargetApp,
        sticker: bool,
    ) -> BoxFut<Result<PreparedCopy, String>> {
        Box::pin(async move {
            let r = self.session.prepare_copy(&item, target, sticker).await;
            match self.note(&r, |_| false) {
                Some(e) => Err(e),
                None => Ok(r.expect("checked")),
            }
        })
    }

    fn record_copy(self: Arc<Self>, item: ItemSummary, prepared: PreparedCopy) -> BoxFut<Result<(), String>> {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || self.session.record_copy(&item, &prepared).map_err(|e| describe(&e)))
                .await
                .map_err(|e| e.to_string())?
        })
    }

    fn sync(self: Arc<Self>) -> BoxFut<Result<(), String>> {
        Box::pin(async move {
            let r = self.session.sync().await;
            match self.note(&r, |_| true) {
                Some(e) => Err(e),
                None => Ok(()),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thumbs::tests::encoded;
    use image::ImageFormat;
    use stickpile_api::{MediaKind, Visibility};
    use stickpile_core::mock::{dead_url, MockServer};
    use stickpile_core::ClientConfig;

    fn config_for(url: &str, dir: &std::path::Path) -> ClientConfig {
        ClientConfig {
            server_url: url.to_string(),
            token_file: Some(dir.join("state/token")),
            cache_dir: Some(dir.join("cache")),
            timeout_secs: 2,
            ..ClientConfig::default()
        }
    }

    fn animated_gif() -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = image::codecs::gif::GifEncoder::new(&mut out);
            enc.set_repeat(image::codecs::gif::Repeat::Infinite).unwrap();
            for i in 0..3u8 {
                let img = image::RgbaImage::from_pixel(12, 12, image::Rgba([i * 80, 0, 0, 255]));
                enc.encode_frame(image::Frame::new(img)).unwrap();
            }
        }
        out
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
    }

    fn backend(rt: &tokio::runtime::Runtime, cfg: ClientConfig) -> Arc<CoreBackend> {
        Arc::new(CoreBackend::new(rt.block_on(Session::open(cfg)).unwrap(), false))
    }

    #[test]
    fn error_texts() {
        assert_eq!(
            describe(&CoreError::NotLoggedIn),
            "Not logged in — run `stickpile login`"
        );
        assert_eq!(
            describe(&CoreError::Offline("timeout".into())),
            "Server unreachable and the item is not cached"
        );
        assert_eq!(describe(&CoreError::Other("boom".into())), "boom");
        assert!(client_name().starts_with("desktop/"));
    }

    /// The whole popup flow through the real core against its mock server.
    #[test]
    fn core_backend_against_mock_server() {
        let server = MockServer::start();
        let (still, gif) = {
            let mut st = server.state();
            let set = st.add_set("alice", "cats", "Cats", Visibility::Public);
            (
                st.add_item(&set.id, "grumpy cat", &["cat"], encoded(40, 30, ImageFormat::Png)),
                st.add_item(&set.id, "dancing cat", &["cat", "dance"], animated_gif()),
            )
        };
        let token = server.state().issue_token("alice", "desktop test");
        let dir = tempfile::tempdir().unwrap();
        stickpile_core::token::write_token(&dir.path().join("state/token"), &token).unwrap();
        let rt = rt();
        let b = backend(&rt, config_for(&server.url, dir.path()));
        assert!(b.has_token());
        rt.block_on(async {
            b.clone().sync().await.unwrap();
            let r = b.clone().search("cat".into(), TargetApp::Discord, 10).await.unwrap();
            assert!(!r.offline);
            let ids: Vec<_> = r.hits.iter().map(|h| h.item.id.as_str()).collect();
            assert!(
                ids.contains(&still.id.as_str()) && ids.contains(&gif.id.as_str()),
                "{ids:?}"
            );
            assert!(!b.login_required());

            let thumb = b.clone().thumbnail(still.clone()).await.unwrap();
            let rgba = crate::thumbs::decode_file(&thumb, 256).unwrap();
            assert!(rgba.width > 0);

            let p = b
                .clone()
                .prepare_copy(still.clone(), TargetApp::Discord, false)
                .await
                .unwrap();
            assert!(p.file.exists());
            let specs = crate::offers::provider_specs(&p.offers);
            let mimes: Vec<_> = specs.iter().map(|s| s.mime.as_str()).collect();
            assert_eq!(mimes, ["image/png", "text/uri-list", "x-special/gnome-copied-files"]);

            let g = b
                .clone()
                .prepare_copy(gif.clone(), TargetApp::Discord, false)
                .await
                .unwrap();
            assert_eq!(gif.kind, MediaKind::Animation);
            assert_eq!(g.offers[0].mime, "image/gif");

            b.clone().record_copy(gif.clone(), g).await.unwrap();
            let recent = b.clone().recent(5).await;
            assert_eq!(recent.first().map(|i| i.id.as_str()), Some(gif.id.as_str()));
        });
    }

    #[test]
    fn rejected_credentials_set_login_required() {
        let server = MockServer::start();
        let dir = tempfile::tempdir().unwrap();
        let rt = rt();
        let b = backend(&rt, config_for(&server.url, dir.path()));
        assert!(!b.has_token());
        assert!(!b.login_required(), "no token alone does not mean logged out (tailnet)");
        let err = rt
            .block_on(b.clone().search("cat".into(), TargetApp::Generic, 5))
            .unwrap_err();
        assert!(err.contains("stickpile login"), "{err}");
        assert!(b.login_required());
        assert!(rt.block_on(b.clone().sync()).is_err());
        assert!(b.login_required());
    }

    #[test]
    fn offline_search_falls_back_without_login_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let rt = rt();
        let b = Arc::new(CoreBackend::new(
            rt.block_on(Session::open(config_for(&dead_url(), dir.path()))).unwrap(),
            true,
        ));
        let r = rt
            .block_on(b.clone().search("cat".into(), TargetApp::Generic, 5))
            .unwrap();
        assert!(r.offline);
        assert!(b.login_required(), "an offline answer says nothing about credentials");
        assert!(rt.block_on(b.clone().sync()).is_err());
    }
}
