//! The model files the configuration needs, and downloading them into
//! `ai.model_dir` at start (`ai.download_models`, docs/ai.md).
//!
//! Each file streams into `<path>.part` next to its place, is checked
//! against its pinned size and sha256 on the way, and is renamed into place
//! only when both match; a part file never counts as the model. A server
//! holds an exclusive lock on the part file while it downloads, so servers
//! sharing a model dir take turns, and the one that waited finds the file
//! in place and keeps it.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use stickpile_ai::{Enricher, TextEncoderKind, TextReader};
use tokio::io::AsyncWriteExt;

use crate::config::{AiConfig, TextEncoder};

/// How often the log says how far a download is.
const REPORT_EVERY: Duration = Duration::from_secs(5);

/// One model file: its place below the model dir, where it comes from and
/// what it must be. An owned [`stickpile_ai::ModelFile`], so tests can serve
/// their own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSource {
    pub path: String,
    pub url: String,
    /// Lowercase hex.
    pub sha256: String,
    pub size: u64,
}

impl From<&stickpile_ai::ModelFile> for ModelSource {
    fn from(f: &stickpile_ai::ModelFile) -> ModelSource {
        ModelSource {
            path: f.path.to_string(),
            url: f.url.to_string(),
            sha256: f.sha256.to_string(),
            size: f.size,
        }
    }
}

/// Where the server downloads model files from and how hard it tries.
/// [`Default`]: [`stickpile_ai::MODEL_FILES`], four attempts per file, 10 s
/// after the first failure, doubling.
#[derive(Clone)]
pub struct ModelSources {
    pub files: Vec<ModelSource>,
    /// Attempts per file before the server gives up until the next start.
    pub attempts: u32,
    /// Wait after the first failed attempt; it doubles after each one.
    pub retry_wait: Duration,
    /// Tests: used instead of loading CLIP once its files are in place.
    #[doc(hidden)]
    pub enricher: Option<Arc<dyn Enricher>>,
    /// Tests: used instead of loading the OCR models once they are in place.
    #[doc(hidden)]
    pub text_reader: Option<Arc<dyn TextReader>>,
}

impl Default for ModelSources {
    fn default() -> ModelSources {
        ModelSources {
            files: stickpile_ai::MODEL_FILES.iter().map(ModelSource::from).collect(),
            attempts: 4,
            retry_wait: Duration::from_secs(10),
            enricher: None,
            text_reader: None,
        }
    }
}

impl ModelSources {
    /// Nothing to download: the test harness's default, so that no test
    /// reaches the network by accident.
    pub fn none() -> ModelSources {
        ModelSources {
            files: Vec::new(),
            ..ModelSources::default()
        }
    }

    /// The rows for `paths` (layout paths), in table order.
    fn rows(&self, paths: &[&str]) -> Vec<ModelSource> {
        self.files
            .iter()
            .filter(|f| paths.contains(&f.path.as_str()))
            .cloned()
            .collect()
    }
}

pub fn text_encoder_kind(t: TextEncoder) -> TextEncoderKind {
    match t {
        TextEncoder::Clip => TextEncoderKind::Clip,
        TextEncoder::Multilingual => TextEncoderKind::Multilingual,
    }
}

/// The CLIP files of `ai.text_encoder` (the image tower and that text
/// tower), in a build with CLIP.
fn clip_paths(ai: &AiConfig) -> &'static [&'static str] {
    if cfg!(feature = "clip") {
        text_encoder_kind(ai.text_encoder).required_files()
    } else {
        &[]
    }
}

/// The OCR files when `ai.ocr`, in a build with OCR.
fn ocr_paths(ai: &AiConfig) -> &'static [&'static str] {
    if ai.ocr && cfg!(feature = "ocr") {
        &stickpile_ai::OCR_FILES
    } else {
        &[]
    }
}

/// Decimal megabytes with one decimal, as the docs give model sizes.
pub fn mb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1e6)
}

/// "1 model file", "5 model files".
fn model_files(n: usize) -> String {
    if n == 1 {
        "1 model file".into()
    } else {
        format!("{n} model files")
    }
}

fn total(files: &[ModelSource]) -> u64 {
    files.iter().map(|f| f.size).sum()
}

/// The files of `files` that are missing below `dir` or have another size
/// (no hashing: cheap enough for `check-config` and every start).
pub fn missing(dir: &Path, files: &[ModelSource]) -> Vec<ModelSource> {
    files
        .iter()
        .filter(|f| !std::fs::metadata(dir.join(&f.path)).is_ok_and(|m| m.is_file() && m.len() == f.size))
        .cloned()
        .collect()
}

/// What is at a model file's place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Found {
    Verified,
    Missing,
    /// Another size (bytes).
    Size(u64),
    /// The right size, another sha256.
    Hash,
}

/// Blocking: reads the whole file when its size is right.
fn inspect(path: &Path, f: &ModelSource) -> Found {
    match std::fs::metadata(path) {
        Ok(m) if m.is_file() && m.len() == f.size => match sha256_file(path) {
            Ok(h) if h == f.sha256 => Found::Verified,
            _ => Found::Hash,
        },
        Ok(m) if m.is_file() => Found::Size(m.len()),
        _ => Found::Missing,
    }
}

async fn inspect_async(path: PathBuf, f: ModelSource) -> Found {
    tokio::task::spawn_blocking(move || inspect(&path, &f))
        .await
        .unwrap_or(Found::Missing)
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// The files of `files` that are not in place as pinned below `dir`:
/// missing, another size or another sha256 (each file of the right size is
/// read once, on the blocking pool). One log line for each that is there but
/// wrong, since it gets replaced.
pub async fn stale(dir: &Path, files: &[ModelSource]) -> Vec<ModelSource> {
    let mut out = Vec::new();
    for f in files {
        let path = dir.join(&f.path);
        match inspect_async(path.clone(), f.clone()).await {
            Found::Verified => {}
            Found::Missing => out.push(f.clone()),
            Found::Size(n) => {
                tracing::warn!(
                    "{} is {n} bytes, not the {} of the pinned model file: downloading it again",
                    path.display(),
                    f.size
                );
                out.push(f.clone());
            }
            Found::Hash => {
                tracing::warn!(
                    "{} does not match the sha256 of the pinned model file: downloading it again",
                    path.display()
                );
                out.push(f.clone());
            }
        }
    }
    out
}

/// Why the server does not download model files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocked {
    /// `ai.download_models = false`.
    Off,
    /// `ai.model_dir` cannot be created or written to (the reason).
    NotWritable(String),
}

impl std::fmt::Display for Blocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Blocked::Off => f.write_str("ai.download_models = false"),
            Blocked::NotWritable(why) => f.write_str(why),
        }
    }
}

/// Whether a file can be made in `dir` (a probe file, removed at once).
fn writable(dir: &Path) -> Result<(), Blocked> {
    let probe = dir.join(format!(".stickpile-write-test-{}", crate::ids::random_base32(6)));
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(e) => Err(Blocked::NotWritable(format!("{} is not writable: {e}", dir.display()))),
    }
}

/// At start: whether the server may download into `ai.model_dir`, which it
/// creates when it may.
fn may_download(ai: &AiConfig) -> Result<(), Blocked> {
    if !ai.download_models {
        return Err(Blocked::Off);
    }
    std::fs::create_dir_all(&ai.model_dir)
        .map_err(|e| Blocked::NotWritable(format!("{} cannot be created: {e}", ai.model_dir.display())))?;
    writable(&ai.model_dir)
}

/// The model files a start loads, and whether the server may fetch them.
#[derive(Debug, Default)]
pub struct Plan {
    /// The CLIP files of the text encoder, when CLIP loads from files.
    pub clip: Vec<ModelSource>,
    /// The OCR files, when OCR loads from files.
    pub ocr: Vec<ModelSource>,
    /// Why nothing is downloaded; `None`: missing and wrong files are.
    pub blocked: Option<Blocked>,
}

/// At start, before loading: the files that `clip` / `ocr` (the backends
/// that load from `ai.model_dir`) need and whether they can be downloaded.
/// Creates the model dir when downloads are on. Logs one line when files
/// are missing and are not downloaded. Cheap: no hashing.
pub fn plan(ai: &AiConfig, sources: &ModelSources, clip: bool, ocr: bool) -> Plan {
    let mut plan = Plan {
        clip: if ai.enabled && clip {
            sources.rows(clip_paths(ai))
        } else {
            Vec::new()
        },
        ocr: if ai.enabled && ocr {
            sources.rows(ocr_paths(ai))
        } else {
            Vec::new()
        },
        blocked: None,
    };
    if plan.clip.is_empty() && plan.ocr.is_empty() {
        return plan;
    }
    if let Err(why) = may_download(ai) {
        let all: Vec<ModelSource> = plan.clip.iter().chain(&plan.ocr).cloned().collect();
        let gone = missing(&ai.model_dir, &all);
        if !gone.is_empty() {
            tracing::info!(
                "not downloading the {} missing in {} ({} MB): {why}",
                model_files(gone.len()),
                ai.model_dir.display(),
                mb(total(&gone))
            );
        }
        plan.blocked = Some(why);
    }
    plan
}

/// What `check-config` says about the model files of `ai`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigNote {
    /// A `warning:` (something stays off) rather than a `note:`.
    pub warning: bool,
    pub text: String,
}

/// For `check-config`: the files the configuration needs that are missing
/// or of another size in `ai.model_dir`, and whether the server downloads
/// them at start (and how much) or why not. Creates nothing; sha256 is
/// checked at start only.
pub fn check_config_notes(ai: &AiConfig) -> Vec<ConfigNote> {
    if !ai.enabled {
        return Vec::new();
    }
    let table = ModelSources::default();
    let dir = &ai.model_dir;
    let clip = missing(dir, &table.rows(clip_paths(ai)));
    let ocr = missing(dir, &table.rows(ocr_paths(ai)));
    if clip.is_empty() && ocr.is_empty() {
        return Vec::new();
    }
    let list = |files: &[ModelSource]| -> String {
        files
            .iter()
            .map(|f| format!("  {} ({} MB)", dir.join(&f.path).display(), mb(f.size)))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let blocked = if !ai.download_models {
        Some(Blocked::Off)
    } else if dir.exists() {
        writable(dir).err()
    } else {
        None
    };
    match blocked {
        None => {
            let all: Vec<ModelSource> = clip.iter().chain(&ocr).cloned().collect();
            let mut hosts: Vec<&str> = Vec::new();
            for f in &all {
                let host = f.url.split('/').nth(2).unwrap_or(&f.url);
                if !hosts.contains(&host) {
                    hosts.push(host);
                }
            }
            let yet = if dir.exists() { "" } else { ", which does not exist yet" };
            vec![ConfigNote {
                warning: false,
                text: format!(
                    "ai.enabled = true and {} {} missing or the wrong size in {}{yet}: the server \
                     downloads {} MB from {} in the background when it starts, checks the sha256 and then \
                     loads the models (ai.download_models; docs/ai.md):\n{}",
                    model_files(all.len()),
                    if all.len() == 1 { "is" } else { "are" },
                    dir.display(),
                    mb(total(&all)),
                    hosts.join(" and "),
                    list(&all)
                ),
            }]
        }
        Some(why) => {
            let mut out = Vec::new();
            if !clip.is_empty() {
                out.push(ConfigNote {
                    warning: true,
                    text: format!(
                        "ai.enabled = true but model files are missing or the wrong size, and the server does \
                         not download them ({why}): AI stays off until they exist (see docs/ai.md, \
                         `scripts/fetch-models.sh`):\n{}",
                        list(&clip)
                    ),
                });
            }
            if !ocr.is_empty() {
                out.push(ConfigNote {
                    warning: true,
                    text: format!(
                        "ai.ocr = true but the OCR model files are missing or the wrong size, and the server \
                         does not download them ({why}): the words in pictures are not read until they exist \
                         (see docs/ai.md, `scripts/fetch-models.sh`):\n{}",
                        list(&ocr)
                    ),
                });
            }
            out
        }
    }
}

/// Bytes of the model download in progress, for the admin page
/// ([`crate::state::AppState::model_download`]).
#[derive(Debug, Default)]
pub struct Progress {
    done: AtomicU64,
    total: AtomicU64,
}

impl Progress {
    /// `(done, total)` bytes while the server downloads model files.
    pub fn get(&self) -> Option<(u64, u64)> {
        let total = self.total.load(Ordering::Acquire);
        (total > 0).then(|| (self.done.load(Ordering::Acquire).min(total), total))
    }
    fn add(&self, n: u64) {
        self.done.fetch_add(n, Ordering::AcqRel);
    }
    fn sub(&self, n: u64) {
        let _ = self
            .done
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |d| Some(d.saturating_sub(n)));
    }
}

/// How one attempt at one file ended well.
enum Outcome {
    /// Downloaded, verified and renamed into place.
    Installed,
    /// Another process put it in place meanwhile.
    Kept,
}

/// One download pass at start: the files one after the other, each tried
/// [`ModelSources::attempts`] times.
pub struct Downloader<'a> {
    sources: &'a ModelSources,
    dir: &'a Path,
    progress: &'a Progress,
    client: reqwest::Client,
    started: Instant,
    last_report: Instant,
    files: usize,
    installed: usize,
    failed: usize,
}

impl<'a> Downloader<'a> {
    /// Logs what is about to be downloaded and starts the progress.
    pub fn start(
        sources: &'a ModelSources,
        dir: &'a Path,
        progress: &'a Progress,
        files: &[ModelSource],
    ) -> anyhow::Result<Downloader<'a>> {
        // No overall timeout: the CLIP image tower is 350 MB. A stalled
        // connection ends with the read timeout.
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .read_timeout(Duration::from_secs(60))
            .user_agent(concat!("stickpile/", env!("CARGO_PKG_VERSION")))
            .build()?;
        let bytes = total(files);
        tracing::info!(
            "downloading {}, {} MB, to {}",
            model_files(files.len()),
            mb(bytes),
            dir.display()
        );
        progress.done.store(0, Ordering::Release);
        progress.total.store(bytes, Ordering::Release);
        let now = Instant::now();
        Ok(Downloader {
            sources,
            dir,
            progress,
            client,
            started: now,
            last_report: now,
            files: files.len(),
            installed: 0,
            failed: 0,
        })
    }

    /// Download `files` one after the other; true when each is in place.
    pub async fn fetch(&mut self, files: &[ModelSource]) -> bool {
        let mut all = true;
        for f in files {
            if self.fetch_one(f).await {
                self.installed += 1;
            } else {
                self.failed += 1;
                all = false;
            }
        }
        all
    }

    /// Log the result and end the progress.
    pub fn finish(self) {
        self.progress.total.store(0, Ordering::Release);
        let secs = self.started.elapsed().as_secs_f64();
        if self.failed == 0 {
            tracing::info!("downloaded and verified {} in {secs:.0} s", model_files(self.installed));
        } else {
            tracing::error!(
                "{} of {} could not be downloaded; the server tries again at its next start",
                self.failed,
                model_files(self.files)
            );
        }
    }

    async fn fetch_one(&mut self, f: &ModelSource) -> bool {
        let dest = self.dir.join(&f.path);
        let attempts = self.sources.attempts.max(1);
        for attempt in 1..=attempts {
            let started = Instant::now();
            match self.attempt(f, &dest).await {
                Ok(Outcome::Installed) => {
                    tracing::info!(
                        "downloaded {} ({} MB) in {:.0} s",
                        f.path,
                        mb(f.size),
                        started.elapsed().as_secs_f64()
                    );
                    return true;
                }
                Ok(Outcome::Kept) => {
                    tracing::info!("{} is in place already (downloaded by another process)", f.path);
                    return true;
                }
                Err(e) if attempt < attempts => {
                    let wait = self.sources.retry_wait * 2u32.saturating_pow(attempt - 1);
                    tracing::warn!(
                        "downloading {} from {} failed (attempt {attempt} of {attempts}): {e}; trying again in {:.0} s",
                        f.path,
                        f.url,
                        wait.as_secs_f64()
                    );
                    tokio::time::sleep(wait).await;
                }
                Err(e) => {
                    tracing::error!(
                        "could not download {} from {} ({attempts} attempts; the last: {e}); giving up until the next start",
                        f.path,
                        f.url
                    );
                }
            }
        }
        false
    }

    async fn attempt(&mut self, f: &ModelSource, dest: &Path) -> Result<Outcome, String> {
        let parent = dest.parent().unwrap_or(self.dir);
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("creating {}: {e}", parent.display()))?;
        let part = part_path(dest);
        let poll = self.sources.retry_wait.min(Duration::from_secs(1));
        let file = lock_part(&part, poll)
            .await
            .map_err(|e| format!("opening {}: {e}", part.display()))?;
        // Another process may have put it in place while this one waited.
        if inspect_async(dest.to_path_buf(), f.clone()).await == Found::Verified {
            let _ = tokio::fs::remove_file(&part).await;
            self.progress.add(f.size);
            return Ok(Outcome::Kept);
        }
        let mut file = tokio::fs::File::from_std(file);
        let mut counted = 0;
        let written = self.stream(f, &part, &mut file, &mut counted).await;
        let installed = match written {
            Ok(()) => tokio::fs::rename(&part, dest)
                .await
                .map_err(|e| format!("moving {} into place: {e}", part.display())),
            Err(e) => Err(e),
        };
        match installed {
            Ok(()) => {
                if let Ok(d) = std::fs::File::open(parent) {
                    let _ = d.sync_all();
                }
                Ok(Outcome::Installed)
            }
            Err(e) => {
                self.progress.sub(counted);
                let _ = tokio::fs::remove_file(&part).await;
                Err(e)
            }
        }
    }

    /// GET `f.url` into `file` (truncated first), counting and hashing on
    /// the way; Ok only with exactly `f.size` bytes of `f.sha256`.
    async fn stream(
        &mut self,
        f: &ModelSource,
        part: &Path,
        file: &mut tokio::fs::File,
        counted: &mut u64,
    ) -> Result<(), String> {
        let write_err = |e: std::io::Error| format!("writing {}: {e}", part.display());
        file.set_len(0).await.map_err(write_err)?;
        let mut resp = self.client.get(&f.url).send().await.map_err(http_err)?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("HTTP {status}"));
        }
        if let Some(n) = resp.content_length().filter(|n| *n != f.size) {
            return Err(format!("the server sends {n} bytes, not the pinned {}", f.size));
        }
        let mut hasher = Sha256::new();
        let mut len = 0u64;
        while let Some(chunk) = resp.chunk().await.map_err(http_err)? {
            len += chunk.len() as u64;
            if len > f.size {
                return Err(format!("the server sends more than the pinned {} bytes", f.size));
            }
            hasher.update(&chunk);
            file.write_all(&chunk).await.map_err(write_err)?;
            *counted += chunk.len() as u64;
            self.progress.add(chunk.len() as u64);
            if self.last_report.elapsed() >= REPORT_EVERY {
                self.last_report = Instant::now();
                if let Some((done, total)) = self.progress.get() {
                    tracing::info!("downloading the models: {} of {} MB", mb(done), mb(total));
                }
            }
        }
        if len != f.size {
            return Err(format!("the server sent {len} bytes, not the pinned {}", f.size));
        }
        file.flush().await.map_err(write_err)?;
        file.sync_all().await.map_err(write_err)?;
        let got = hex::encode(hasher.finalize());
        if got != f.sha256 {
            tracing::error!(
                "{} from {} does not match its pinned sha256 (expected {}, got {got}): deleted it, nothing installed",
                f.path,
                f.url,
                f.sha256
            );
            return Err("sha256 mismatch".into());
        }
        Ok(())
    }
}

/// `vision.onnx` -> `vision.onnx.part`, next to it.
fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

/// Open `part` (created if missing, not truncated) with an exclusive lock.
/// While another process holds it, wait (`poll`); then make sure the locked
/// file is still the one at `part` (the holder renames or removes it).
async fn lock_part(part: &Path, poll: Duration) -> std::io::Result<std::fs::File> {
    let mut told = false;
    loop {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(part)?;
        match file.try_lock() {
            Ok(()) => {
                let mine = file.metadata()?;
                if std::fs::metadata(part).is_ok_and(|m| m.dev() == mine.dev() && m.ino() == mine.ino()) {
                    return Ok(file);
                }
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                if !told {
                    tracing::info!("another process is downloading {}; waiting for it", part.display());
                    told = true;
                }
                drop(file);
                tokio::time::sleep(poll).await;
            }
            // A file system without locks: go on alone.
            Err(std::fs::TryLockError::Error(e)) if e.kind() == std::io::ErrorKind::Unsupported => return Ok(file),
            Err(std::fs::TryLockError::Error(e)) => return Err(e),
        }
    }
}

/// The error and its causes on one line, without the URL (the callers
/// name it).
fn http_err(e: reqwest::Error) -> String {
    let e = e.without_url();
    let mut out = e.to_string();
    let mut source = std::error::Error::source(&e);
    while let Some(s) = source {
        out.push_str(": ");
        out.push_str(&s.to_string());
        source = s.source();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_files_sit_next_to_their_model() {
        assert_eq!(
            part_path(Path::new("/m/clip-vit-b32/vision.onnx")),
            Path::new("/m/clip-vit-b32/vision.onnx.part")
        );
    }

    #[test]
    fn sizes_in_megabytes() {
        assert_eq!(mb(4_819_576 + 8_074_092), "12.9");
        assert_eq!(mb(0), "0.0");
    }

    #[test]
    fn the_configuration_picks_the_files() {
        let table = ModelSources::default();
        let paths = |files: Vec<ModelSource>| files.into_iter().map(|f| f.path).collect::<Vec<_>>();
        let mut ai = AiConfig {
            enabled: true,
            ..AiConfig::default()
        };
        assert_eq!(
            paths(table.rows(clip_paths(&ai))),
            [
                "clip-vit-b32/vision.onnx",
                "clip-vit-b32/text.onnx",
                "clip-vit-b32/tokenizer.json"
            ]
        );
        assert_eq!(paths(table.rows(ocr_paths(&ai))), ["ocr/det.onnx", "ocr/rec.onnx"]);
        ai.text_encoder = TextEncoder::Multilingual;
        ai.ocr = false;
        assert_eq!(
            paths(table.rows(clip_paths(&ai))),
            [
                "clip-vit-b32/vision.onnx",
                "multilingual/model.onnx",
                "multilingual/tokenizer.json",
                "multilingual/dense.safetensors"
            ]
        );
        assert!(table.rows(ocr_paths(&ai)).is_empty());
        assert!(ModelSources::none().rows(clip_paths(&ai)).is_empty());
    }

    #[test]
    fn progress_is_none_unless_downloading() {
        let p = Progress::default();
        assert_eq!(p.get(), None);
        p.total.store(10, Ordering::Release);
        p.add(4);
        assert_eq!(p.get(), Some((4, 10)));
        p.sub(9);
        assert_eq!(p.get(), Some((0, 10)));
        p.add(30);
        assert_eq!(p.get(), Some((10, 10)), "never more than the total");
    }
}
