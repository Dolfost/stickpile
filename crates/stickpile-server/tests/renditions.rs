//! The Discord and Signal renditions made in the publish jobs
//! (`service::renditions`, `stickpile_media::pack`), checked with the
//! platform crates' own rules: still and animated inputs, padding, cuts.

mod common;

use std::path::Path;

use common::{png, TestServer};
use stickpile_api::{MediaKind, Visibility};
use stickpile_server::service::hydrate;
use stickpile_server::service::renditions::{self, Kind, Rendered};

/// A GIF of `frames` frames of 100 ms, each one different.
fn long_gif(frames: u64) -> Vec<u8> {
    use image::codecs::gif::{GifEncoder, Repeat};
    use image::{Delay, Frame};
    let mut out = Vec::new();
    {
        let mut enc = GifEncoder::new(&mut out);
        enc.set_repeat(Repeat::Infinite).unwrap();
        for i in 0..frames {
            let img = common::image(48, 48, 100 + i, (i * 3) as u8);
            enc.encode_frame(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(100, 1)))
                .unwrap();
        }
    }
    out
}

/// A 4 s test video (ffmpeg's `testsrc`), if ffmpeg is there.
fn video(dir: &Path) -> Option<Vec<u8>> {
    if !common::ffmpeg_available() {
        return None;
    }
    let out = dir.join("clip.mp4");
    let ok = std::process::Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=4:size=160x120:rate=25",
            "-pix_fmt",
            "yuv420p",
            "-y",
        ])
        .arg(&out)
        .status()
        .is_ok_and(|s| s.success());
    ok.then(|| std::fs::read(&out).unwrap())
}

#[tokio::test]
async fn renditions_fit_every_platform() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let set = srv.create_set(&alice, "shapes", Visibility::Private).await;
    let wide = common::encode(&common::image(300, 100, 1, 0), image::ImageFormat::Png);
    srv.upload_one(&alice, &set.id, "wide.png", wide).await;
    srv.upload_one(&alice, &set.id, "photo.jpg", common::jpeg(2)).await;
    srv.upload_one(&alice, &set.id, "small.png", png(3)).await;
    srv.upload_one(&alice, &set.id, "short.gif", common::gif_anim(4)).await;
    srv.upload_one(&alice, &set.id, "long.gif", long_gif(60)).await;
    let tmp = tempfile::tempdir().unwrap();
    let video = video(tmp.path());
    if let Some(v) = &video {
        srv.upload_one(&alice, &set.id, "clip.mp4", v.clone()).await;
    }
    let state = &srv.state;
    let rows = hydrate::fetch_by_set(&state.db, &set.id).await.unwrap();
    let media_ids: Vec<String> = rows.iter().map(|r| r.media_id.clone()).collect();
    let variants = hydrate::variant_rows(&state.db, &media_ids).await.unwrap();

    for kind in [Kind::DiscordSticker, Kind::DiscordEmoji, Kind::Signal] {
        let target = kind.target();
        for r in &rows {
            let original = renditions::original(state, &variants, r);
            let made = match renditions::render(state, r, original, kind).await.unwrap() {
                Rendered::Made(m) => m,
                Rendered::Skip(why) => panic!("{kind:?} {}: {why}", r.name),
            };
            assert!(made.bytes.len() as u64 <= target.max_bytes, "{kind:?} {}", r.name);
            // The platform crates' own checks, and the shape.
            let (w, h, animated, ms) = match kind {
                Kind::Signal => {
                    let i = stickpile_signal::check_sticker(&made.bytes).unwrap();
                    (i.width, i.height, i.animated, i.duration_ms)
                }
                _ => {
                    let k = match kind {
                        Kind::DiscordSticker => stickpile_discord::ExpressionKind::Sticker,
                        _ => stickpile_discord::ExpressionKind::Emoji,
                    };
                    let i = stickpile_discord::check_file(k, &made.bytes).unwrap();
                    (i.width, i.height, i.animated, i.duration_ms)
                }
            };
            assert_eq!(w, h, "{kind:?} {}: square", r.name);
            let moving = r.kind() != MediaKind::Still;
            assert_eq!(animated, moving, "{kind:?} {}", r.name);
            assert_eq!(made.animated, moving);
            if moving {
                assert!(w <= target.side && w >= target.side / 4, "{kind:?} {}: {w}", r.name);
                let ms = u64::from(ms.unwrap_or(0));
                assert!(ms > 0 && ms <= target.max_animation_ms, "{kind:?} {}: {ms} ms", r.name);
            } else {
                assert_eq!(w, target.side, "{kind:?} {}", r.name);
            }
            // Only what is longer than the platform takes is cut.
            let cut = match (r.name.as_str(), kind) {
                ("long", Kind::DiscordSticker) => Some("cut to the first 5 seconds"),
                ("long", Kind::Signal) | ("clip", Kind::Signal) => Some("cut to the first 3 seconds"),
                _ => None,
            };
            assert_eq!(made.note.as_deref(), cut, "{kind:?} {}", r.name);
            if r.name == "wide" {
                let img = image::load_from_memory(&made.bytes).unwrap().to_rgba8();
                assert_eq!(img.get_pixel(w / 2, 2)[3], 0, "{kind:?}: padded above");
                assert_eq!(img.get_pixel(w / 2, h / 2)[3], 255);
            }
        }
    }
    // Without ffmpeg a video cannot be made into a sticker; say so.
    if video.is_none() {
        eprintln!("ffmpeg not found: videos were not tried");
    }
}
