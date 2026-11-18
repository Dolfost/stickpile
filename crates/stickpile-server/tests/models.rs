//! Downloading the AI model files at start (`ai.download_models`,
//! `stickpile_server::models`): a local host serves fake model files under
//! a test table, and stand-ins (`MockEnricher`, `MockTextReader`) take the
//! place of CLIP and the OCR models once the files are in place.

mod common;
mod web_common;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path as UrlPath, State};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use common::{png, Opts, TestServer};
use futures::StreamExt;
use sha2::{Digest, Sha256};
use stickpile_ai::{layout, MockEnricher, MockTextReader};
use stickpile_api::Visibility;
use stickpile_server::models::{ModelSource, ModelSources};
use stickpile_server::state::AiStatus;
use stickpile_server::{EnricherSource, StartOptions, TextReaderSource};
use web_common::Browser;

#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);

impl Logs {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl std::io::Write for Logs {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Logs {
    type Writer = Logs;
    fn make_writer(&'a self) -> Logs {
        self.clone()
    }
}

/// Capture this test's log lines (the test runtime runs every task on the
/// test's thread).
fn capture() -> (Logs, tracing::subscriber::DefaultGuard) {
    let logs = Logs::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(logs.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .finish();
    (logs, tracing::subscriber::set_default(subscriber))
}

/// A few KB that stand for the model file at `path`.
fn fake(path: &str) -> Vec<u8> {
    format!("{path}: not really a model\n")
        .repeat(100 + path.len())
        .into_bytes()
}

/// A local host for the model files: the test table points at it.
struct Host {
    table: Vec<ModelSource>,
    /// Paths asked for, in order.
    hits: Arc<Mutex<Vec<String>>>,
    /// Opens the gate of the gated file.
    open: tokio::sync::watch::Sender<bool>,
}

#[derive(Clone)]
struct Served {
    files: Arc<HashMap<String, Vec<u8>>>,
    hits: Arc<Mutex<Vec<String>>>,
    /// Sent in two halves, the second once the gate opens.
    gated: Option<String>,
    open: tokio::sync::watch::Receiver<bool>,
}

async fn serve(State(s): State<Served>, UrlPath(path): UrlPath<String>) -> Response {
    s.hits.lock().unwrap().push(path.clone());
    let Some(bytes) = s.files.get(&path).cloned() else {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    };
    if s.gated.as_deref() != Some(path.as_str()) {
        return bytes.into_response();
    }
    let len = bytes.len();
    let second = Bytes::from(bytes[len / 2..].to_vec());
    let first = Bytes::from(bytes[..len / 2].to_vec());
    let mut open = s.open.clone();
    let rest = futures::stream::once(async move {
        let _ = open.wait_for(|o| *o).await;
        Ok::<_, std::io::Error>(second)
    });
    let body = futures::stream::once(async move { Ok::<_, std::io::Error>(first) }).chain(rest);
    Response::builder()
        .header(axum::http::header::CONTENT_LENGTH, len)
        .body(Body::from_stream(body))
        .unwrap()
}

impl Host {
    /// Serves every file of the table; `corrupt` ones with one byte changed
    /// (the table keeps the right sha256); `gated` in two halves.
    async fn start(corrupt: &[&str], gated: Option<&str>) -> Host {
        let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", tcp.local_addr().unwrap());
        let mut files = HashMap::new();
        let mut table = Vec::new();
        for f in stickpile_ai::MODEL_FILES {
            let bytes = fake(f.path);
            table.push(ModelSource {
                path: f.path.to_string(),
                url: format!("{base}/f/{}", f.path),
                sha256: hex::encode(Sha256::digest(&bytes)),
                size: bytes.len() as u64,
            });
            let mut served = bytes;
            if corrupt.contains(&f.path) {
                served[0] ^= 1;
            }
            files.insert(f.path.to_string(), served);
        }
        let hits = Arc::new(Mutex::new(Vec::new()));
        let (open, rx) = tokio::sync::watch::channel(false);
        let app = axum::Router::new()
            .route("/f/{*path}", axum::routing::get(serve))
            .with_state(Served {
                files: Arc::new(files),
                hits: hits.clone(),
                gated: gated.map(str::to_string),
                open: rx,
            });
        tokio::spawn(async move { axum::serve(tcp, app).await });
        Host { table, hits, open }
    }

    fn sources(&self) -> ModelSources {
        ModelSources {
            files: self.table.clone(),
            attempts: 2,
            retry_wait: Duration::from_millis(10),
            enricher: Some(Arc::new(MockEnricher::default())),
            text_reader: Some(Arc::new(MockTextReader::new(&["HELLO"]))),
        }
    }

    fn hits(&self) -> Vec<String> {
        self.hits.lock().unwrap().clone()
    }

    fn row(&self, path: &str) -> &ModelSource {
        self.table.iter().find(|f| f.path == path).unwrap()
    }
}

/// A server with CLIP and OCR loading from `model_dir` (as in production)
/// and the host's table.
async fn server(host: &Host, model_dir: &Path, f: impl FnOnce(&mut stickpile_server::Config)) -> TestServer {
    let dir = model_dir.to_path_buf();
    TestServer::start_with(
        Opts {
            ai_from_config: true,
            ocr_from_config: true,
            models: Some(host.sources()),
            ..Default::default()
        },
        move |c| {
            c.ai.enabled = true;
            c.ai.model_dir = dir;
            f(c);
        },
    )
    .await
}

async fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..1000 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}");
}

async fn settled(srv: &TestServer) {
    wait_until("the AI status to settle", || srv.state.ai_status() != AiStatus::Loading).await;
}

/// Every file below `dir`, relative, sorted.
fn tree(dir: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                out.push(p.strip_prefix(root).unwrap().display().to_string());
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

fn strings(paths: &[&str]) -> Vec<String> {
    paths.iter().map(|p| p.to_string()).collect()
}

const ENGLISH: [&str; 3] = [layout::VISION, layout::CLIP_TEXT, layout::CLIP_TOKENIZER];
const OCR: [&str; 2] = [layout::OCR_DET, layout::OCR_REC];

#[tokio::test]
async fn missing_models_are_downloaded_verified_and_loaded() {
    let (logs, _guard) = capture();
    let host = Host::start(&[], Some(layout::VISION)).await;
    let tmp = tempfile::tempdir().unwrap();
    let models = tmp.path().join("models");
    let srv = server(&host, &models, |_| {}).await;
    assert_eq!(srv.state.ai_status(), AiStatus::Loading, "startup does not wait");

    // Halfway through the image tower: the OCR files came first and their
    // reader is published; the admin page says how far the download is.
    wait_until("the image tower to start", || {
        host.hits().contains(&layout::VISION.to_string()) && srv.state.model_download().is_some()
    })
    .await;
    assert!(
        srv.state.text_reader().is_some(),
        "OCR loads as soon as its files are in place"
    );
    assert_eq!(srv.state.ai_status(), AiStatus::Loading);
    let (done, total) = srv.state.model_download().unwrap();
    let want: u64 = host
        .table
        .iter()
        .filter(|f| ENGLISH.contains(&f.path.as_str()) || OCR.contains(&f.path.as_str()))
        .map(|f| f.size)
        .sum();
    assert_eq!(total, want);
    assert!(done < total, "{done} of {total}");
    let admin = srv.admin("root").await;
    let browser = Browser::login(&srv, &admin).await;
    let page = browser.get("/admin").await.expect(200).text();
    assert!(page.contains("downloading the models ("), "{page}");
    assert!(!page.contains(">loading<"), "{page}");
    assert!(models.join("clip-vit-b32/vision.onnx.part").is_file());
    assert!(!models.join(layout::VISION).exists(), "nothing unverified in place");

    // An upload meanwhile: its `Enrich` job reads the words now and leaves
    // the embedding to the recheck after the download (no failed job).
    let set = srv.create_set(&admin, "s", Visibility::Private).await;
    let item = srv.upload_one(&admin, &set.id, "a.png", png(1)).await;
    srv.run_jobs().await;
    let (ocr, embedding): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT ocr_model, embedding_model FROM media WHERE id = ?")
            .bind(&item.media.id)
            .fetch_one(&srv.state.db)
            .await
            .unwrap();
    assert!(ocr.is_some() && embedding.is_none(), "{ocr:?} {embedding:?}");
    let failed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE status != 'done'")
        .fetch_one(&srv.state.db)
        .await
        .unwrap();
    assert_eq!(failed, 0);

    host.open.send(true).unwrap();
    settled(&srv).await;
    assert_eq!(srv.state.ai_status(), AiStatus::Ready);
    assert_eq!(srv.state.ai_model_id(), Some("mock-64"));
    assert_eq!(srv.state.model_download(), None);
    let mut picked: Option<i64> = None;
    for _ in 0..500 {
        picked = sqlx::query_scalar("SELECT total FROM rechecks WHERE parts LIKE '%embedding%'")
            .fetch_optional(&srv.state.db)
            .await
            .unwrap();
        if picked.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(picked, Some(1), "the server's recheck embeds the upload");
    // Exactly the files of the configuration, once each, OCR first.
    assert_eq!(
        host.hits(),
        strings(&[OCR[0], OCR[1], ENGLISH[0], ENGLISH[1], ENGLISH[2]])
    );
    for p in ENGLISH.iter().chain(&OCR) {
        assert_eq!(std::fs::read(models.join(p)).unwrap(), fake(p), "{p}");
    }
    assert_eq!(
        tree(&models),
        sorted(strings(&[ENGLISH[0], ENGLISH[1], ENGLISH[2], OCR[0], OCR[1]])),
        "no part files left"
    );
    let page = browser.get("/admin").await.expect(200).text();
    assert!(!page.contains("downloading the models"), "{page}");

    let text = logs.text();
    let mb = stickpile_server::models::mb(want);
    assert!(
        text.contains(&format!("downloading 5 model files, {mb} MB, to {}", models.display())),
        "{text}"
    );
    assert!(text.contains("downloaded clip-vit-b32/vision.onnx ("), "{text}");
    assert!(text.contains("downloaded and verified 5 model files in"), "{text}");
    assert!(text.contains("OCR ready") && text.contains("AI ready"), "{text}");
}

/// Pictures uploaded before the models were there get them from the
/// server's own rechecks, as after any model change.
#[tokio::test]
async fn the_rechecks_run_once_the_downloaded_models_load() {
    let host = Host::start(&[], None).await;
    let srv = TestServer::start().await;
    let u = srv.user("alice").await;
    let set = srv.create_set(&u, "s", Visibility::Private).await;
    srv.upload_one(&u, &set.id, "a.png", png(1)).await;

    let tmp = tempfile::tempdir().unwrap();
    let mut config = srv.state.config.clone();
    config.ai.enabled = true;
    config.ai.model_dir = tmp.path().join("models");
    let state = stickpile_server::build_state(
        config,
        StartOptions {
            enricher: EnricherSource::FromConfig,
            text_reader: TextReaderSource::FromConfig,
            models: host.sources(),
        },
    )
    .await
    .unwrap();
    wait_until("the models", || state.ai_status() != AiStatus::Loading).await;
    assert_eq!(state.ai_status(), AiStatus::Ready);
    assert!(state.text_reader().is_some());
    // The CLIP recheck starts just after the status turns ready.
    let mut parts: Vec<String> = Vec::new();
    for _ in 0..500 {
        parts = sqlx::query_scalar("SELECT parts FROM rechecks ORDER BY rowid")
            .fetch_all(&state.db)
            .await
            .unwrap();
        if parts.len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(parts.len(), 2, "{parts:?}");
    assert!(parts[0].contains("text"), "{parts:?}");
    assert!(parts[1].contains("embedding"), "{parts:?}");
}

#[tokio::test]
async fn a_wrong_sha256_is_rejected_and_nothing_installed() {
    let (logs, _guard) = capture();
    let host = Host::start(&[layout::OCR_REC], None).await;
    let tmp = tempfile::tempdir().unwrap();
    let models = tmp.path().join("models");
    let dir = models.clone();
    // OCR only: CLIP comes from elsewhere.
    let srv = TestServer::start_with(
        Opts {
            ai: true,
            ocr_from_config: true,
            models: Some(host.sources()),
            ..Default::default()
        },
        move |c| {
            c.ai.enabled = true;
            c.ai.model_dir = dir;
        },
    )
    .await;
    wait_until("the download to end", || {
        logs.text().contains("could not be downloaded")
    })
    .await;
    let rec = host.row(layout::OCR_REC).url.clone();
    // Each attempt gets the file and rejects it.
    assert_eq!(host.hits(), strings(&[OCR[0], OCR[1], OCR[1]]));
    assert_eq!(tree(&models), strings(&[OCR[0]]), "only the verified file is in place");
    assert!(srv.state.text_reader().is_none());
    assert_eq!(srv.state.model_download(), None);
    let text = logs.text();
    assert_eq!(
        text.matches(&format!("ocr/rec.onnx from {rec} does not match its pinned sha256"))
            .count(),
        2,
        "{text}"
    );
    assert!(text.contains("deleted it, nothing installed"), "{text}");
    assert!(
        text.contains(&format!("could not download ocr/rec.onnx from {rec} (2 attempts")),
        "{text}"
    );
    assert!(text.contains("1 of 2 model files could not be downloaded"), "{text}");
    assert!(text.contains("ai.ocr is on, but the OCR models are missing"), "{text}");
}

#[tokio::test]
async fn download_models_false_downloads_nothing() {
    let (logs, _guard) = capture();
    let host = Host::start(&[], None).await;
    let tmp = tempfile::tempdir().unwrap();
    let srv = server(&host, tmp.path(), |c| c.ai.download_models = false).await;
    settled(&srv).await;
    assert_eq!(srv.state.ai_status(), AiStatus::Failed, "no model files, no AI");
    assert!(host.hits().is_empty());
    assert!(tree(tmp.path()).is_empty());
    let text = logs.text();
    assert!(
        text.contains(&format!(
            "not downloading the 5 model files missing in {} (",
            tmp.path().display()
        )) && text.contains("MB): ai.download_models = false"),
        "{text}"
    );
    assert_eq!(text.matches("not downloading").count(), 1, "one line: {text}");
}

#[tokio::test]
async fn a_read_only_model_dir_is_skipped() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let models = tmp.path().join("models");
    std::fs::create_dir(&models).unwrap();
    std::fs::set_permissions(&models, std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::write(models.join("probe"), b"x").is_ok() {
        eprintln!("running as root: every directory is writable; skipping");
        return;
    }
    let (logs, _guard) = capture();
    let host = Host::start(&[], None).await;
    let srv = server(&host, &models, |_| {}).await;
    settled(&srv).await;
    assert_eq!(srv.state.ai_status(), AiStatus::Failed);
    assert!(host.hits().is_empty());
    let text = logs.text();
    assert!(
        text.contains(&format!("{} is not writable", models.display())) && text.contains("not downloading the 5"),
        "{text}"
    );
    std::fs::set_permissions(&models, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[tokio::test]
async fn only_the_files_the_configuration_needs_are_fetched() {
    // Multilingual, no OCR.
    let host = Host::start(&[], None).await;
    let tmp = tempfile::tempdir().unwrap();
    let srv = server(&host, tmp.path(), |c| {
        c.ai.text_encoder = stickpile_server::config::TextEncoder::Multilingual;
        c.ai.ocr = false;
    })
    .await;
    settled(&srv).await;
    assert_eq!(srv.state.ai_status(), AiStatus::Ready);
    let multilingual = [
        layout::VISION,
        layout::MCLIP_MODEL,
        layout::MCLIP_TOKENIZER,
        layout::MCLIP_DENSE,
    ];
    assert_eq!(host.hits(), strings(&multilingual));
    assert_eq!(tree(tmp.path()), sorted(strings(&multilingual)));
    assert!(srv.state.text_reader().is_none());

    // English with OCR: the image tower is there already.
    let host = Host::start(&[], None).await;
    let srv = server(&host, tmp.path(), |_| {}).await;
    settled(&srv).await;
    assert_eq!(srv.state.ai_status(), AiStatus::Ready);
    assert_eq!(host.hits(), strings(&[OCR[0], OCR[1], ENGLISH[1], ENGLISH[2]]));
    assert!(srv.state.text_reader().is_some());
}

#[tokio::test]
async fn verified_files_are_kept_and_others_replaced() {
    let (logs, _guard) = capture();
    let host = Host::start(&[], None).await;
    let tmp = tempfile::tempdir().unwrap();
    let m = tmp.path();
    let put = |p: &str, bytes: &[u8]| {
        let path = m.join(p);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    };
    // Verified: kept.
    put(layout::VISION, &fake(layout::VISION));
    put(layout::OCR_REC, &fake(layout::OCR_REC));
    // The right size, other bytes.
    let mut other = fake(layout::CLIP_TEXT);
    other[3] ^= 1;
    put(layout::CLIP_TEXT, &other);
    // Another size.
    put(layout::CLIP_TOKENIZER, b"{}");
    // A crash left the whole file as a part file: it is not the model.
    put(&format!("{}.part", layout::OCR_DET), &fake(layout::OCR_DET));

    let srv = server(&host, m, |_| {}).await;
    settled(&srv).await;
    assert_eq!(srv.state.ai_status(), AiStatus::Ready);
    assert_eq!(host.hits(), strings(&[OCR[0], ENGLISH[1], ENGLISH[2]]));
    for p in ENGLISH.iter().chain(&OCR) {
        assert_eq!(std::fs::read(m.join(p)).unwrap(), fake(p), "{p}");
    }
    assert_eq!(
        tree(m),
        sorted(strings(&[ENGLISH[0], ENGLISH[1], ENGLISH[2], OCR[0], OCR[1]]))
    );
    let text = logs.text();
    assert!(
        text.contains(&format!(
            "{} does not match the sha256 of the pinned model file: downloading it again",
            m.join(layout::CLIP_TEXT).display()
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "{} is 2 bytes, not the {} of the pinned model file",
            m.join(layout::CLIP_TOKENIZER).display(),
            host.row(layout::CLIP_TOKENIZER).size
        )),
        "{text}"
    );
    assert!(text.contains("downloading 3 model files"), "{text}");
}

/// Two servers on one model dir: whoever holds a part file downloads it,
/// the other waits and keeps what it finds in place.
#[tokio::test]
async fn two_servers_share_one_model_dir() {
    let (logs, _guard) = capture();
    let host = Host::start(&[], Some(layout::VISION)).await;
    let tmp = tempfile::tempdir().unwrap();
    let models: PathBuf = tmp.path().join("models");
    let first = server(&host, &models, |_| {}).await;
    wait_until("the first server's image tower", || {
        host.hits().contains(&layout::VISION.to_string())
    })
    .await;
    let second = server(&host, &models, |_| {}).await;
    wait_until("the second server to wait", || {
        logs.text().contains("another process is downloading")
    })
    .await;
    host.open.send(true).unwrap();
    settled(&first).await;
    settled(&second).await;
    assert_eq!(first.state.ai_status(), AiStatus::Ready);
    assert_eq!(second.state.ai_status(), AiStatus::Ready);
    assert!(first.state.text_reader().is_some() && second.state.text_reader().is_some());
    assert_eq!(
        sorted(host.hits()),
        sorted(strings(&[ENGLISH[0], ENGLISH[1], ENGLISH[2], OCR[0], OCR[1]])),
        "each file downloaded once"
    );
    for p in ENGLISH.iter().chain(&OCR) {
        assert_eq!(std::fs::read(models.join(p)).unwrap(), fake(p), "{p}");
    }
    assert_eq!(
        tree(&models),
        sorted(strings(&[ENGLISH[0], ENGLISH[1], ENGLISH[2], OCR[0], OCR[1]]))
    );
    assert!(
        logs.text()
            .contains("clip-vit-b32/vision.onnx is in place already (downloaded by another process)"),
        "{}",
        logs.text()
    );
}
