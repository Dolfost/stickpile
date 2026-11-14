use image::{GenericImageView, ImageFormat};
use stickpile_api::{TargetApp, Variant};

use super::*;
use crate::tests::{config_for, login_token, seeded_server};
use crate::Session;

const RED: Rgba<u8> = Rgba([255, 0, 0, 255]);

fn encode(img: &RgbaImage, format: ImageFormat) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    match format {
        ImageFormat::Jpeg => image::DynamicImage::ImageRgba8(img.clone())
            .to_rgb8()
            .write_to(&mut out, format)
            .unwrap(),
        _ => img.write_to(&mut out, format).unwrap(),
    }
    out.into_inner()
}

/// Opaque red with a fully transparent column at x = 0.
fn sample(w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_fn(w, h, |x, _| if x == 0 { Rgba([0, 0, 255, 0]) } else { RED })
}

fn cfg(square: SquarePad, background: Background) -> CopyConfig {
    CopyConfig { square, background }
}

#[test]
fn pad_square_centres_without_scaling() {
    let img = RgbaImage::from_pixel(40, 20, RED);
    let t = pad_square(&img, TRANSPARENT);
    assert_eq!(t.dimensions(), (40, 40));
    assert_eq!(*t.get_pixel(0, 0), TRANSPARENT);
    assert_eq!(*t.get_pixel(39, 9), TRANSPARENT);
    assert_eq!(*t.get_pixel(0, 10), RED);
    assert_eq!(*t.get_pixel(39, 29), RED);
    assert_eq!(*t.get_pixel(20, 30), TRANSPARENT);

    let tall = RgbaImage::from_pixel(3, 8, RED);
    let w = pad_square(&tall, WHITE);
    assert_eq!(w.dimensions(), (8, 8));
    assert_eq!(*w.get_pixel(1, 0), WHITE);
    assert_eq!(*w.get_pixel(2, 0), RED);
    assert_eq!(*w.get_pixel(4, 7), RED);
    assert_eq!(*w.get_pixel(5, 7), WHITE);

    // White margins do not touch the image's own alpha.
    let ghost = sample(4, 2);
    let p = pad_square(&ghost, WHITE);
    assert_eq!(*p.get_pixel(0, 0), WHITE);
    assert_eq!(p.get_pixel(0, 1)[3], 0);

    let square = RgbaImage::from_pixel(5, 5, RED);
    assert_eq!(pad_square(&square, WHITE), square);
}

#[test]
fn flatten_composites_onto_white() {
    let mut img = RgbaImage::from_vec(3, 1, vec![255, 0, 0, 0, 0, 0, 0, 128, 10, 20, 30, 255]).unwrap();
    flatten_onto_white(&mut img);
    assert_eq!(*img.get_pixel(0, 0), WHITE);
    assert_eq!(*img.get_pixel(1, 0), Rgba([127, 127, 127, 255]));
    assert_eq!(*img.get_pixel(2, 0), Rgba([10, 20, 30, 255]));
}

#[test]
fn tags_and_cache_paths_differ_per_combination() {
    use Background::Keep;
    use SquarePad::{Off, Transparent};
    let bg_white = Background::White;
    assert_eq!(adjust_tag(&cfg(Off, Keep), false), None);
    assert_eq!(adjust_tag(&cfg(Off, bg_white), true), None, "stickers keep their alpha");
    assert_eq!(adjust_tag(&cfg(Off, bg_white), false).as_deref(), Some("bg-white"));
    assert_eq!(
        adjust_tag(&cfg(Transparent, Keep), true).as_deref(),
        Some("sq-transparent")
    );
    assert_eq!(
        adjust_tag(&cfg(SquarePad::White, bg_white), false).as_deref(),
        Some("sq-white.bg-white")
    );
    let mut tags = std::collections::HashSet::new();
    for sq in [Off, Transparent, SquarePad::White] {
        for bg in [Keep, bg_white] {
            tags.insert(adjust_tag(&cfg(sq, bg), false));
        }
    }
    assert_eq!(tags.len(), 6);
    assert_eq!(
        adjusted_rel("clip/m1.original/grumpy cat.jpg", "sq-white", "png"),
        "clip/m1.original.sq-white/grumpy cat.png"
    );
    assert_eq!(adjusted_rel("x.webp", "bg-white", "png"), "bg-white/x.png");
}

#[test]
fn still_is_padded_and_flattened_into_png() {
    let png = encode(&sample(30, 20), ImageFormat::Png);

    let a = adjust_image(&png, false, &cfg(SquarePad::Transparent, Background::Keep))
        .unwrap()
        .unwrap();
    assert_eq!(a.mime, "image/png");
    let img = image::load_from_memory_with_format(&a.bytes, ImageFormat::Png).unwrap();
    assert_eq!(img.dimensions(), (30, 30));
    let img = img.to_rgba8();
    assert_eq!(img.get_pixel(15, 0)[3], 0, "transparent margin");
    assert_eq!(*img.get_pixel(15, 15), RED);
    assert_eq!(img.get_pixel(0, 15)[3], 0, "own transparency kept");

    let a = adjust_image(&png, false, &cfg(SquarePad::Off, Background::White))
        .unwrap()
        .unwrap();
    let img = image::load_from_memory(&a.bytes).unwrap();
    assert_eq!(img.dimensions(), (30, 20));
    assert!(!img.color().has_alpha(), "opaque result is written as RGB");
    assert_eq!(img.to_rgba8().get_pixel(0, 5), &WHITE);

    let a = adjust_image(&png, false, &cfg(SquarePad::Transparent, Background::White))
        .unwrap()
        .unwrap();
    let img = image::load_from_memory(&a.bytes).unwrap().to_rgba8();
    assert_eq!(img.dimensions(), (30, 30));
    assert_eq!(img.get_pixel(15, 0)[3], 0, "margins use the square colour");
    assert_eq!(*img.get_pixel(0, 15), WHITE, "the image itself is flattened");

    let a = adjust_image(&png, false, &cfg(SquarePad::White, Background::White))
        .unwrap()
        .unwrap();
    let img = image::load_from_memory(&a.bytes).unwrap();
    assert!(!img.color().has_alpha());
    assert_eq!(img.to_rgba8().get_pixel(15, 2), &WHITE);
}

#[test]
fn nothing_to_do_returns_none() {
    let opaque_square = encode(&RgbaImage::from_pixel(16, 16, RED), ImageFormat::Png);
    let all = cfg(SquarePad::White, Background::White);
    assert_eq!(adjust_image(&opaque_square, false, &all).unwrap(), None);
    let wide = encode(&sample(30, 20), ImageFormat::Png);
    assert_eq!(adjust_image(&wide, false, &CopyConfig::default()).unwrap(), None);
    assert_eq!(
        adjust_image(&wide, true, &cfg(SquarePad::Off, Background::White)).unwrap(),
        None
    );
    let gif = crate::clipboard::tests::animated_gif(3);
    assert_eq!(adjust_image(&gif, false, &all).unwrap(), None, "animations stay");
    assert!(adjust_image(b"not an image", false, &all).is_err());
}

#[test]
fn jpeg_and_webp_stills_become_png() {
    let jpeg = encode(&RgbaImage::from_pixel(20, 10, RED), ImageFormat::Jpeg);
    let a = adjust_image(&jpeg, false, &cfg(SquarePad::White, Background::Keep))
        .unwrap()
        .unwrap();
    assert_eq!(a.mime, "image/png");
    let img = image::load_from_memory_with_format(&a.bytes, ImageFormat::Png).unwrap();
    assert_eq!(img.dimensions(), (20, 20));

    let webp = encode(&sample(12, 20), ImageFormat::WebP);
    let a = adjust_image(&webp, false, &cfg(SquarePad::Transparent, Background::Keep))
        .unwrap()
        .unwrap();
    let img = image::load_from_memory_with_format(&a.bytes, ImageFormat::Png).unwrap();
    assert_eq!(img.dimensions(), (20, 20));
}

#[test]
fn sticker_is_padded_to_512_square_webp() {
    let sticker = encode(&sample(512, 300), ImageFormat::WebP);
    for (square, corner_alpha) in [(SquarePad::Transparent, 0u8), (SquarePad::White, 255)] {
        let a = adjust_image(&sticker, true, &cfg(square, Background::White))
            .unwrap()
            .unwrap();
        assert_eq!(a.mime, "image/webp");
        assert!(a.bytes.len() <= 512 * 1024);
        let img = image::load_from_memory_with_format(&a.bytes, ImageFormat::WebP).unwrap();
        assert_eq!(img.dimensions(), (512, 512), "longest side stays 512");
        let img = img.to_rgba8();
        let corner = img.get_pixel(256, 5);
        assert_eq!(corner[3], corner_alpha, "{square:?}");
        if square == SquarePad::White {
            assert!(corner[0] > 245 && corner[1] > 245 && corner[2] > 245, "{corner:?}");
        }
        assert_eq!(img.get_pixel(0, 256)[3], 0, "stickers are never flattened");
        let mid = img.get_pixel(256, 256);
        assert!(mid[0] > 200 && mid[1] < 60 && mid[3] == 255, "{mid:?}");
        // Lossless: the padded sticker holds the source pixels unchanged
        // (colours under fully transparent pixels do not count).
        assert_eq!(&a.bytes[12..16], b"VP8L");
        let src = image::load_from_memory(&sticker).unwrap().to_rgba8();
        let top = (512 - 300) / 2;
        for (x, y, p) in src.enumerate_pixels() {
            let q = img.get_pixel(x, y + top);
            assert!(p == q || (p[3] == 0 && q[3] == 0), "({x}, {y}): {p:?} -> {q:?}");
        }
    }
    let square_sticker = encode(&sample(512, 512), ImageFormat::WebP);
    assert_eq!(
        adjust_image(&square_sticker, true, &cfg(SquarePad::White, Background::Keep)).unwrap(),
        None
    );
}

async fn session_with(server: &crate::mock::MockServer, dir: &Path, copy: CopyConfig) -> Session {
    login_token(server, dir);
    let mut config = config_for(&server.url, dir);
    config.copy = copy;
    Session::open(config).await.unwrap()
}

#[tokio::test]
async fn prepared_copy_carries_the_adjusted_pixels() {
    let (server, items) = seeded_server();
    let ghost = {
        let mut st = server.state();
        st.add_item(
            &items[0].set_id,
            "ghost",
            &[],
            encode(&sample(30, 20), ImageFormat::Png),
        )
    };
    let dir = tempfile::tempdir().unwrap();
    let both = cfg(SquarePad::White, Background::White);
    let session = session_with(&server, dir.path(), both).await;

    let p = session.prepare_copy(&ghost, TargetApp::Discord, false).await.unwrap();
    let rel = p.file.strip_prefix(dir.path().join("cache")).unwrap();
    assert_eq!(
        rel,
        Path::new(&format!("clip/{}.original.sq-white.bg-white/ghost.png", ghost.media_id))
    );
    let mimes: Vec<_> = p.offers.iter().map(|o| o.mime.as_str()).collect();
    assert_eq!(mimes, ["image/png", "text/uri-list", "x-special/gnome-copied-files"]);
    assert_eq!(p.offers[0].data, std::fs::read(&p.file).unwrap(), "data == file");
    let img = image::load_from_memory_with_format(&p.offers[0].data, ImageFormat::Png).unwrap();
    assert_eq!(img.dimensions(), (30, 30));
    assert!(!img.color().has_alpha());
    assert!(String::from_utf8_lossy(&p.offers[1].data).contains("sq-white.bg-white/ghost.png"));

    // Cached: the same file, not rewritten.
    let mtime = std::fs::metadata(&p.file).unwrap().modified().unwrap();
    let again = session.prepare_copy(&ghost, TargetApp::Discord, false).await.unwrap();
    assert_eq!(again.file, p.file);
    assert_eq!(std::fs::metadata(&again.file).unwrap().modified().unwrap(), mtime);

    // Another combination gets its own file; the first stays as it was.
    let other = session_with(&server, dir.path(), cfg(SquarePad::Transparent, Background::Keep)).await;
    let t = other.prepare_copy(&ghost, TargetApp::Discord, false).await.unwrap();
    assert_ne!(t.file, p.file);
    assert!(t.file.to_string_lossy().contains(".original.sq-transparent/ghost.png"));
    let img = image::load_from_memory(&t.offers[0].data).unwrap().to_rgba8();
    assert_eq!(img.get_pixel(15, 0)[3], 0);
    assert_eq!(std::fs::read(&p.file).unwrap(), p.offers[0].data);

    // Options off: the downloaded original itself.
    let plain = session_with(&server, dir.path(), CopyConfig::default()).await;
    let o = plain.prepare_copy(&ghost, TargetApp::Discord, false).await.unwrap();
    assert!(o.file.to_string_lossy().ends_with(".original/ghost.png"));

    // Animations are left alone.
    let gif = session
        .prepare_copy(&items[1], TargetApp::Generic, false)
        .await
        .unwrap();
    assert_eq!(gif.file.extension().unwrap(), "gif");
    assert!(gif.file.parent().unwrap().to_string_lossy().ends_with(".original"));
    assert_eq!(gif.offers[0].data, crate::tests::gif_bytes(3, 2));
}

#[tokio::test]
async fn sticker_copy_is_padded_and_stays_a_sticker() {
    let (server, items) = seeded_server();
    let still = &items[0];
    server.state().add_variant(
        &still.id,
        Variant::TgSticker,
        "image/webp",
        encode(&sample(512, 300), ImageFormat::WebP),
    );
    let mut item = still.clone();
    item.variants = server
        .state()
        .items
        .iter()
        .find(|i| i.item.id == still.id)
        .unwrap()
        .item
        .media
        .variants
        .clone();
    let dir = tempfile::tempdir().unwrap();
    let session = session_with(&server, dir.path(), cfg(SquarePad::Transparent, Background::White)).await;
    let p = session.prepare_copy(&item, TargetApp::Telegram, true).await.unwrap();
    assert!(p.sticker);
    assert_eq!(p.variant, Variant::TgSticker);
    assert!(p
        .file
        .to_string_lossy()
        .ends_with(".tg_sticker.sq-transparent/grumpy cat.webp"));
    let bytes = std::fs::read(&p.file).unwrap();
    let img = image::load_from_memory_with_format(&bytes, ImageFormat::WebP).unwrap();
    assert_eq!(img.dimensions(), (512, 512));
    let mimes: Vec<_> = p.offers.iter().map(|o| o.mime.as_str()).collect();
    assert_eq!(
        mimes,
        [
            "text/uri-list",
            "x-special/gnome-copied-files",
            "application/x-sticker",
            "application/x-no-dialog"
        ]
    );
    assert_eq!(p.offers[0].data, crate::clipboard::uri_list(&p.file).into_bytes());
    assert_eq!(p.offers[2].data, b"1");
    assert_eq!(p.offers[3].data, b"1");
}

#[tokio::test]
async fn a_new_rendition_replaces_its_old_adjustment() {
    let (server, items) = seeded_server();
    let still = &items[0];
    let dir = tempfile::tempdir().unwrap();
    let session = session_with(&server, dir.path(), cfg(SquarePad::White, Background::Keep)).await;
    let refreshed = |server: &crate::mock::MockServer| {
        let mut item = still.clone();
        item.variants = server
            .state()
            .items
            .iter()
            .find(|i| i.item.id == still.id)
            .unwrap()
            .item
            .media
            .variants
            .clone();
        item
    };
    server.state().add_variant(
        &still.id,
        Variant::TgSticker,
        "image/webp",
        encode(&sample(512, 300), ImageFormat::WebP),
    );
    let first = session
        .prepare_copy(&refreshed(&server), TargetApp::Telegram, true)
        .await
        .unwrap();
    let old = std::fs::read(&first.file).unwrap();

    let tall = RgbaImage::from_fn(300, 512, |x, y| Rgba([x as u8, y as u8, 0, 255]));
    server.state().add_variant(
        &still.id,
        Variant::TgSticker,
        "image/webp",
        encode(&tall, ImageFormat::WebP),
    );
    let second = session
        .prepare_copy(&refreshed(&server), TargetApp::Telegram, true)
        .await
        .unwrap();
    assert_eq!(second.file, first.file);
    let new = std::fs::read(&second.file).unwrap();
    assert_ne!(new, old);
    let img = image::load_from_memory(&new).unwrap().to_rgba8();
    assert_eq!(
        img.get_pixel(5, 256)[3],
        255,
        "white side margins of the new tall sticker"
    );
}
