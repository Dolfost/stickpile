//! `--demo DIR`: a backend that serves the media files of one directory, so
//! the popup, the clipboard offers and drag-and-drop can be exercised without
//! a server (and before `stickpile-core` is complete). Search is a plain
//! substring match over file names; offers follow the same `clipboard`
//! config as the real client. Each subdirectory is a pack (a directory from
//! `stickpile backup` works), the loose files are one more; favorites and
//! most used start from a fixed choice, and the account is a demo user that
//! logs in and out without a server.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use stickpile_api::{
    ItemSummary, MediaFormat, MediaKind, SearchHit, SetSource, SetSummary, TargetApp, Variant, VariantInfo, Visibility,
};
use stickpile_core::clipboard::ClipOffer;
use stickpile_core::config::ClipboardConfig;
use stickpile_core::PreparedCopy;
use stickpile_quick::model::Results;

use crate::backend::{AuthKind, Backend, BoxFut, Credentials, LoginError, Profile};
use crate::offers;

struct DemoItem {
    summary: ItemSummary,
    path: PathBuf,
}

pub struct DemoBackend {
    dir: PathBuf,
    items: Vec<DemoItem>,
    packs: Vec<SetSummary>,
    recent: Mutex<Vec<String>>,
    /// Item id -> copies.
    copies: Mutex<HashMap<String, u64>>,
    /// Liked item ids, newest like first.
    liked: Mutex<Vec<String>>,
    /// The signed-in user; `None` after logout.
    user: Mutex<Option<String>>,
    clipboard: ClipboardConfig,
}

/// Id and set fields of the loose files' pack.
const LOOSE_PACK: &str = "demo";
const DEMO_USER: &str = "demo";

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
        set_id: LOOSE_PACK.into(),
        set_slug: LOOSE_PACK.into(),
        set_name: "Demo directory".into(),
        owner_username: DEMO_USER.into(),
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

/// The media files directly inside `dir`, sorted by name.
fn media_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && format_of(p).is_some())
        .map(|p| std::path::absolute(&p).unwrap_or(p))
        .collect();
    paths.sort();
    Ok(paths)
}

fn slug_of(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

fn pack_summary(id: &str, slug: &str, name: &str, items: &[DemoItem]) -> SetSummary {
    SetSummary {
        id: id.into(),
        slug: slug.into(),
        name: name.into(),
        description: String::new(),
        owner_username: DEMO_USER.into(),
        visibility: Visibility::Private,
        source: SetSource::Local,
        item_count: items.len() as u64,
        cover_item_id: items.first().map(|i| i.summary.id.clone()),
        cover_url: None,
        updated_at: 0,
    }
}

impl DemoBackend {
    /// Index the media files directly inside `dir` and inside each of its
    /// subdirectories (sorted by name).
    pub fn scan(dir: &Path, clipboard: ClipboardConfig) -> std::io::Result<DemoBackend> {
        let mut items: Vec<DemoItem> = media_files(dir)?
            .into_iter()
            .map(|path| DemoItem {
                summary: summary_for(&path, format_of(&path).expect("filtered")),
                path,
            })
            .collect();
        let mut packs = Vec::new();
        if !items.is_empty() {
            packs.push(pack_summary(LOOSE_PACK, LOOSE_PACK, "Demo directory", &items));
        }
        let mut subdirs: Vec<PathBuf> = std::fs::read_dir(dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_dir() && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
            .collect();
        subdirs.sort();
        for sub in subdirs {
            let name = sub
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let pack: Vec<DemoItem> = media_files(&sub)?
                .into_iter()
                .map(|path| {
                    let mut summary = summary_for(&path, format_of(&path).expect("filtered"));
                    summary.id = format!("{name}/{}", summary.id);
                    summary.media_id = summary.id.clone();
                    DemoItem { summary, path }
                })
                .collect();
            if pack.is_empty() {
                continue;
            }
            let set = pack_summary(&name, &slug_of(&name), &name, &pack);
            items.extend(pack.into_iter().map(|mut i| {
                i.summary.set_id = set.id.clone();
                i.summary.set_slug = set.slug.clone();
                i.summary.set_name = set.name.clone();
                i
            }));
            packs.push(set);
        }
        // A fixed choice so the tabs are not empty on the first look.
        let liked = items.iter().step_by(3).take(6).map(|i| i.summary.id.clone()).collect();
        let copies = items
            .iter()
            .step_by(2)
            .take(5)
            .zip((1..=5u64).rev())
            .map(|(i, n)| (i.summary.id.clone(), n))
            .collect();
        Ok(DemoBackend {
            dir: std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf()),
            items,
            packs,
            recent: Mutex::new(Vec::new()),
            copies: Mutex::new(copies),
            liked: Mutex::new(liked),
            user: Mutex::new(Some(DEMO_USER.into())),
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
        *self
            .copies
            .lock()
            .expect("not poisoned")
            .entry(item_id.to_string())
            .or_default() += 1;
    }

    pub fn most_used_now(&self, limit: usize) -> Vec<ItemSummary> {
        let copies = self.copies.lock().expect("not poisoned");
        let mut used: Vec<(usize, u64)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| copies.get(&item.summary.id).map(|n| (i, *n)))
            .filter(|(_, n)| *n > 0)
            .collect();
        used.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        used.into_iter()
            .take(limit)
            .map(|(i, _)| self.items[i].summary.clone())
            .collect()
    }

    pub fn favorites_now(&self, limit: usize) -> Vec<ItemSummary> {
        let liked = self.liked.lock().expect("not poisoned");
        liked
            .iter()
            .filter_map(|id| self.find(id))
            .take(limit)
            .map(|i| i.summary.clone())
            .collect()
    }

    pub fn packs_now(&self) -> Vec<SetSummary> {
        self.packs.clone()
    }

    pub fn pack_items_now(&self, set_id: &str) -> Result<Vec<ItemSummary>, String> {
        if !self.packs.iter().any(|p| p.id == set_id) {
            return Err(format!("{set_id} is not a demo pack"));
        }
        Ok(self
            .items
            .iter()
            .filter(|i| i.summary.set_id == set_id)
            .map(|i| i.summary.clone())
            .collect())
    }

    /// The pack's first item that is not a video (demo mode has no video thumbnails).
    pub fn pack_cover_now(&self, set_id: &str) -> Option<PathBuf> {
        self.items
            .iter()
            .find(|i| i.summary.set_id == set_id && i.summary.kind != MediaKind::Video)
            .map(|i| i.path.clone())
    }

    pub fn toggle_favorite_now(&self, item_id: &str) -> Result<bool, String> {
        if self.find(item_id).is_none() {
            return Err(format!("{item_id} is not in the demo directory"));
        }
        let mut liked = self.liked.lock().expect("not poisoned");
        match liked.iter().position(|id| id == item_id) {
            Some(pos) => {
                liked.remove(pos);
                Ok(false)
            }
            None => {
                liked.insert(0, item_id.to_string());
                Ok(true)
            }
        }
    }

    pub fn profile_now(&self) -> Profile {
        let server_url = format!("demo:{}", self.dir.display());
        let user = self.user.lock().expect("not poisoned").clone();
        let Some(user) = user else {
            return Profile {
                tailnet_login_available: true,
                ..Profile::signed_out(server_url)
            };
        };
        Profile {
            auth: AuthKind::Token,
            display_name: Some(if user == DEMO_USER {
                "Demo user".into()
            } else {
                user.clone()
            }),
            username: Some(user),
            copies: Some(self.copies.lock().expect("not poisoned").values().sum()),
            likes: Some(self.liked.lock().expect("not poisoned").len() as u64),
            packs: Some(self.packs.len() as u64),
            items: Some(self.items.len() as u64),
            tailnet_login_available: true,
            ..Profile::signed_out(server_url)
        }
    }

    /// Any credentials work; `None` signs in as the demo user.
    pub fn login_now(&self, credentials: Option<Credentials>) -> Result<Profile, LoginError> {
        let user = match credentials {
            None => DEMO_USER.to_string(),
            Some(c) if c.username.trim().is_empty() => {
                return Err(LoginError::Failed("A username is required".into()));
            }
            Some(c) => c.username.trim().to_string(),
        };
        *self.user.lock().expect("not poisoned") = Some(user);
        Ok(self.profile_now())
    }

    pub fn logout_now(&self) {
        *self.user.lock().expect("not poisoned") = None;
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

    fn most_used(self: Arc<Self>, limit: usize) -> BoxFut<Result<Vec<ItemSummary>, String>> {
        Box::pin(async move { Ok(self.most_used_now(limit)) })
    }

    fn favorites(self: Arc<Self>, limit: usize) -> BoxFut<Result<Vec<ItemSummary>, String>> {
        Box::pin(async move { Ok(self.favorites_now(limit)) })
    }

    fn packs(self: Arc<Self>) -> BoxFut<Result<Vec<SetSummary>, String>> {
        Box::pin(async move { Ok(self.packs_now()) })
    }

    fn pack_items(self: Arc<Self>, set_id: String) -> BoxFut<Result<Vec<ItemSummary>, String>> {
        Box::pin(async move { self.pack_items_now(&set_id) })
    }

    fn pack_cover(self: Arc<Self>, set: SetSummary) -> BoxFut<Result<Option<PathBuf>, String>> {
        Box::pin(async move { Ok(self.pack_cover_now(&set.id)) })
    }

    fn toggle_favorite(self: Arc<Self>, item_id: String) -> BoxFut<Result<bool, String>> {
        Box::pin(async move { self.toggle_favorite_now(&item_id) })
    }

    fn profile(self: Arc<Self>) -> BoxFut<Result<Profile, String>> {
        Box::pin(async move { Ok(self.profile_now()) })
    }

    fn login(self: Arc<Self>, credentials: Option<Credentials>) -> BoxFut<Result<Profile, LoginError>> {
        Box::pin(async move { self.login_now(credentials) })
    }

    fn logout(self: Arc<Self>) -> BoxFut<Result<(), String>> {
        Box::pin(async move {
            self.logout_now();
            Ok(())
        })
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

            assert_eq!(b.clone().most_used(10).await.unwrap()[0].id, "clip.mp4");
            assert_eq!(b.clone().favorites(10).await.unwrap().len(), 2);
            let packs = b.clone().packs().await.unwrap();
            assert_eq!(packs.len(), 1);
            assert_eq!(b.clone().pack_items(packs[0].id.clone()).await.unwrap().len(), 4);
            let cover = b.clone().pack_cover(packs[0].clone()).await.unwrap().unwrap();
            assert!(
                cover.ends_with("dance_party.gif"),
                "videos have no thumbnail: {cover:?}"
            );
            assert!(b.clone().toggle_favorite("dance_party.gif".into()).await.unwrap());
            assert_eq!(b.clone().profile().await.unwrap().auth, AuthKind::Token);
            b.clone().logout().await.unwrap();
            assert_eq!(b.clone().profile().await.unwrap().auth, AuthKind::None);
            let p = b.clone().login(None).await.unwrap();
            assert_eq!(p.username.as_deref(), Some("demo"));
        });
    }

    #[test]
    fn subdirectories_are_packs() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::write(d.join("loose.png"), encoded(16, 16, ImageFormat::Png)).unwrap();
        for sub in ["Cats Etc", "dogs", ".hidden", "empty"] {
            std::fs::create_dir(d.join(sub)).unwrap();
        }
        std::fs::write(d.join("Cats Etc/a-clip.mp4"), b"not really mp4").unwrap();
        std::fs::write(d.join("Cats Etc/b-grumpy.png"), encoded(20, 20, ImageFormat::Png)).unwrap();
        std::fs::write(d.join("Cats Etc/c-dance.gif"), encoded(20, 20, ImageFormat::Gif)).unwrap();
        std::fs::write(d.join("dogs/good-dog.webp"), encoded(30, 30, ImageFormat::WebP)).unwrap();
        std::fs::write(d.join(".hidden/x.png"), encoded(8, 8, ImageFormat::Png)).unwrap();
        let b = DemoBackend::scan(d, ClipboardConfig::default()).unwrap();
        assert_eq!(b.len(), 5);

        let packs = b.packs_now();
        let ids: Vec<_> = packs.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["demo", "Cats Etc", "dogs"]);
        assert_eq!(packs[1].slug, "cats-etc");
        assert_eq!(packs.iter().map(|p| p.item_count).collect::<Vec<_>>(), [1, 3, 1]);

        let cats = b.pack_items_now("Cats Etc").unwrap();
        let ids: Vec<_> = cats.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(
            ids,
            ["Cats Etc/a-clip.mp4", "Cats Etc/b-grumpy.png", "Cats Etc/c-dance.gif"]
        );
        assert!(cats
            .iter()
            .all(|i| i.set_name == "Cats Etc" && i.set_slug == "cats-etc"));
        assert_eq!(b.pack_items_now("demo").unwrap()[0].id, "loose.png");
        assert!(b.pack_items_now("nope").is_err());
        assert!(b.pack_cover_now("Cats Etc").unwrap().ends_with("b-grumpy.png"));

        let hits = b.search_now("dog", 10);
        assert_eq!(hits[0].item.id, "dogs/good-dog.webp");
        let p = b.prepare_now(&hits[0].item, TargetApp::Generic, false).unwrap();
        assert!(p.file.ends_with("dogs/good-dog.webp"));
    }

    #[test]
    fn favorites_most_used_and_account() {
        let (_d, b) = fixture();
        let ids = |items: Vec<ItemSummary>| items.into_iter().map(|i| i.id).collect::<Vec<_>>();
        // The fixed choice: every third item liked, every second one copied.
        assert_eq!(ids(b.favorites_now(10)), ["clip.mp4", "sticker.webp"]);
        assert_eq!(ids(b.most_used_now(10)), ["clip.mp4", "naahh-cat.png"]);

        assert!(b.toggle_favorite_now("naahh-cat.png").unwrap());
        assert_eq!(ids(b.favorites_now(1)), ["naahh-cat.png"]);
        assert!(!b.toggle_favorite_now("naahh-cat.png").unwrap());
        assert_eq!(b.favorites_now(10).len(), 2);
        assert!(b.toggle_favorite_now("nope.png").is_err());

        b.record_now("naahh-cat.png");
        b.record_now("naahh-cat.png");
        b.record_now("dance_party.gif");
        assert_eq!(
            ids(b.most_used_now(10)),
            ["naahh-cat.png", "clip.mp4", "dance_party.gif"]
        );

        let p = b.profile_now();
        assert_eq!(p.auth, AuthKind::Token);
        assert_eq!(p.username.as_deref(), Some("demo"));
        assert_eq!((p.packs, p.likes, p.copies), (Some(1), Some(2), Some(12)));
        b.logout_now();
        let p = b.profile_now();
        assert_eq!((p.auth, p.username), (AuthKind::None, None));
        let creds = |username: &str| Credentials {
            username: username.into(),
            password: "pw".into(),
            totp: None,
        };
        assert!(matches!(b.login_now(Some(creds(" "))), Err(LoginError::Failed(_))));
        assert_eq!(
            b.login_now(Some(creds("alice"))).unwrap().username.as_deref(),
            Some("alice")
        );
        assert_eq!(b.login_now(None).unwrap().display_name.as_deref(), Some("Demo user"));
    }
}
