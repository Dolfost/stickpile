//! `--demo DIR`: a backend that serves the media files of one directory, so
//! the popup, the clipboard offers and drag-and-drop can be exercised without
//! a server (and before `stickpile-core` is complete). Search is a plain
//! substring match over file names; offers follow the same `clipboard`
//! config as the real client.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use stickpile_api::{ItemSummary, MediaFormat, MediaKind, SearchHit, TargetApp, Variant, VariantInfo};
use stickpile_core::clipboard::ClipOffer;
use stickpile_core::config::ClipboardConfig;
use stickpile_core::PreparedCopy;
use stickpile_quick::model::Results;

use crate::backend::{Backend, BoxFut};
use crate::offers;

struct DemoItem {
    summary: ItemSummary,
    path: PathBuf,
}

pub struct DemoBackend {
    items: Vec<DemoItem>,
    recent: Mutex<Vec<String>>,
    clipboard: ClipboardConfig,
}

pub fn format_of(path: &Path) -> Option<MediaFormat> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => MediaFormat::Png,
        "jpg" | "jpeg" => MediaFormat::Jpeg,
        "webp" => MediaFormat::Webp,
        "gif" => MediaFormat::Gif,
        "mp4" => MediaFormat::Mp4,
        "webm" => MediaFormat::Webm,
        _ => return None,
    })
}

fn kind_of(format: MediaFormat) -> MediaKind {
    match format {
        MediaFormat::Gif => MediaKind::Animation,
        MediaFormat::Mp4 | MediaFormat::Webm => MediaKind::Video,
        _ => MediaKind::Still,
    }
}

fn summary_for(path: &Path, format: MediaFormat) -> ItemSummary {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let (width, height) = image::image_dimensions(path).unwrap_or((0, 0));
    let mut variants = vec![VariantInfo {
        variant: Variant::Original,
        mime: format.mime().into(),
        width,
        height,
        size_bytes: size,
    }];
    // A 512 px WebP already is a Telegram static sticker.
    if format == MediaFormat::Webp && width.max(height) == 512 && size <= 512 * 1024 {
        variants.push(VariantInfo {
            variant: Variant::TgSticker,
            mime: "image/webp".into(),
            width,
            height,
            size_bytes: size,
        });
    }
    ItemSummary {
        id: file_name.clone(),
        set_id: "demo".into(),
        set_slug: "demo".into(),
        set_name: "Demo directory".into(),
        owner_username: "demo".into(),
        name: stem.clone(),
        keywords: stem
            .split(['-', '_', ' ', '.'])
            .filter(|s| !s.is_empty())
            .map(str::to_lowercase)
            .collect(),
        emojis: vec![],
        vibe: None,
        tags: vec![],
        kind: kind_of(format),
        format,
        width,
        height,
        quality: None,
        likes: 0,
        copies: 0,
        media_id: file_name,
        sha256: String::new(),
        phash: String::new(),
        variants,
        updated_at: 0,
    }
}

impl DemoBackend {
    /// Index the media files directly inside `dir` (sorted by name).
    pub fn scan(dir: &Path, clipboard: ClipboardConfig) -> std::io::Result<DemoBackend> {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file() && format_of(p).is_some())
            .collect();
        paths.sort();
        let items = paths
            .into_iter()
            .map(|path| {
                let path = std::path::absolute(&path).unwrap_or(path);
                DemoItem {
                    summary: summary_for(&path, format_of(&path).expect("filtered")),
                    path,
                }
            })
            .collect();
        Ok(DemoBackend {
            items,
            recent: Mutex::new(Vec::new()),
            clipboard,
        })
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn find(&self, id: &str) -> Option<&DemoItem> {
        self.items.iter().find(|i| i.summary.id == id)
    }

    pub fn search_now(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        self.items
            .iter()
            .filter(|i| {
                let hay = format!("{} {}", i.summary.name.to_lowercase(), i.summary.keywords.join(" "));
                words.iter().all(|w| hay.contains(w.as_str()))
            })
            .take(limit)
            .map(|i| SearchHit {
                item: i.summary.clone(),
                score: 1.0,
                matched: vec!["name".into()],
                incompatible: None,
            })
            .collect()
    }

    pub fn recent_now(&self, limit: usize) -> Vec<ItemSummary> {
        let ids = self.recent.lock().expect("not poisoned");
        ids.iter()
            .filter_map(|id| self.find(id))
            .take(limit)
            .map(|i| i.summary.clone())
            .collect()
    }

    pub fn prepare_now(&self, item: &ItemSummary, target: TargetApp, sticker: bool) -> Result<PreparedCopy, String> {
        let demo = self
            .find(&item.id)
            .ok_or_else(|| format!("{} is not in the demo directory", item.id))?;
        let has_sticker = demo.summary.variants.iter().any(|v| v.variant == Variant::TgSticker);
        let wanted_sticker = sticker;
        let sticker = sticker && has_sticker;
        let mimes = self.clipboard.offers(target).for_kind(demo.summary.kind, sticker);
        let bytes = std::fs::read(&demo.path).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        let mut skipped = Vec::new();
        for mime in mimes {
            let data = match mime.as_str() {
                m if m == demo.summary.format.mime() => Some(bytes.clone()),
                "image/png" if demo.summary.kind != MediaKind::Video => to_png(&bytes),
                m => offers::file_ref_payload(m, &demo.path),
            };
            match data {
                Some(data) => out.push(ClipOffer {
                    mime: mime.clone(),
                    data,
                }),
                None => skipped.push(mime.as_str()),
            }
        }
        let mut warnings = Vec::new();
        if wanted_sticker && !sticker {
            warnings.push("no 512 px WebP sticker rendition, copied as a normal item".to_string());
        }
        if !skipped.is_empty() {
            warnings.push(format!("cannot offer {}", skipped.join(", ")));
        }
        let warning = (!warnings.is_empty()).then(|| format!("demo: {}", warnings.join("; ")));
        Ok(PreparedCopy {
            item_id: item.id.clone(),
            target,
            variant: if sticker { Variant::TgSticker } else { Variant::Original },
            sticker,
            file: demo.path.clone(),
            offers: out,
            warning,
        })
    }

    pub fn record_now(&self, item_id: &str) {
        let mut ids = self.recent.lock().expect("not poisoned");
        ids.retain(|id| id != item_id);
        ids.insert(0, item_id.to_string());
        ids.truncate(200);
    }
}

fn to_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory(bytes).ok()?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

impl Backend for DemoBackend {
    fn login_required(&self) -> bool {
        false
    }

    fn search(self: Arc<Self>, query: String, _target: TargetApp, limit: usize) -> BoxFut<Result<Results, String>> {
        Box::pin(async move {
            Ok(Results {
                hits: self.search_now(&query, limit),
                offline: false,
            })
        })
    }

    fn recent(self: Arc<Self>, limit: usize) -> BoxFut<Vec<ItemSummary>> {
        Box::pin(async move { self.recent_now(limit) })
    }

    fn thumbnail(self: Arc<Self>, item: ItemSummary) -> BoxFut<Result<PathBuf, String>> {
        Box::pin(async move {
            let demo = self.find(&item.id).ok_or("unknown item")?;
            if demo.summary.kind == MediaKind::Video {
                return Err("demo mode has no video thumbnails".into());
            }
            Ok(demo.path.clone())
        })
    }

    fn prepare_copy(
        self: Arc<Self>,
        item: ItemSummary,
        target: TargetApp,
        sticker: bool,
    ) -> BoxFut<Result<PreparedCopy, String>> {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || self.prepare_now(&item, target, sticker))
                .await
                .map_err(|e| e.to_string())?
        })
    }

    fn record_copy(self: Arc<Self>, item: ItemSummary, _prepared: PreparedCopy) -> BoxFut<Result<(), String>> {
        Box::pin(async move {
            self.record_now(&item.id);
            Ok(())
        })
    }

    fn sync(self: Arc<Self>) -> BoxFut<Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thumbs::tests::encoded;
    use image::ImageFormat;

    fn fixture() -> (tempfile::TempDir, DemoBackend) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("naahh-cat.png"), encoded(64, 64, ImageFormat::Png)).unwrap();
        std::fs::write(dir.path().join("dance_party.gif"), encoded(32, 32, ImageFormat::Gif)).unwrap();
        std::fs::write(dir.path().join("sticker.webp"), encoded(512, 256, ImageFormat::WebP)).unwrap();
        std::fs::write(dir.path().join("clip.mp4"), b"not really mp4").unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"ignored").unwrap();
        let backend = DemoBackend::scan(dir.path(), ClipboardConfig::default()).unwrap();
        (dir, backend)
    }

    #[test]
    fn scans_media_files_only() {
        let (_d, b) = fixture();
        assert_eq!(b.len(), 4);
        let s = &b.items[1].summary;
        assert_eq!(s.id, "dance_party.gif");
        assert_eq!(s.kind, MediaKind::Animation);
        assert_eq!(s.keywords, ["dance", "party"]);
        assert_eq!((s.width, s.height), (32, 32));
        let webp = b.find("sticker.webp").unwrap();
        assert!(webp.summary.variants.iter().any(|v| v.variant == Variant::TgSticker));
        assert_eq!(b.find("clip.mp4").unwrap().summary.kind, MediaKind::Video);
    }

    #[test]
    fn substring_search() {
        let (_d, b) = fixture();
        let ids = |q| b.search_now(q, 10).into_iter().map(|h| h.item.id).collect::<Vec<_>>();
        assert_eq!(ids("NAAHH"), ["naahh-cat.png"]);
        assert_eq!(ids("party dance"), ["dance_party.gif"]);
        assert!(ids("zebra").is_empty());
        assert_eq!(b.search_now("", 2).len(), 2);
    }

    #[test]
    fn still_offers_follow_default_config() {
        let (_d, b) = fixture();
        let item = b.find("naahh-cat.png").unwrap().summary.clone();
        let p = b.prepare_now(&item, TargetApp::Discord, false).unwrap();
        let mimes: Vec<_> = p.offers.iter().map(|o| o.mime.as_str()).collect();
        assert_eq!(mimes, ["image/png", "text/uri-list", "x-special/gnome-copied-files"]);
        assert!(p.offers[0].data.starts_with(b"\x89PNG"));
        assert!(String::from_utf8_lossy(&p.offers[1].data).ends_with("naahh-cat.png\r\n"));
        assert_eq!(p.warning, None);
        assert!(!p.sticker);
    }

    #[test]
    fn gif_and_video_offers() {
        let (_d, b) = fixture();
        let gif = b.find("dance_party.gif").unwrap().summary.clone();
        let p = b.prepare_now(&gif, TargetApp::Discord, false).unwrap();
        assert_eq!(p.offers[0].mime, "image/gif");
        assert!(p.offers[0].data.starts_with(b"GIF8"));
        let mp4 = b.find("clip.mp4").unwrap().summary.clone();
        let p = b.prepare_now(&mp4, TargetApp::Telegram, false).unwrap();
        let mimes: Vec<_> = p.offers.iter().map(|o| o.mime.as_str()).collect();
        assert_eq!(mimes, ["text/uri-list", "x-special/gnome-copied-files"]);
    }

    #[test]
    fn webp_still_converted_to_png_and_sticker_route() {
        let (_d, b) = fixture();
        let webp = b.find("sticker.webp").unwrap().summary.clone();
        let p = b.prepare_now(&webp, TargetApp::Telegram, false).unwrap();
        assert_eq!(p.offers[0].mime, "image/png");
        assert!(p.offers[0].data.starts_with(b"\x89PNG"));
        let s = b.prepare_now(&webp, TargetApp::Telegram, true).unwrap();
        assert!(s.sticker);
        assert_eq!(s.variant, Variant::TgSticker);
        assert_eq!(s.offers[0].mime, "text/uri-list");
        // No sticker rendition: falls back to the plain route.
        let png = b.find("naahh-cat.png").unwrap().summary.clone();
        let p = b.prepare_now(&png, TargetApp::Telegram, true).unwrap();
        assert!(!p.sticker);
        assert!(p.warning.unwrap().contains("no 512 px WebP sticker"));
    }

    #[test]
    fn unsupported_offers_produce_a_warning() {
        let (_d, mut b) = fixture();
        b.clipboard.default.still = vec!["image/gif".into(), "text/plain".into()];
        let item = b.find("naahh-cat.png").unwrap().summary.clone();
        let p = b.prepare_now(&item, TargetApp::Generic, false).unwrap();
        assert_eq!(p.offers.len(), 1);
        assert_eq!(p.offers[0].mime, "text/plain");
        assert_eq!(p.warning.as_deref(), Some("demo: cannot offer image/gif"));
    }

    #[test]
    fn recent_is_mru() {
        let (_d, b) = fixture();
        assert!(b.recent_now(10).is_empty());
        b.record_now("clip.mp4");
        b.record_now("sticker.webp");
        b.record_now("clip.mp4");
        let ids: Vec<_> = b.recent_now(10).into_iter().map(|i| i.id).collect();
        assert_eq!(ids, ["clip.mp4", "sticker.webp"]);
        assert_eq!(b.recent_now(1).len(), 1);
    }

    #[test]
    fn async_trait_methods() {
        let (_d, b) = fixture();
        let b = Arc::new(b);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            assert!(!b.login_required());
            let r = b.clone().search("cat".into(), TargetApp::Generic, 5).await.unwrap();
            assert_eq!(r.hits.len(), 1);
            let item = r.hits[0].item.clone();
            assert!(b
                .clone()
                .thumbnail(item.clone())
                .await
                .unwrap()
                .ends_with("naahh-cat.png"));
            let p = b
                .clone()
                .prepare_copy(item.clone(), TargetApp::Generic, false)
                .await
                .unwrap();
            b.clone().record_copy(item, p).await.unwrap();
            assert_eq!(b.clone().recent(5).await.len(), 1);
            b.clone().sync().await.unwrap();
            let mp4 = b.find("clip.mp4").unwrap().summary.clone();
            assert!(b.clone().thumbnail(mp4).await.is_err());
        });
    }
}
