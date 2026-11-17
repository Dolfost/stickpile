//! Keeping offline (docs/offline.md, the brief's section 6.1) against the
//! mock server: what arrives, what is skipped, what is never deleted, the
//! downloader's limits, two processes on one cache, and an older client on
//! the same cache.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use stickpile_api::{ItemSummary, MediaFormat, MediaKind, Role, TargetApp, Variant, VariantInfo, Visibility};

use super::*;
use crate::cache::Cache;
use crate::offline::{
    needs, played_rendition, Device, FileRole, OfflineControl, OfflineOptions, OfflineOutcome, OfflineState, PlanScope,
    Playback, Reason, RunEnd, Scope, SkipReason,
};
use crate::session::pick_variant;

// -- helpers -------------------------------------------------------------------

/// Options with nothing automatic, so a test sees only what it keeps.
fn plain(targets: &[TargetApp]) -> OfflineOptions {
    let mut o = OfflineOptions {
        targets: targets.to_vec(),
        ..OfflineOptions::default()
    };
    o.auto.thumbnails = false;
    o
}

/// A control whose delays suit tests.
fn quick() -> OfflineControl {
    let c = OfflineControl::new();
    c.tune(|t| {
        t.backoff = Duration::from_millis(10);
        t.default_retry_after = Duration::from_millis(200);
        t.claim_poll = Duration::from_millis(30);
        t.foreground_wait = Duration::from_secs(30);
    });
    c
}

async fn run(session: &Session, control: &OfflineControl) -> OfflineOutcome {
    tokio::time::timeout(Duration::from_secs(60), session.offline_run(control))
        .await
        .expect("the run ends")
        .unwrap()
}

/// Every file under the cache's `files/` and `clip/`, relative.
fn on_disk(cache_dir: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut stack = vec![cache_dir.join("files"), cache_dir.join("clip")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.insert(p.strip_prefix(cache_dir).unwrap().to_string_lossy().into_owned());
            }
        }
    }
    out
}

fn part_files(cache_dir: &Path) -> Vec<String> {
    on_disk(cache_dir)
        .into_iter()
        .filter(|p| p.contains(".part."))
        .collect()
}

fn kept(cache: &Cache) -> BTreeSet<String> {
    cache
        .file_index()
        .unwrap()
        .into_iter()
        .filter(|(_, (_, k))| *k)
        .map(|(p, _)| p)
        .collect()
}

/// The file requests the mock got, `GET /api/v1/items/<id>/file/<variant>`.
fn file_requests(server: &MockServer) -> Vec<String> {
    server
        .state()
        .requests
        .iter()
        .filter(|r| r.contains("/file/") || r.contains("/cover/"))
        .cloned()
        .collect()
}

fn cache_dir(dir: &Path) -> PathBuf {
    dir.join("cache")
}

fn sha(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}

fn info(variant: Variant, mime: &str, size_bytes: u64) -> VariantInfo {
    VariantInfo {
        variant,
        mime: mime.into(),
        width: 512,
        height: 512,
        size_bytes,
    }
}

fn summary(kind: MediaKind, format: MediaFormat, variants: Vec<VariantInfo>) -> ItemSummary {
    ItemSummary {
        id: "item1".into(),
        set_id: "set1".into(),
        set_slug: "s".into(),
        set_name: "S".into(),
        owner_username: "alice".into(),
        name: "cat".into(),
        keywords: vec![],
        emojis: vec![],
        vibe: None,
        tags: vec![],
        text: String::new(),
        kind,
        format,
        width: 512,
        height: 512,
        quality: None,
        likes: 0,
        copies: 0,
        media_id: "m1".into(),
        sha256: String::new(),
        phash: String::new(),
        variants,
        updated_at: 0,
    }
}

// -- needs() and the played rendition --------------------------------------------

#[test]
fn needs_per_target_and_kind() {
    let opts = OfflineOptions::default();
    let still = summary(
        MediaKind::Still,
        MediaFormat::Webp,
        vec![
            info(Variant::Original, "image/webp", 1000),
            info(Variant::Thumb, "image/webp", 10),
            info(Variant::Png, "image/png", 3000),
            info(Variant::TgSticker, "image/webp", 900),
        ],
    );
    let anim = summary(
        MediaKind::Animation,
        MediaFormat::Gif,
        vec![
            info(Variant::Original, "image/gif", 5000),
            info(Variant::Thumb, "image/webp", 10),
            info(Variant::Mp4, "video/mp4", 800),
            info(Variant::DiscordGif, "image/gif", 4000),
            info(Variant::TgVideoSticker, "video/webm", 200),
        ],
    );
    let video = summary(
        MediaKind::Video,
        MediaFormat::Webm,
        vec![
            info(Variant::Original, "video/webm", 9000),
            info(Variant::Thumb, "image/webp", 10),
            info(Variant::Mp4, "video/mp4", 7000),
        ],
    );
    for item in [&still, &anim, &video] {
        for targets in [
            vec![TargetApp::Generic],
            vec![TargetApp::Telegram],
            vec![
                TargetApp::Discord,
                TargetApp::Whatsapp,
                TargetApp::Slack,
                TargetApp::Signal,
            ],
            TargetApp::ALL.to_vec(),
        ] {
            for sticker_renditions in [false, true] {
                for plays_motion in [false, true] {
                    let opts = OfflineOptions {
                        sticker_renditions,
                        ..opts.clone()
                    };
                    let device = Device {
                        targets: targets.clone(),
                        plays_motion,
                    };
                    let got = needs(item, &opts, &device);
                    // The thumbnail first, each file once.
                    assert_eq!(got[0].variant, Variant::Thumb);
                    assert_eq!(got[0].rel, "files/thumb/m1.webp");
                    let rels: BTreeSet<&str> = got.iter().map(|f| f.rel.as_str()).collect();
                    assert_eq!(rels.len(), got.len(), "no duplicates");
                    // What a copy uses, for every target and flag.
                    let mut want: BTreeSet<Variant> = BTreeSet::from([Variant::Thumb]);
                    for &t in &targets {
                        want.insert(pick_variant(item, t, false).unwrap().0.variant);
                        if sticker_renditions {
                            let (v, is_sticker, _) = pick_variant(item, t, true).unwrap();
                            if is_sticker {
                                want.insert(v.variant);
                            }
                        }
                    }
                    if plays_motion {
                        if let Some((v, _)) = played_rendition(item) {
                            want.insert(v.variant);
                        }
                    }
                    let variants: BTreeSet<Variant> = got.iter().map(|f| f.variant).collect();
                    assert_eq!(
                        variants, want,
                        "{:?} {targets:?} {sticker_renditions} {plays_motion}",
                        item.kind
                    );
                    for f in &got[1..] {
                        assert!(
                            f.rel.starts_with(&format!("clip/m1.{}/cat.", f.variant.as_str())),
                            "{}",
                            f.rel
                        );
                        let size = item
                            .variants
                            .iter()
                            .find(|v| v.variant == f.variant)
                            .unwrap()
                            .size_bytes;
                        assert_eq!(f.size, size);
                    }
                }
            }
        }
    }
    // Known choices: WhatsApp takes no WebP still (the PNG), Telegram's
    // sticker is the 512 px WebP / the video sticker, the grid plays the
    // smaller picture of an animation and the MP4 of a WebM video.
    let roles = |item: &ItemSummary, targets: &[TargetApp], plays_motion: bool| -> Vec<(Variant, Vec<FileRole>)> {
        needs(
            item,
            &OfflineOptions::default(),
            &Device {
                targets: targets.to_vec(),
                plays_motion,
            },
        )
        .into_iter()
        .map(|f| (f.variant, f.roles))
        .collect()
    };
    let wa = roles(&still, &[TargetApp::Whatsapp], false);
    assert!(wa.iter().any(|(v, _)| *v == Variant::Png), "{wa:?}");
    let tg = roles(&still, &[TargetApp::Telegram], false);
    assert!(tg.contains(&(
        Variant::TgSticker,
        vec![FileRole::Copy {
            target: TargetApp::Telegram,
            sticker: true
        }]
    )));
    let tg_anim = roles(&anim, &[TargetApp::Telegram], false);
    assert!(
        tg_anim.iter().any(|(v, _)| *v == Variant::TgVideoSticker),
        "{tg_anim:?}"
    );
    let played = roles(&anim, &[TargetApp::Generic], true);
    assert!(played
        .iter()
        .any(|(v, r)| *v == Variant::DiscordGif && r.contains(&FileRole::Played)));
    let played = roles(&video, &[TargetApp::Generic], true);
    assert!(played
        .iter()
        .any(|(v, r)| *v == Variant::Mp4 && r.contains(&FileRole::Played)));
    // Without sticker renditions, no sticker file.
    let opts = OfflineOptions {
        sticker_renditions: false,
        ..OfflineOptions::default()
    };
    let device = Device {
        targets: vec![TargetApp::Telegram],
        plays_motion: false,
    };
    assert!(!needs(&still, &opts, &device)
        .iter()
        .any(|f| f.variant == Variant::TgSticker));
}

/// The cases of the Android crate's `motion::choose` tests, unchanged: the
/// rule moved into the core must choose the same.
#[test]
fn played_rendition_equals_the_android_motion_rule() {
    let item = |kind, format, variants| summary(kind, format, variants);
    let chosen = |item: &ItemSummary| played_rendition(item).map(|(i, k)| (i.variant, k));

    let still = item(
        MediaKind::Still,
        MediaFormat::Png,
        vec![
            info(Variant::Original, "image/png", 10),
            info(Variant::Thumb, "image/webp", 5),
        ],
    );
    assert_eq!(chosen(&still), None);

    let gif = |original: u64, discord: Option<u64>| {
        let mut v = vec![
            info(Variant::Original, "image/gif", original),
            info(Variant::Thumb, "image/webp", 1),
            info(Variant::Mp4, "video/mp4", 2),
        ];
        v.extend(discord.map(|d| info(Variant::DiscordGif, "image/gif", d)));
        item(MediaKind::Animation, MediaFormat::Gif, v)
    };
    assert_eq!(
        chosen(&gif(300, None)),
        Some((Variant::Original, Playback::Picture)),
        "the MP4 is smaller but loses the transparency"
    );
    assert_eq!(
        chosen(&gif(9_000, Some(4_000))),
        Some((Variant::DiscordGif, Playback::Picture))
    );
    let webp = item(
        MediaKind::Animation,
        MediaFormat::Webp,
        vec![
            info(Variant::Original, "image/webp", 200),
            info(Variant::DiscordGif, "image/gif", 900),
            info(Variant::Mp4, "video/mp4", 50),
        ],
    );
    assert_eq!(chosen(&webp), Some((Variant::Original, Playback::Picture)));
    let only_mp4 = item(
        MediaKind::Animation,
        MediaFormat::Webp,
        vec![
            info(Variant::Mp4, "video/mp4", 50),
            info(Variant::Thumb, "image/webp", 1),
        ],
    );
    assert_eq!(chosen(&only_mp4), Some((Variant::Mp4, Playback::Video)));
    assert_eq!(chosen(&item(MediaKind::Animation, MediaFormat::Gif, vec![])), None);

    let webm = item(
        MediaKind::Video,
        MediaFormat::Webm,
        vec![
            info(Variant::Original, "video/webm", 100),
            info(Variant::Mp4, "video/mp4", 300),
            info(Variant::DiscordGif, "image/gif", 50),
        ],
    );
    assert_eq!(chosen(&webm), Some((Variant::Mp4, Playback::Video)));
    let mp4 = item(
        MediaKind::Video,
        MediaFormat::Mp4,
        vec![
            info(Variant::Original, "video/mp4", 100),
            info(Variant::Thumb, "image/webp", 1),
        ],
    );
    assert_eq!(chosen(&mp4), Some((Variant::Original, Playback::Video)));
    let odd = item(
        MediaKind::Video,
        MediaFormat::Mp4,
        vec![info(Variant::Original, "application/octet-stream", 100)],
    );
    assert_eq!(chosen(&odd), None, "only what a player takes");
}

// -- keeping a sticker, a pack, the library ----------------------------------------

/// Mock with alice's public pack `cats` (a still with a Telegram sticker
/// rendition, a GIF, a WebP still) and her private pack `memes` (a still, a
/// video).
fn offline_server() -> (MockServer, Vec<ItemSummary>) {
    let server = MockServer::start();
    let items = {
        let mut st = server.state();
        let cats = st.add_set("alice", "cats", "Cats", Visibility::Public);
        let memes = st.add_set("alice", "memes", "Memes", Visibility::Private);
        let grumpy = st.add_item(&cats.id, "grumpy cat", &["cat"], png_bytes(20, 20, 1));
        st.add_variant(&grumpy.id, Variant::TgSticker, "image/webp", webp_bytes(32, 32, 9));
        st.add_item(&cats.id, "naahh", &["nope"], gif_bytes(3, 2));
        st.add_item(&cats.id, "sleepy", &["tired"], webp_bytes(16, 16, 3));
        st.add_item(&memes.id, "this is fine", &["fire"], png_bytes(24, 18, 4));
        st.add_item(&memes.id, "clip", &["video"], mp4_bytes(3000));
        let ids: Vec<String> = st.items.iter().map(|i| i.item.id.clone()).collect();
        ids.iter()
            .map(|id| {
                let it = st.items.iter().find(|i| &i.item.id == id).unwrap().item.clone();
                st.summary(&it)
            })
            .collect()
    };
    (server, items)
}

async fn synced(server: &MockServer, dir: &Path, opts: OfflineOptions) -> Session {
    let session = session_for(server, dir).await;
    session.set_offline_options(opts);
    session.sync().await.unwrap();
    session
}

#[tokio::test]
async fn keep_a_sticker_downloads_its_files_and_only_those() {
    let (server, items) = offline_server();
    let dir = tempfile::tempdir().unwrap();
    let session = synced(&server, dir.path(), plain(&[TargetApp::Telegram, TargetApp::Discord])).await;
    let grumpy = &items[0];
    session.offline_keep(&Scope::Item(grumpy.id.clone())).unwrap();
    assert_eq!(session.offline_keeps().unwrap(), [Scope::Item(grumpy.id.clone())]);
    assert_eq!(session.offline_reasons(grumpy).unwrap(), [Reason::Item]);
    assert!(session.offline_reasons(&items[1]).unwrap().is_empty());

    let plan = session.offline_plan().unwrap();
    let want: BTreeSet<String> = plan.files.iter().map(|f| f.file.rel.clone()).collect();
    let variants: BTreeSet<Variant> = plan.files.iter().map(|f| f.file.variant).collect();
    assert_eq!(
        variants,
        BTreeSet::from([Variant::Thumb, Variant::Original, Variant::TgSticker]),
        "thumbnail, the copy for both apps, the Telegram sticker"
    );
    assert_eq!(plan.totals.present_files, 0);

    let control = quick();
    let out = run(&session, &control).await;
    assert!(out.is_complete(), "{out:?}");
    assert_eq!(out.status.state, OfflineState::Idle);
    assert_eq!((out.status.files_done, out.status.files_total), (3, 3));
    assert_eq!(out.status.scopes.len(), 1);
    assert_eq!(
        out.status.scopes[0].scope,
        PlanScope::Kept(Scope::Item(grumpy.id.clone()))
    );
    assert_eq!(out.status.scopes[0].files_done, 3);
    let cache = cache_dir(dir.path());
    assert_eq!(on_disk(&cache), want, "the expected files and only those");
    assert_eq!(kept(session.cache()), want, "all flagged as kept");
    assert_eq!(file_requests(&server).len(), 3, "each fetched once");
    // The copy path finds them (one store): no further request.
    let copy = session.prepare_copy(grumpy, TargetApp::Telegram, true).await.unwrap();
    assert!(copy.sticker);
    session.prepare_copy(grumpy, TargetApp::Discord, false).await.unwrap();
    assert_eq!(file_requests(&server).len(), 3);
    // Running again downloads nothing.
    let again = run(&session, &control).await;
    assert!(again.is_complete());
    assert_eq!(file_requests(&server).len(), 3);
    // Usage tells kept from other files.
    let usage = session.offline_usage().unwrap();
    assert_eq!(usage.kept_files, 3);
    assert_eq!(usage.budget, Some(1024 * 1024 * 1024));
}

#[tokio::test]
async fn keep_a_pack_and_the_library() {
    let (server, items) = offline_server();
    let dir = tempfile::tempdir().unwrap();
    let mut opts = plain(&[TargetApp::Generic]);
    opts.videos = true;
    let session = synced(&server, dir.path(), opts).await;
    let cats_id = items[0].set_id.clone();

    // The estimate before keeping it: 3 items, thumbnail and original each.
    let est = session.offline_estimate(&Scope::Set(cats_id.clone())).unwrap();
    assert_eq!((est.items, est.totals.files, est.totals.present_files), (3, 6, 0));
    assert!(est.fits());
    assert_eq!(est.kept_bytes_after, est.totals.bytes);

    session.offline_keep(&Scope::Set(cats_id.clone())).unwrap();
    let plan = session.offline_plan().unwrap();
    assert_eq!(plan.scopes[0].name.as_deref(), Some("Cats"));
    assert_eq!(plan.scopes[0].items, 3);
    let out = run(&session, &quick()).await;
    assert!(out.is_complete(), "{out:?}");
    assert_eq!(out.status.scopes[0].name.as_deref(), Some("Cats"));
    let cache = cache_dir(dir.path());
    let pack: BTreeSet<String> = on_disk(&cache);
    assert_eq!(pack.len(), 6, "{pack:?}");
    for item in &items[..3] {
        assert!(pack.contains(&format!("files/thumb/{}.webp", item.media_id)));
        assert!(pack
            .iter()
            .any(|p| p.starts_with(&format!("clip/{}.original/", item.media_id))));
    }
    assert!(!pack.iter().any(|p| p.contains(&items[3].media_id)), "memes not kept");
    let est = session.offline_estimate(&Scope::Set(cats_id.clone())).unwrap();
    assert_eq!((est.totals.files, est.totals.present_files), (6, 6));

    // The library adds the other pack.
    session.offline_keep(&Scope::Library).unwrap();
    let out = run(&session, &quick()).await;
    assert!(out.is_complete(), "{out:?}");
    let all = on_disk(&cache);
    assert_eq!(all.len(), 10, "five items, thumbnail and original each: {all:?}");
    assert_eq!(kept(session.cache()), all);
    assert_eq!(file_requests(&server).len(), 10);
    let reasons = session.offline_reasons(&items[0]).unwrap();
    assert_eq!(reasons, [Reason::Set, Reason::Library]);

    // Releasing the library returns memes' files to the LRU: still there,
    // no longer kept.
    session.offline_release(&Scope::Library).unwrap();
    assert_eq!(on_disk(&cache), all, "nothing deleted at once");
    assert_eq!(kept(session.cache()), pack);
}

#[tokio::test]
async fn a_sync_adds_to_a_kept_pack_and_removes_deleted_items() {
    let (server, items) = offline_server();
    let dir = tempfile::tempdir().unwrap();
    let session = synced(&server, dir.path(), plain(&[TargetApp::Generic])).await;
    let cats_id = items[0].set_id.clone();
    session.offline_keep(&Scope::Set(cats_id.clone())).unwrap();
    assert!(run(&session, &quick()).await.is_complete());
    let cache = cache_dir(dir.path());
    assert_eq!(on_disk(&cache).len(), 6);

    // A new sticker in the pack is wanted after the sync and downloaded.
    let added = server
        .state()
        .add_item(&cats_id, "new one", &["new"], png_bytes(12, 12, 7));
    session.sync().await.unwrap();
    let plan = session.offline_plan().unwrap();
    assert_eq!((plan.totals.files, plan.totals.present_files), (8, 6));
    assert!(run(&session, &quick()).await.is_complete());
    assert!(on_disk(&cache).contains(&format!("files/thumb/{}.webp", added.media_id)));

    // A sticker deleted on the server: its kept files go with the sync.
    let gone = &items[1];
    server.state().delete_item(&gone.id);
    session.sync().await.unwrap();
    let files = on_disk(&cache);
    assert!(!files.iter().any(|p| p.contains(&gone.media_id)), "{files:?}");
    assert_eq!(files.len(), 6);
    assert_eq!(kept(session.cache()), files);
}

#[tokio::test]
async fn kind_options_skip_and_a_kept_video_is_still_downloaded() {
    let (server, items) = offline_server();
    let dir = tempfile::tempdir().unwrap();
    // Videos off (the default), animations off, and no file over 1 MiB.
    let mut opts = plain(&[TargetApp::Generic]);
    opts.animations = false;
    opts.max_file_mb = 1;
    let session = synced(&server, dir.path(), opts).await;
    {
        // A still whose original is over the limit.
        let mut st = server.state();
        let it = st.add_item(&items[3].set_id, "huge still", &[], png_bytes(8, 8, 5));
        st.add_variant(&it.id, Variant::Original, "image/png", vec![0u8; 1_200_000]);
    }
    session.sync().await.unwrap();
    session.offline_keep(&Scope::Library).unwrap();
    let plan = session.offline_plan().unwrap();
    let skipped: Vec<(String, Option<Variant>, SkipReason)> = plan
        .skipped
        .iter()
        .map(|s| (s.name.clone(), s.variant, s.reason.clone()))
        .collect();
    assert!(skipped.contains(&(
        "naahh".into(),
        None,
        SkipReason::KindOff {
            kind: MediaKind::Animation
        }
    )));
    assert!(skipped.contains(&("clip".into(), None, SkipReason::KindOff { kind: MediaKind::Video })));
    assert!(skipped.iter().any(|(n, v, r)| n == "huge still"
        && *v == Some(Variant::Original)
        && matches!(r, SkipReason::TooLarge { .. })));
    assert!(SkipReason::KindOff { kind: MediaKind::Video }
        .to_string()
        .contains("videos"));

    let out = run(&session, &quick()).await;
    assert!(out.is_complete(), "{out:?}");
    assert_eq!(out.status.skipped, plan.skipped.len() as u64);
    let files = on_disk(&cache_dir(dir.path()));
    let video = &items[4];
    assert!(
        files.contains(&format!("files/thumb/{}.webp", video.media_id)),
        "the grid still shows it"
    );
    assert!(!files
        .iter()
        .any(|p| p.starts_with(&format!("clip/{}.", video.media_id))));
    assert!(!files
        .iter()
        .any(|p| p.starts_with(&format!("clip/{}.", items[1].media_id))));

    // Kept by itself, the video is downloaded whatever the options say.
    session.offline_keep(&Scope::Item(video.id.clone())).unwrap();
    assert!(run(&session, &quick()).await.is_complete());
    let files = on_disk(&cache_dir(dir.path()));
    assert!(files
        .iter()
        .any(|p| p.starts_with(&format!("clip/{}.original/", video.media_id))));
}

#[tokio::test]
async fn automatic_rules_keep_and_release() {
    let (server, items) = offline_server();
    let dir = tempfile::tempdir().unwrap();
    let session = synced(&server, dir.path(), plain(&[TargetApp::Generic])).await;
    let cache = cache_dir(dir.path());
    let control = quick();

    // Thumbnails of every item.
    let mut opts = plain(&[TargetApp::Generic]);
    opts.auto.thumbnails = true;
    session.set_offline_options(opts.clone());
    let out = run(&session, &control).await;
    assert!(out.is_complete(), "{out:?}");
    assert_eq!(out.status.scopes[0].scope, PlanScope::Thumbnails);
    let thumbs: BTreeSet<String> = items
        .iter()
        .map(|i| format!("files/thumb/{}.webp", i.media_id))
        .collect();
    assert_eq!(kept(session.cache()), thumbs);
    opts.auto.thumbnails = false;
    session.set_offline_options(opts.clone());
    session.offline_refresh().unwrap();
    assert!(kept(session.cache()).is_empty(), "released");
    assert_eq!(on_disk(&cache), thumbs, "not deleted");

    // The most recently used.
    for item in [&items[0], &items[2]] {
        let copy = session.prepare_copy(item, TargetApp::Generic, false).await.unwrap();
        session.record_copy(item, &copy).unwrap();
    }
    opts.auto.used = true;
    opts.auto.used_count = 1;
    session.set_offline_options(opts.clone());
    session.offline_refresh().unwrap();
    assert_eq!(session.offline_reasons(&items[2]).unwrap(), [Reason::Used]);
    assert!(
        session.offline_reasons(&items[0]).unwrap().is_empty(),
        "only the most recent one"
    );
    let used = kept(session.cache());
    assert!(used.iter().all(|p| p.contains(&items[2].media_id)), "{used:?}");
    assert_eq!(used.len(), 2, "thumbnail and original");
    opts.auto.used_count = 2;
    session.set_offline_options(opts.clone());
    session.offline_refresh().unwrap();
    assert_eq!(kept(session.cache()).len(), 4);
    opts.auto.used = false;
    session.set_offline_options(opts.clone());
    session.offline_refresh().unwrap();
    assert!(kept(session.cache()).is_empty());

    // Liked: listed from the server by the run, released by an unlike.
    session.toggle_like(&items[3].id).await.unwrap();
    opts.auto.liked = true;
    session.set_offline_options(opts.clone());
    let out = run(&session, &control).await;
    assert!(out.is_complete(), "{out:?}");
    assert_eq!(session.offline_reasons(&items[3]).unwrap(), [Reason::Liked]);
    assert!(kept(session.cache()).iter().all(|p| p.contains(&items[3].media_id)));
    assert_eq!(kept(session.cache()).len(), 2);
    assert!(!session.toggle_like(&items[3].id).await.unwrap());
    session.offline_refresh().unwrap();
    assert!(kept(session.cache()).is_empty());
    opts.auto.liked = false;

    // Own packs: alice's, not bob's.
    {
        let mut st = server.state();
        st.add_user("bob", "pw", Role::User);
        let bobs = st.add_set("bob", "bobs", "Bob's", Visibility::Public);
        st.add_item(&bobs.id, "bob's sticker", &[], png_bytes(10, 10, 8));
    }
    session.sync().await.unwrap();
    opts.auto.own_packs = true;
    session.set_offline_options(opts.clone());
    let out = run(&session, &control).await;
    assert!(out.is_complete(), "{out:?}");
    let bob_item = session
        .cache()
        .items()
        .unwrap()
        .into_iter()
        .find(|i| i.owner_username == "bob")
        .unwrap();
    assert!(session.offline_reasons(&bob_item).unwrap().is_empty());
    assert_eq!(session.offline_reasons(&items[0]).unwrap(), [Reason::OwnPack]);
    let own = kept(session.cache());
    assert!(!own.iter().any(|p| p.contains(&bob_item.media_id)));
    // Videos are off: the clip's original is not kept.
    assert_eq!(own.len(), 2 * 5 - 1, "{own:?}");
    opts.auto.own_packs = false;
    session.set_offline_options(opts);
    session.offline_refresh().unwrap();
    assert!(kept(session.cache()).is_empty());
}

// -- what is never deleted ---------------------------------------------------------

#[test]
fn eviction_and_clear_cache_never_remove_kept_files() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::open(dir.path(), 1).unwrap(); // 1 MiB cap
    let write = |rel: &str, len: usize| {
        let p = cache.abs(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, vec![0u8; len]).unwrap();
    };
    // One file flagged after it was cached, one downloaded as kept.
    write("clip/k1.original/a.png", 700 * 1024);
    cache.register_file("clip/k1.original/a.png").unwrap();
    let wanted: std::collections::HashSet<String> = ["clip/k1.original/a.png", "files/thumb/k2.webp"]
        .map(String::from)
        .into();
    let report = cache.offline_mark(&wanted, &Default::default()).unwrap();
    assert_eq!(report.flagged, 1);
    write("files/thumb/k2.webp", 700 * 1024);
    assert!(cache
        .register_kept_file("files/thumb/k2.webp", cache.offline_epoch().unwrap())
        .unwrap());
    assert_eq!(cache.offline_mark(&wanted, &Default::default()).unwrap().flagged, 0);
    // Kept files do not count against the cap: two more files fit...
    write("files/thumb/o1.webp", 400 * 1024);
    cache.register_file("files/thumb/o1.webp").unwrap();
    write("files/thumb/o2.webp", 400 * 1024);
    cache.register_file("files/thumb/o2.webp").unwrap();
    assert!(cache.abs("files/thumb/o1.webp").exists());
    // ...and pressure evicts only those.
    write("clip/o3.original/big.mp4", 900 * 1024);
    let report = cache.register_file("clip/o3.original/big.mp4").unwrap();
    assert_eq!(report.removed_files, 2);
    assert_eq!((report.kept_files, report.kept_bytes), (2, 1400 * 1024));
    for k in &wanted {
        assert!(cache.abs(k).exists(), "{k} survived eviction");
    }
    assert!(!cache.abs("files/thumb/o1.webp").exists());
    // A kept file used again keeps its place behind every other one.
    assert!(cache.lookup_file("clip/k1.original/a.png", None).unwrap().is_some());
    let report = cache.evict(None).unwrap();
    assert_eq!(report.kept_files, 2);

    // "Clear cache" leaves kept files and says so.
    let report = cache.clear_files().unwrap();
    assert_eq!(report.removed_files, 1);
    assert_eq!((report.kept_files, report.kept_bytes), (2, 1400 * 1024));
    for k in &wanted {
        assert!(cache.abs(k).exists());
    }
    let usage = cache.offline_usage().unwrap();
    assert_eq!((usage.kept_files, usage.other_files), (2, 0));

    // The confirmed removal takes them and forgets what to keep.
    cache.offline_keep(&Scope::Library).unwrap();
    let report = cache.offline_reset().unwrap();
    assert_eq!((report.removed_files, report.freed_bytes), (2, 1400 * 1024));
    for k in &wanted {
        assert!(!cache.abs(k).exists());
    }
    assert!(cache.offline_keeps().unwrap().is_empty());
    assert!(!cache.abs("clip/k1.original").exists(), "empty clip dir pruned");
}

#[tokio::test]
async fn session_clear_cache_keeps_kept_files_and_remove_all_takes_them() {
    let (server, items) = offline_server();
    let dir = tempfile::tempdir().unwrap();
    let session = synced(&server, dir.path(), plain(&[TargetApp::Generic])).await;
    session.offline_keep(&Scope::Item(items[0].id.clone())).unwrap();
    assert!(run(&session, &quick()).await.is_complete());
    // Another, unkept file.
    session
        .prepare_copy(&items[2], TargetApp::Generic, false)
        .await
        .unwrap();
    let cache = cache_dir(dir.path());
    let before = on_disk(&cache);
    let report = session.clear_cache().unwrap();
    assert_eq!(report.removed_files, 1);
    assert_eq!(report.kept_files, 2);
    assert_eq!(on_disk(&cache).len(), before.len() - 1);
    assert_eq!(on_disk(&cache), kept(session.cache()));

    let report = session.offline_remove_all().unwrap();
    assert_eq!(report.removed_files, 2);
    assert!(on_disk(&cache).is_empty());
    assert!(session.offline_keeps().unwrap().is_empty());
}

#[tokio::test]
async fn the_budget_stops_the_downloader_without_deleting() {
    let server = MockServer::start();
    let ids: Vec<ItemSummary> = {
        let mut st = server.state();
        let set = st.add_set("alice", "big", "Big", Visibility::Private);
        (0..3)
            .map(|i| st.add_item(&set.id, &format!("big {i}"), &[], mp4_bytes(600 * 1024 + i)))
            .collect()
    };
    let dir = tempfile::tempdir().unwrap();
    let mut opts = plain(&[TargetApp::Generic]);
    opts.max_mb = 1;
    let session = synced(&server, dir.path(), opts).await;
    for item in &ids {
        session.offline_keep(&Scope::Item(item.id.clone())).unwrap();
    }
    let plan = session.offline_plan().unwrap();
    assert!(plan.over_budget());
    let out = run(&session, &quick()).await;
    assert_eq!(out.end, RunEnd::LimitReached, "{out:?}");
    assert_eq!(out.status.state, OfflineState::LimitReached);
    assert!(out.status.message.as_deref().unwrap().contains("max_mb"));
    let files = on_disk(&cache_dir(dir.path()));
    let originals = files.iter().filter(|p| p.contains(".original/")).count();
    assert_eq!(originals, 1, "one 600 KiB file fits in 1 MiB: {files:?}");
    assert!(part_files(&cache_dir(dir.path())).is_empty());
    // Running again deletes nothing and fetches nothing more.
    let requests = file_requests(&server).len();
    let again = run(&session, &quick()).await;
    assert_eq!(again.end, RunEnd::LimitReached);
    assert_eq!(on_disk(&cache_dir(dir.path())), files);
    assert_eq!(file_requests(&server).len(), requests);
    // A larger budget goes on.
    let mut opts = session.offline_options();
    opts.max_mb = 4;
    session.set_offline_options(opts);
    assert!(run(&session, &quick()).await.is_complete());
}

/// The disk is made full by writing the downloads to `/dev/full`, whose
/// writes fail with ENOSPC (the kernel's own error, as on a full disk).
#[tokio::test]
async fn a_full_disk_stops_the_downloader_without_deleting() {
    assert!(Path::new("/dev/full").exists(), "this test needs /dev/full");
    let (server, items) = offline_server();
    let dir = tempfile::tempdir().unwrap();
    let session = synced(&server, dir.path(), plain(&[TargetApp::Generic])).await;
    session.offline_keep(&Scope::Item(items[0].id.clone())).unwrap();
    assert!(run(&session, &quick()).await.is_complete());
    let before = on_disk(&cache_dir(dir.path()));
    let requests = file_requests(&server).len();

    session.offline_keep(&Scope::Library).unwrap();
    let control = quick();
    control.tune(|t| t.part_sink = Some(PathBuf::from("/dev/full")));
    let out = run(&session, &control).await;
    assert_eq!(out.end, RunEnd::DiskFull, "{out:?}");
    assert_eq!(out.status.state, OfflineState::DiskFull);
    let message = out.status.message.unwrap();
    assert!(message.contains("No space left"), "{message}");
    assert_eq!(
        on_disk(&cache_dir(dir.path())),
        before,
        "nothing deleted, nothing partial"
    );
    // No retry loop: at most one request per download slot.
    assert!(file_requests(&server).len() - requests <= 4);
    assert!(
        session.cache().offline_failures().unwrap().is_empty(),
        "not a failure of the file"
    );
}

// -- interruption, pause, rate limits, failures ---------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_interrupted_download_resumes_and_leaves_no_partial_file() {
    let (mut server, items) = offline_server();
    let dir = tempfile::tempdir().unwrap();
    let session = synced(&server, dir.path(), plain(&[TargetApp::Generic])).await;
    let slow = server
        .state()
        .add_item(&items[0].set_id, "slow", &[], mp4_bytes(40 * 1024));
    server
        .state()
        .slow_files
        .insert((slow.id.clone(), Variant::Original), Duration::from_millis(50));
    session.sync().await.unwrap();
    session.offline_keep(&Scope::Item(slow.id.clone())).unwrap();
    let cache = cache_dir(dir.path());
    let control = quick();
    let (out, _) = tokio::join!(run(&session, &control), async {
        // The server goes away in the middle of the download.
        for _ in 0..200 {
            if !part_files(&cache).is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!part_files(&cache).is_empty(), "the download started");
        tokio::task::block_in_place(|| server.kill());
    });
    assert_eq!(out.end, RunEnd::WaitingForNetwork, "{out:?}");
    assert_eq!(out.status.state, OfflineState::WaitingForNetwork);
    assert!(
        part_files(&cache).is_empty(),
        "no partial file left: {:?}",
        on_disk(&cache)
    );
    assert!(!on_disk(&cache)
        .iter()
        .any(|p| p.contains(&format!("{}.original", slow.media_id))));
    assert!(session.cache().offline_failures().unwrap().is_empty());

    // With the server unreachable a run ends at once.
    let t = Instant::now();
    let out = run(&session, &control).await;
    assert_eq!(out.end, RunEnd::WaitingForNetwork);
    assert!(t.elapsed() < Duration::from_secs(5));

    // Back: the next run finishes it.
    tokio::task::block_in_place(|| server.restart());
    server.state().slow_files.clear();
    let out = run(&session, &control).await;
    assert!(out.is_complete(), "{out:?}");
    assert!(part_files(&cache).is_empty());
    let original = on_disk(&cache)
        .into_iter()
        .find(|p| p.contains(&format!("{}.original", slow.media_id)))
        .unwrap();
    assert_eq!(std::fs::read(cache.join(original)).unwrap(), mp4_bytes(40 * 1024));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pause_resume_and_cancel() {
    let (server, items) = offline_server();
    let dir = tempfile::tempdir().unwrap();
    let session = synced(&server, dir.path(), plain(&[TargetApp::Generic])).await;
    let slow = server
        .state()
        .add_item(&items[0].set_id, "slow", &[], mp4_bytes(20 * 1024));
    server
        .state()
        .slow_files
        .insert((slow.id.clone(), Variant::Original), Duration::from_millis(40));
    session.sync().await.unwrap();
    session.offline_keep(&Scope::Item(slow.id.clone())).unwrap();
    let cache = cache_dir(dir.path());
    let control = quick();
    let mut status = control.subscribe();

    let (out, _) = tokio::join!(run(&session, &control), async {
        for _ in 0..200 {
            if !part_files(&cache).is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        control.pause();
        assert!(control.is_paused());
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(control.status().state, OfflineState::Paused);
        assert!(part_files(&cache).is_empty(), "the running download stopped");
        server.state().slow_files.clear();
        control.resume();
    });
    assert!(out.is_complete(), "{out:?}");
    assert!(status.has_changed().unwrap());
    assert_eq!(status.borrow_and_update().state, OfflineState::Idle);

    // Cancel.
    let slow2 = server
        .state()
        .add_item(&items[0].set_id, "slow2", &[], mp4_bytes(20 * 1024));
    server
        .state()
        .slow_files
        .insert((slow2.id.clone(), Variant::Original), Duration::from_millis(40));
    session.sync().await.unwrap();
    session.offline_keep(&Scope::Item(slow2.id.clone())).unwrap();
    let (out, _) = tokio::join!(run(&session, &control), async {
        for _ in 0..200 {
            if !part_files(&cache).is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        control.cancel();
    });
    assert_eq!(out.end, RunEnd::Cancelled);
    assert!(part_files(&cache).is_empty());
    assert!(!out.is_complete());
    // The next run starts afresh.
    server.state().slow_files.clear();
    assert!(run(&session, &control).await.is_complete());
}

#[tokio::test]
async fn a_rate_limit_is_obeyed() {
    let (server, items) = offline_server();
    let dir = tempfile::tempdir().unwrap();
    let session = synced(&server, dir.path(), plain(&[TargetApp::Generic])).await;
    session.offline_keep(&Scope::Item(items[0].id.clone())).unwrap();
    {
        let mut st = server.state();
        st.file_429 = 1;
        st.retry_after_secs = Some(1);
    }
    let control = quick();
    control.tune(|t| t.concurrency = 1);
    let t = Instant::now();
    let out = run(&session, &control).await;
    assert!(out.is_complete(), "{out:?}");
    assert!(
        t.elapsed() >= Duration::from_secs(1),
        "waited as told: {:?}",
        t.elapsed()
    );
    let reqs = file_requests(&server);
    assert_eq!(reqs.len(), 3, "the refused file asked again: {reqs:?}");
    assert!(session.cache().offline_failures().unwrap().is_empty());
}

#[tokio::test]
async fn a_failing_file_fails_with_its_reason_and_the_rest_arrive() {
    let (server, items) = offline_server();
    let dir = tempfile::tempdir().unwrap();
    let session = synced(&server, dir.path(), plain(&[TargetApp::Generic])).await;
    let cats = items[0].set_id.clone();
    {
        let mut st = server.state();
        st.failing_files.insert((items[0].id.clone(), Variant::Original), 500);
        st.wrong_size_files.insert((items[1].id.clone(), Variant::Original));
    }
    session.offline_keep(&Scope::Set(cats)).unwrap();
    let out = run(&session, &quick()).await;
    assert_eq!(out.end, RunEnd::Complete);
    assert!(!out.is_complete());
    assert_eq!(out.status.failed, 2, "{out:?}");
    assert_eq!(out.status.files_done, out.status.files_total - 2);
    let failures = session.cache().offline_failures().unwrap();
    assert_eq!(failures.len(), 2);
    let failed = |id: &str| failures.iter().find(|f| f.item_id == id).unwrap().clone();
    let f = failed(&items[0].id);
    assert!(f.error.contains("this file fails"), "{f:?}");
    assert_eq!(f.attempts, 3);
    assert!(failed(&items[1].id).error.contains("size mismatch"));
    let plan = session.offline_plan().unwrap();
    assert_eq!(plan.failures.len(), 2);
    assert!(part_files(&cache_dir(dir.path())).is_empty());
    // Three tries each, then the next run tries once more.
    let before = file_requests(&server).len();
    let again = run(&session, &quick()).await;
    assert_eq!(again.status.failed, 2);
    assert_eq!(file_requests(&server).len() - before, 2);
    assert_eq!(session.cache().offline_failures().unwrap()[0].attempts, 4);
    // Fixed on the server: it arrives and the failure is forgotten.
    server.state().failing_files.clear();
    server.state().wrong_size_files.clear();
    let fixed = run(&session, &quick()).await;
    assert!(fixed.is_complete(), "{fixed:?}");
    assert!(session.cache().offline_failures().unwrap().is_empty());
}

// -- two processes, claims -------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_caches_on_one_directory_neither_duplicate_nor_tear_a_download() {
    let (server, items) = offline_server();
    let dir = tempfile::tempdir().unwrap();
    for item in &items {
        server
            .state()
            .slow_files
            .insert((item.id.clone(), Variant::Original), Duration::from_millis(20));
    }
    let a = synced(&server, dir.path(), plain(&[TargetApp::Generic])).await;
    let mut opts = plain(&[TargetApp::Generic]);
    opts.videos = true;
    a.set_offline_options(opts.clone());
    // A second "process": its own Session and SQLite connection on the same files.
    let b = Session::open(config_for(&server.url, dir.path())).await.unwrap();
    b.set_offline_options(opts);
    a.offline_keep(&Scope::Library).unwrap();
    let (ca, cb) = (quick(), quick());
    let (oa, ob) = tokio::join!(run(&a, &ca), run(&b, &cb));
    assert!(oa.is_complete(), "{oa:?}");
    assert!(ob.is_complete(), "{ob:?}");
    let reqs = file_requests(&server);
    let unique: BTreeSet<&String> = reqs.iter().collect();
    assert_eq!(reqs.len(), unique.len(), "each file fetched once: {reqs:?}");
    assert_eq!(reqs.len(), 10);
    let cache = cache_dir(dir.path());
    assert!(part_files(&cache).is_empty());
    // Not torn: every original is the server's file.
    for item in &items {
        let rel = on_disk(&cache)
            .into_iter()
            .find(|p| p.starts_with(&format!("clip/{}.original/", item.media_id)))
            .unwrap();
        assert_eq!(sha(&std::fs::read(cache.join(rel)).unwrap()), item.sha256);
    }
    assert!(a.cache().offline_failures().unwrap().is_empty());
}

#[tokio::test]
async fn a_stale_claim_expires() {
    let (server, items) = offline_server();
    let dir = tempfile::tempdir().unwrap();
    let session = synced(&server, dir.path(), plain(&[TargetApp::Generic])).await;
    session.offline_keep(&Scope::Item(items[0].id.clone())).unwrap();
    let plan = session.offline_plan().unwrap();
    let rel = plan.files[1].file.rel.clone();
    // A dead process's claim, and its part file.
    assert!(session.cache().offline_claim(&rel, "dead", 120).unwrap());
    let part = session.cache().abs(&format!("{rel}.part.dead"));
    std::fs::create_dir_all(part.parent().unwrap()).unwrap();
    std::fs::write(&part, b"half").unwrap();
    // A thumbnail's part file of a dead process, and one of a live one.
    let thumbs = session.cache().abs("files/thumb");
    std::fs::create_dir_all(&thumbs).unwrap();
    std::fs::write(thumbs.join("dead.webp.part.dead"), b"half").unwrap();
    std::fs::write(thumbs.join("live.webp.part.live"), b"half").unwrap();
    assert!(session
        .cache()
        .offline_claim("files/thumb/live.webp", "live", 120)
        .unwrap());
    let control = quick();
    control.tune(|t| t.claim_stale_secs = 2);
    let t = Instant::now();
    let out = run(&session, &control).await;
    assert!(out.is_complete(), "{out:?}");
    assert!(
        t.elapsed() >= Duration::from_secs(1),
        "waited for the claim to go stale"
    );
    assert!(!part.exists(), "the dead process's part file is gone");
    assert!(!thumbs.join("dead.webp.part.dead").exists());
    assert!(
        thumbs.join("live.webp.part.live").exists(),
        "a live download's part file stays"
    );
    assert!(!session.cache().offline_claimed_by_other(&rel, "x", 120).unwrap());
}

// -- copies -------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_copy_goes_before_background_downloads() {
    let (server, items) = offline_server();
    let dir = tempfile::tempdir().unwrap();
    let session = synced(&server, dir.path(), plain(&[TargetApp::Generic])).await;
    session.offline_keep(&Scope::Set(items[0].set_id.clone())).unwrap();
    let control = quick();
    // While a user's download runs, no background download starts.
    let held = session.foreground().enter();
    let (out, _) = tokio::join!(run(&session, &control), async {
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(file_requests(&server).is_empty(), "the background waited");
        drop(held);
    });
    assert!(out.is_complete(), "{out:?}");

    // A copy while slow background downloads run: it is served at once.
    let slow = server
        .state()
        .add_item(&items[3].set_id, "slow", &[], mp4_bytes(30 * 1024));
    server
        .state()
        .slow_files
        .insert((slow.id.clone(), Variant::Original), Duration::from_millis(30));
    session.sync().await.unwrap();
    session.offline_keep(&Scope::Item(slow.id.clone())).unwrap();
    let cache = cache_dir(dir.path());
    let (out, copied_at) = tokio::join!(run(&session, &control), async {
        for _ in 0..200 {
            if !part_files(&cache).is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let t = Instant::now();
        session
            .prepare_copy(&items[3], TargetApp::Generic, false)
            .await
            .unwrap();
        let took = t.elapsed();
        (Instant::now(), took)
    });
    assert!(out.is_complete(), "{out:?}");
    assert!(
        copied_at.1 < Duration::from_millis(500),
        "the copy did not wait: {:?}",
        copied_at.1
    );
}

// -- with the server unreachable ------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn offline_a_kept_sticker_copies_for_every_target_and_another_is_not_on_device() {
    let (mut server, items) = offline_server();
    let dir = tempfile::tempdir().unwrap();
    let targets = [
        TargetApp::Telegram,
        TargetApp::Discord,
        TargetApp::Whatsapp,
        TargetApp::Generic,
    ];
    let session = synced(&server, dir.path(), plain(&targets)).await;
    let kept_item = &items[0];
    session.offline_keep(&Scope::Item(kept_item.id.clone())).unwrap();
    assert!(run(&session, &quick()).await.is_complete());
    assert_eq!(
        session.available_offline(&items[..2], TargetApp::Telegram, true),
        [true, false]
    );
    assert!(session.variant_on_device(kept_item, Variant::TgSticker));
    assert!(!session.variant_on_device(&items[1], Variant::Original));

    tokio::task::block_in_place(|| server.kill());
    for target in targets {
        for sticker in [false, true] {
            let copy = session.prepare_copy(kept_item, target, sticker).await;
            let copy = copy.unwrap_or_else(|e| panic!("{target:?} {sticker}: {e}"));
            assert!(copy.file.exists());
        }
        let err = session.prepare_copy(&items[1], target, false).await.unwrap_err();
        assert!(err.is_not_on_device(), "{err}");
        assert!(err.is_offline());
        assert!(err.to_string().contains("not on this device"), "{err}");
    }
    let err = session.fetch_variant(&items[2], Variant::Original).await.unwrap_err();
    assert!(
        matches!(
            err,
            crate::CoreError::NotOnDevice {
                variant: Variant::Original,
                ..
            }
        ),
        "{err}"
    );
    session.fetch_variant(kept_item, Variant::Original).await.unwrap();
}

// -- sign-out, server change, older clients ------------------------------------------

#[tokio::test]
async fn sign_out_and_a_server_change_remove_kept_files_and_the_list() {
    let (server, items) = offline_server();
    let dir = tempfile::tempdir().unwrap();
    let session = synced(&server, dir.path(), plain(&[TargetApp::Generic])).await;
    session.offline_keep(&Scope::Set(items[0].set_id.clone())).unwrap();
    assert!(run(&session, &quick()).await.is_complete());
    let cache = cache_dir(dir.path());
    assert_eq!(on_disk(&cache).len(), 6);
    session
        .cache()
        .offline_fail("clip/x.original/x.png", "x", "boom", 1)
        .unwrap();
    session
        .cache()
        .offline_claim("clip/y.original/y.png", "someone", 120)
        .unwrap();
    // An unkept file stays (content-addressed, LRU).
    session
        .prepare_copy(&items[3], TargetApp::Generic, false)
        .await
        .unwrap();

    session.logout().await.unwrap();
    assert!(session.offline_keeps().unwrap().is_empty());
    assert!(session.cache().offline_failures().unwrap().is_empty());
    assert!(!session
        .cache()
        .offline_claimed_by_other("clip/y.original/y.png", "me", 120)
        .unwrap());
    let left = on_disk(&cache);
    assert_eq!(left.len(), 1, "only the unkept copy: {left:?}");
    assert!(kept(session.cache()).is_empty());

    // Server change.
    drop(session);
    let session = synced(&server, dir.path(), plain(&[TargetApp::Generic])).await;
    session.offline_keep(&Scope::Library).unwrap();
    assert!(run(&session, &quick()).await.is_complete());
    assert!(!kept(session.cache()).is_empty());
    drop(session);
    let other = MockServer::start();
    let moved = Session::open(config_for(&other.url, dir.path())).await.unwrap();
    assert!(moved.offline_keeps().unwrap().is_empty());
    assert!(kept(moved.cache()).is_empty());
    assert!(on_disk(&cache).len() <= 1, "{:?}", on_disk(&cache));
}

/// The schema of a cache written by stickpile 0.8.0 (cache.rs at v0.8.0).
const SCHEMA_0_8_0: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS items (id TEXT PRIMARY KEY, set_id TEXT NOT NULL, updated_at INTEGER NOT NULL, json TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS items_set ON items(set_id);
CREATE INDEX IF NOT EXISTS items_without_text ON items(id) WHERE json_extract(json, '$.text') IS NULL;
CREATE TABLE IF NOT EXISTS files (path TEXT PRIMARY KEY, size INTEGER NOT NULL, last_access INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS files_lru ON files(last_access);
CREATE TABLE IF NOT EXISTS history (seq INTEGER PRIMARY KEY AUTOINCREMENT, item_id TEXT NOT NULL, copied_at INTEGER NOT NULL, target TEXT, variant TEXT, item_json TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS history_item ON history(item_id);
CREATE TABLE IF NOT EXISTS usage_queue (seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE, json TEXT NOT NULL, queued_at INTEGER NOT NULL);
";

/// What 0.8.0 does when it opens a cache.
fn open_as_0_8_0(dir: &Path) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open(dir.join(crate::cache::DB_FILE)).unwrap();
    conn.busy_timeout(Duration::from_secs(10)).unwrap();
    let _: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0)).unwrap();
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert!(version <= 1, "0.8.0 refuses schema {version}");
    conn.execute_batch(SCHEMA_0_8_0).unwrap();
    conn.execute_batch("PRAGMA user_version=1").unwrap();
    conn
}

/// 0.8.0's `evict_to(limit)`: least recently used first, kept or not.
fn evict_as_0_8_0(conn: &rusqlite::Connection, dir: &Path, limit: u64) -> usize {
    let mut total: u64 = conn
        .query_row("SELECT COALESCE(SUM(size), 0) FROM files", [], |r| r.get::<_, i64>(0))
        .unwrap() as u64;
    let rows: Vec<(String, u64)> = conn
        .prepare("SELECT path, size FROM files ORDER BY last_access ASC")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let mut removed = 0;
    for (rel, size) in rows {
        if total <= limit {
            break;
        }
        let _ = std::fs::remove_file(dir.join(&rel));
        conn.execute("DELETE FROM files WHERE path = ?1", [&rel]).unwrap();
        total -= size;
        removed += 1;
    }
    removed
}

#[test]
fn a_cache_written_by_0_8_0_opens_and_upgrades() {
    let dir = tempfile::tempdir().unwrap();
    {
        let conn = open_as_0_8_0(dir.path());
        conn.execute(
            "INSERT INTO files(path, size, last_access) VALUES ('files/thumb/m.webp', 4, 7)",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO meta(key, value) VALUES ('server_url', 'http://s')", [])
            .unwrap();
    }
    std::fs::create_dir_all(dir.path().join("files/thumb")).unwrap();
    std::fs::write(dir.path().join("files/thumb/m.webp"), b"webp").unwrap();
    let cache = Cache::open(dir.path(), 512).unwrap();
    assert_eq!(cache.meta_get("offline_format").unwrap().as_deref(), Some("1"));
    assert_eq!(cache.meta_get("server_url").unwrap().as_deref(), Some("http://s"));
    let index = cache.file_index().unwrap();
    assert_eq!(index.get("files/thumb/m.webp"), Some(&(4, false)), "kept 0 by default");
    assert!(cache.lookup_file("files/thumb/m.webp", Some(4)).unwrap().is_some());
    cache.offline_keep(&Scope::Library).unwrap();
    drop(cache);
    // Opening again (the column exists now) changes nothing; the schema
    // version stays 1, so 0.8.0 still opens it.
    let cache = Cache::open(dir.path(), 512).unwrap();
    assert_eq!(cache.offline_keeps().unwrap().len(), 1);
    drop(cache);
    let conn = open_as_0_8_0(dir.path());
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(version, 1);
}

#[tokio::test]
async fn a_cache_written_by_the_new_client_works_in_0_8_0_and_heals() {
    let (server, items) = offline_server();
    let dir = tempfile::tempdir().unwrap();
    let session = synced(&server, dir.path(), plain(&[TargetApp::Generic])).await;
    session.offline_keep(&Scope::Set(items[0].set_id.clone())).unwrap();
    assert!(run(&session, &quick()).await.is_complete());
    session
        .prepare_copy(&items[3], TargetApp::Generic, false)
        .await
        .unwrap();
    let cache = cache_dir(dir.path());
    let kept_files = kept(session.cache());
    assert_eq!(kept_files.len(), 6);
    let requests = file_requests(&server).len();

    {
        // 0.8.0 opens the cache and works in it: it reads items and files...
        let conn = open_as_0_8_0(&cache);
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 5);
        // ...registers a file of its own (kept 0 by the default)...
        std::fs::create_dir_all(cache.join("files/thumb")).unwrap();
        std::fs::write(cache.join("files/thumb/old.webp"), b"x").unwrap();
        conn.execute(
            "INSERT INTO files(path, size, last_access) VALUES ('files/thumb/old.webp', 1, 1)
             ON CONFLICT(path) DO UPDATE SET size = excluded.size, last_access = excluded.last_access",
            [],
        )
        .unwrap();
        // ...evicts least recently used first: the kept files go last...
        let to_keep: u64 = conn
            .query_row("SELECT SUM(size) FROM files WHERE kept = 1", [], |r| r.get::<_, i64>(0))
            .unwrap() as u64;
        evict_as_0_8_0(&conn, &cache, to_keep);
        assert_eq!(on_disk(&cache), kept_files, "0.8.0 took the other files first");
        // ...but under more pressure it evicts kept files, and registers one
        // of them again without the flag's meaning.
        assert!(evict_as_0_8_0(&conn, &cache, 0) > 0);
        assert!(on_disk(&cache).is_empty());
        let rel = kept_files.iter().find(|p| p.contains(".original/")).unwrap();
        std::fs::create_dir_all(cache.join(rel).parent().unwrap()).unwrap();
        std::fs::write(cache.join(rel), b"stale and the wrong size").unwrap();
        conn.execute(
            "INSERT INTO files(path, size, last_access) VALUES (?1, 24, 5)
             ON CONFLICT(path) DO UPDATE SET size = excluded.size, last_access = excluded.last_access",
            [rel],
        )
        .unwrap();
    }

    // The new client heals: the plan wants them, they are fetched again.
    let plan = session.offline_refresh().unwrap();
    assert_eq!(plan.totals.present_files, 0);
    let out = run(&session, &quick()).await;
    assert!(out.is_complete(), "{out:?}");
    assert_eq!(kept(session.cache()), kept_files);
    assert_eq!(on_disk(&cache), kept_files);
    assert_eq!(file_requests(&server).len() - requests, 6);
    for item in &items[..3] {
        let rel = on_disk(&cache)
            .into_iter()
            .find(|p| p.starts_with(&format!("clip/{}.original/", item.media_id)))
            .unwrap();
        assert_eq!(sha(&std::fs::read(cache.join(rel)).unwrap()), item.sha256);
    }
}

#[tokio::test]
async fn a_sign_out_by_0_8_0_is_completed_by_the_new_client() {
    let (server, items) = offline_server();
    let dir = tempfile::tempdir().unwrap();
    let session = synced(&server, dir.path(), plain(&[TargetApp::Generic])).await;
    session.offline_keep(&Scope::Item(items[0].id.clone())).unwrap();
    assert!(run(&session, &quick()).await.is_complete());
    let cache = cache_dir(dir.path());
    assert_eq!(on_disk(&cache).len(), 2);
    {
        // 0.8.0's `reset_account_data` knows nothing of offline data.
        let conn = open_as_0_8_0(&cache);
        conn.execute_batch(
            "DELETE FROM items; DELETE FROM history; DELETE FROM usage_queue;
             DELETE FROM meta WHERE key IN ('me', 'updated_since');",
        )
        .unwrap();
    }
    session.offline_refresh().unwrap();
    assert!(session.offline_keeps().unwrap().is_empty());
    assert!(on_disk(&cache).is_empty());
}

// -- options -------------------------------------------------------------------------

#[tokio::test]
async fn the_device_targets_come_from_the_options_or_the_history() {
    let (server, items) = offline_server();
    let dir = tempfile::tempdir().unwrap();
    let session = synced(&server, dir.path(), plain(&[])).await;
    assert_eq!(session.offline_device().unwrap().targets, [TargetApp::Generic]);
    let copy = session
        .prepare_copy(&items[0], TargetApp::Discord, false)
        .await
        .unwrap();
    session.record_copy(&items[0], &copy).unwrap();
    assert_eq!(
        session.offline_device().unwrap().targets,
        [TargetApp::Generic, TargetApp::Discord]
    );
    session.set_offline_options(plain(&[TargetApp::Telegram, TargetApp::Telegram]));
    let device = session.offline_device().unwrap();
    assert_eq!(device.targets, [TargetApp::Telegram]);
    assert!(!device.plays_motion);
    let android = Session::open(config_for(&server.url, dir.path()))
        .await
        .unwrap()
        .with_grid_playback(true);
    assert!(android.offline_device().unwrap().plays_motion);
}

/// The released 0.8.0 CLI on a cache the new client wrote, against the mock
/// (run by hand: `STICKPILE_080_CLI=<path to the 0.8.0 stickpile binary>
/// STICKPILE_SCRATCH=<dir> cargo test -p stickpile-core real_0_8_0 --
/// --ignored --nocapture`). The profile (config, token, cache with a 1 MiB
/// `cache_max_mb`) lives in a fresh directory under `STICKPILE_SCRATCH`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the released 0.8.0 CLI (STICKPILE_080_CLI)"]
async fn real_0_8_0_cli_on_a_cache_the_new_client_wrote() {
    let cli = std::env::var("STICKPILE_080_CLI").expect("STICKPILE_080_CLI");
    let scratch = std::env::var("STICKPILE_SCRATCH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let dir = tempfile::tempdir_in(&scratch).unwrap();
    let (server, items) = offline_server();
    let big = server
        .state()
        .add_item(&items[3].set_id, "big unkept", &[], mp4_bytes(900 * 1024));
    {
        // A larger still in the kept pack, so that the kept files and the
        // copy do not fit in 0.8.0's 1 MiB together.
        let mut st = server.state();
        let it = st.add_item(&items[0].set_id, "large kept", &[], png_bytes(8, 8, 6));
        st.add_variant(&it.id, Variant::Original, "image/png", vec![1u8; 300 * 1024]);
    }
    let config_path = dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            "server_url = \"{}\"\ntoken_file = \"{}\"\ncache_dir = \"{}\"\ncache_max_mb = 1\n\n[offline]\nmax_mb = 64\n",
            server.url,
            dir.path().join("state/token").display(),
            dir.path().join("cache").display()
        ),
    )
    .unwrap();
    let cache = cache_dir(dir.path());
    let stickpile = |args: &[&str]| {
        let out = std::process::Command::new(&cli)
            .arg("--config")
            .arg(&config_path)
            .args(args)
            .env("HOME", dir.path())
            .env("XDG_CONFIG_HOME", dir.path().join("xdg-config"))
            .env("XDG_CACHE_HOME", dir.path().join("xdg-cache"))
            .env("XDG_STATE_HOME", dir.path().join("xdg-state"))
            .output()
            .unwrap();
        let text = format!(
            "$ stickpile {}\nexit {}\n{}{}",
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        println!("{text}");
        (out.status.success(), text)
    };

    // The new client keeps the pack `cats` and downloads it.
    let mut config = crate::ClientConfig::load(&config_path).unwrap();
    config.offline.auto.thumbnails = false;
    login_token(&server, dir.path());
    let session = Session::open(config.clone()).await.unwrap();
    session.sync().await.unwrap();
    session.offline_keep(&Scope::Set(items[0].set_id.clone())).unwrap();
    assert!(run(&session, &quick()).await.is_complete());
    let kept_files = kept(session.cache());
    println!("kept by the new client: {kept_files:?}");
    assert_eq!(kept_files.len(), 8);
    drop(session);

    // 0.8.0 opens it: version, sync, an offline search, a copy that needs
    // room under its 1 MiB cap.
    let (ok, text) = stickpile(&["--version"]);
    assert!(ok && text.contains("0.8.0"), "{text}");
    let (ok, text) = stickpile(&["sync"]);
    assert!(ok, "{text}");
    let (ok, text) = stickpile(&["--json", "search", "cat"]);
    assert!(ok, "{text}");
    let (ok, text) = stickpile(&["copy", "--id", &big.id, "--print-path"]);
    assert!(ok, "{text}");
    let after_080 = on_disk(&cache);
    println!("files after 0.8.0: {after_080:?}");
    let evicted: Vec<&String> = kept_files.iter().filter(|p| !after_080.contains(*p)).collect();
    println!("kept files 0.8.0 evicted: {evicted:?}");
    assert!(!evicted.is_empty(), "with a 1 MiB cap 0.8.0 evicts kept files");

    // The new client heals.
    let session = Session::open(config).await.unwrap();
    let plan = session.offline_refresh().unwrap();
    println!(
        "new client: {} of {} kept files present before the run",
        plan.totals.present_files, plan.totals.files
    );
    let out = run(&session, &quick()).await;
    println!("run: {:?}", out.end);
    assert!(out.is_complete(), "{out:?}");
    assert_eq!(kept(session.cache()), kept_files);
    for p in &kept_files {
        assert!(cache.join(p).exists(), "{p}");
    }
    println!("healed: every kept file is back and flagged");
}
