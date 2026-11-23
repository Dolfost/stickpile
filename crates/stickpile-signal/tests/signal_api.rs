//! Signal import, upload and the Signal-ready zip against the in-process
//! mock of Signal's service and CDN.

mod mock_signal;

use mock_signal::{png, start, webp, Shared, ACI, DEVICE, PASSWORD};
use stickpile_signal::crypto::{derive_keys, encrypt};
use stickpile_signal::{
    import_pack, signal_ready_files, signal_ready_zip, split_parts, upload_pack, upload_parts, Credentials,
    CryptoError, ImageFormat, Manifest, ManifestSticker, PackKey, PackLink, SignalClient, SignalConfig, SignalError,
    UploadPack, UploadSticker,
};

const PACK_ID: &str = "0123456789abcdef0123456789abcdef";

fn client(service: &str, cdn: &str) -> SignalClient {
    SignalClient::new(&SignalConfig {
        service_url: service.into(),
        cdn_url: cdn.into(),
        trust_public_roots: false,
    })
    .unwrap()
}

fn creds() -> Credentials {
    Credentials::new(ACI, DEVICE, PASSWORD).unwrap()
}

fn ms(id: u32, emoji: &str, ct: Option<&str>) -> ManifestSticker {
    ManifestSticker {
        id,
        emoji: Some(emoji.into()),
        content_type: ct.map(str::to_string),
    }
}

/// Puts an encrypted pack on the mock CDN: `files` by sticker id.
fn seed_pack(state: &Shared, key: &PackKey, manifest: &Manifest, files: &[(u32, Vec<u8>)]) {
    let keys = derive_keys(key.as_bytes());
    let mut st = state.lock().unwrap();
    st.objects.insert(
        format!("stickers/{PACK_ID}/manifest.proto"),
        encrypt(&keys, &manifest.encode()),
    );
    for (id, bytes) in files {
        st.objects
            .insert(format!("stickers/{PACK_ID}/full/{id}"), encrypt(&keys, bytes));
    }
}

fn link(key: &PackKey) -> PackLink {
    PackLink::new(PACK_ID, key.clone()).unwrap()
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

#[tokio::test]
async fn import_reads_and_decrypts_a_pack() {
    let (service, cdn, state) = start().await;
    let key = PackKey::random();
    let manifest = Manifest {
        title: Some("Cats".into()),
        author: Some("Someone".into()),
        cover: Some(ms(4, "", Some("image/webp"))),
        stickers: vec![
            ms(0, "😼", Some("image/webp")),
            ms(1, "😾", None),
            ms(2, "🙀", Some("image/png")),
            ms(3, "😿", None), // missing on the CDN
            ms(5, "😹", None), // damaged
        ],
    };
    seed_pack(
        &state,
        &key,
        &manifest,
        &[
            (0, webp(512, 1)),
            (1, png(512, None, 2)),
            (2, png(512, Some(3), 3)),
            (4, webp(512, 4)),
        ],
    );
    {
        // A file encrypted with another key fails its MAC.
        let other = derive_keys(PackKey::random().as_bytes());
        state
            .lock()
            .unwrap()
            .objects
            .insert(format!("stickers/{PACK_ID}/full/5"), encrypt(&other, &webp(512, 5)));
    }
    let c = client(&service, &cdn);
    // Both link forms reach the same pack.
    for text in [link(&key).url(), link(&key).app_url()] {
        let parsed = PackLink::parse(&text).unwrap();
        let pack = import_pack(&c, &parsed, 1 << 20).await.unwrap();
        assert_eq!((pack.title.as_str(), pack.author.as_str()), ("Cats", "Someone"));
        assert_eq!(pack.pack_id, PACK_ID);
        let got: Vec<(u32, Option<&str>, ImageFormat, bool)> = pack
            .stickers
            .iter()
            .map(|s| (s.id, s.emoji.as_deref(), s.format, s.animated))
            .collect();
        assert_eq!(
            got,
            [
                (0, Some("😼"), ImageFormat::Webp, false),
                (1, Some("😾"), ImageFormat::Png, false),
                (2, Some("🙀"), ImageFormat::Apng, true),
            ]
        );
        assert_eq!(pack.stickers[0].bytes, webp(512, 1));
        assert_eq!(pack.stickers[0].content_type.as_deref(), Some("image/webp"));
        assert_eq!(pack.stickers[2].bytes, png(512, Some(3), 3));
        assert_eq!(pack.cover_id, Some(4));
        assert_eq!(pack.cover.as_ref().unwrap().bytes, webp(512, 4));
        let skipped: Vec<u32> = pack.skipped.iter().map(|(id, _)| *id).collect();
        assert_eq!(skipped, [3, 5]);
        assert!(pack.skipped[0].1.contains("missing"), "{:?}", pack.skipped);
        assert!(pack.skipped[1].1.contains("MAC"), "{:?}", pack.skipped);
    }
    let st = state.lock().unwrap();
    assert!(!st.cdn_saw_auth);
    assert!(st.downloads.contains(&format!("/stickers/{PACK_ID}/manifest.proto")));
    assert!(st.downloads.contains(&format!("/stickers/{PACK_ID}/full/2")));
}

#[tokio::test]
async fn import_errors_are_clear_and_never_show_the_key() {
    let (service, cdn, state) = start().await;
    let c = client(&service, &cdn);
    let key = PackKey::random();
    let key_hex = link(&key).url().rsplit('=').next().unwrap().to_string();

    // No such pack: the CDN's 403.
    let err = import_pack(&c, &link(&key), 1 << 20).await.unwrap_err();
    assert!(matches!(err, SignalError::NotFound), "{err:?}");

    // The wrong key fails the manifest's MAC.
    let manifest = Manifest {
        title: Some("T".into()),
        author: Some("A".into()),
        cover: None,
        stickers: vec![ms(0, "😼", None)],
    };
    seed_pack(&state, &key, &manifest, &[(0, png(512, None, 1))]);
    let wrong = PackKey::random();
    let err = import_pack(&c, &link(&wrong), 1 << 20).await.unwrap_err();
    assert!(matches!(err, SignalError::Crypto(CryptoError::BadMac)), "{err:?}");

    // A manifest that is not a protobuf message.
    let keys = derive_keys(key.as_bytes());
    state.lock().unwrap().objects.insert(
        format!("stickers/{PACK_ID}/manifest.proto"),
        encrypt(&keys, &[0x0B, 0x0C]),
    );
    let err = import_pack(&c, &link(&key), 1 << 20).await.unwrap_err();
    assert!(matches!(err, SignalError::Manifest(_)), "{err:?}");

    // Files over the cap are skipped, the import goes on.
    seed_pack(&state, &key, &manifest, &[(0, png(512, None, 1))]);
    let pack = import_pack(&c, &link(&key), 10).await.unwrap();
    assert!(pack.stickers.is_empty());
    assert!(pack.skipped[0].1.contains("larger than 10"), "{:?}", pack.skipped);
    assert_eq!(pack.cover_id, Some(0), "no cover in the manifest: the first sticker");

    // Unreachable CDN: no URL (it names the pack) and no key in the error.
    let unreachable = client("http://127.0.0.1:1", "http://127.0.0.1:1");
    let err = import_pack(&unreachable, &link(&key), 1 << 20).await.unwrap_err();
    let text = format!("{err} {err:?}");
    assert!(!text.contains(&key_hex) && !text.contains(PACK_ID), "{text}");
}

// ---------------------------------------------------------------------------
// Upload
// ---------------------------------------------------------------------------

fn upload_sticker(bytes: Vec<u8>, emoji: &str) -> UploadSticker {
    UploadSticker {
        bytes,
        emoji: emoji.into(),
    }
}

fn sample_pack() -> UploadPack {
    UploadPack {
        title: "Cats".into(),
        author: "stickpile".into(),
        stickers: vec![
            upload_sticker(webp(512, 1), "😼"),
            upload_sticker(png(512, None, 2), "😾"),
            upload_sticker(png(512, Some(3), 3), "🙀"),
        ],
        cover: Some(webp(512, 9)),
    }
}

#[tokio::test]
async fn an_uploaded_pack_reads_back_through_its_link() {
    let (service, cdn, state) = start().await;
    let c = client(&service, &cdn);
    let pack = sample_pack();
    let uploaded = upload_pack(&c, &creds(), &pack).await.unwrap();
    assert_eq!(uploaded.stickers, 3);
    assert_eq!(uploaded.cover_id, 3, "the separate cover gets the next id");
    let url = uploaded.link.url();
    assert!(url.starts_with("https://signal.art/addstickers/#pack_id="), "{url}");
    {
        let st = state.lock().unwrap();
        assert_eq!(st.forms, [(format!("{ACI}.{DEVICE}"), 4)]);
        assert!(st.refused.is_empty(), "{:?}", st.refused);
        assert_eq!(st.objects.len(), 5, "manifest + 3 stickers + cover");
        assert!(!st.cdn_saw_auth, "the CDN never sees the account password");
        // Stored encrypted: no plaintext on the CDN.
        assert!(st
            .objects
            .values()
            .all(|blob| !blob.starts_with(b"RIFF") && !blob.starts_with(b"\x89PNG")));
    }

    // The link the upload returned imports the same pack.
    let parsed = PackLink::parse(&url).unwrap();
    assert_eq!(parsed, uploaded.link);
    let back = import_pack(&c, &parsed, 1 << 20).await.unwrap();
    assert_eq!((back.title.as_str(), back.author.as_str()), ("Cats", "stickpile"));
    assert!(back.skipped.is_empty(), "{:?}", back.skipped);
    assert_eq!(back.stickers.len(), 3);
    for (got, want) in back.stickers.iter().zip(&pack.stickers) {
        assert_eq!(got.bytes, want.bytes);
        assert_eq!(got.emoji.as_deref(), Some(want.emoji.as_str()));
    }
    assert_eq!(back.stickers[0].content_type.as_deref(), Some("image/webp"));
    assert_eq!(back.stickers[2].format, ImageFormat::Apng);
    assert_eq!(back.cover_id, Some(3));
    assert_eq!(back.cover.unwrap().bytes, webp(512, 9));

    // Packs are immutable: publishing again makes a new pack and key.
    let again = upload_pack(&c, &creds(), &pack).await.unwrap();
    assert_ne!(again.link.pack_id, uploaded.link.pack_id);
    assert_ne!(again.link.key, uploaded.link.key);

    // The key is in the URL only, never in Debug output.
    let key_hex = url.rsplit('=').next().unwrap();
    assert!(!format!("{uploaded:?}").contains(key_hex));

    // Without a separate cover the first sticker is the cover.
    let plain = UploadPack {
        cover: None,
        ..pack.clone()
    };
    let p = upload_pack(&c, &creds(), &plain).await.unwrap();
    assert_eq!(p.cover_id, 0);
    let back = import_pack(&c, &p.link, 1 << 20).await.unwrap();
    assert_eq!(back.cover_id, Some(0));
    assert!(back.cover.is_none());
    assert_eq!(state.lock().unwrap().forms.last().unwrap().1, 3);
}

#[tokio::test]
async fn upload_refusals_name_the_problem_and_hide_the_password() {
    let (service, cdn, state) = start().await;
    let c = client(&service, &cdn);

    let wrong = Credentials::new(ACI, DEVICE, "SECRET-wrong").unwrap();
    let err = upload_pack(&c, &wrong, &sample_pack()).await.unwrap_err();
    assert!(matches!(err, SignalError::Unauthorized), "{err:?}");
    let text = format!("{err} {err:?} {wrong:?}");
    assert!(!text.contains("SECRET"), "{text}");

    state.lock().unwrap().rate_limited = 1;
    let err = upload_pack(&c, &creds(), &sample_pack()).await.unwrap_err();
    assert!(
        matches!(
            err,
            SignalError::RateLimited {
                retry_after: Some(4320)
            }
        ),
        "{err:?}"
    );
    assert!(err.to_string().contains("4320"), "{err}");

    // An invalid pack is refused before anything is sent.
    let mut bad = sample_pack();
    bad.stickers.push(upload_sticker(png(1024, None, 7), "😼"));
    let err = upload_pack(&c, &creds(), &bad).await.unwrap_err();
    assert!(
        err.to_string().contains("sticker 4") && err.to_string().contains("1024x1024"),
        "{err}"
    );
    {
        let st = state.lock().unwrap();
        assert_eq!(st.unauthorized, 1);
        assert!(st.forms.is_empty(), "the rate-limited and invalid uploads got no form");
        assert!(st.objects.is_empty());
    }

    // Network failure: no password, no URL.
    let unreachable = client("http://127.0.0.1:1", "http://127.0.0.1:1");
    let err = upload_pack(&unreachable, &creds(), &sample_pack()).await.unwrap_err();
    let text = format!("{err} {err:?}");
    assert!(!text.contains("SECRET") && !text.contains("127.0.0.1:1/v1"), "{text}");
}

// ---------------------------------------------------------------------------
// Signal-ready zip
// ---------------------------------------------------------------------------

/// Reads a stored (uncompressed) zip through its central directory and
/// checks every CRC.
fn read_zip(zip: &[u8]) -> Vec<(String, Vec<u8>)> {
    let u16_at = |at: usize| u16::from_le_bytes(zip[at..at + 2].try_into().unwrap()) as usize;
    let u32_at = |at: usize| u32::from_le_bytes(zip[at..at + 4].try_into().unwrap()) as usize;
    let eocd = zip.len() - 22;
    assert_eq!(u32_at(eocd), 0x0605_4b50);
    let count = u16_at(eocd + 10);
    let mut at = u32_at(eocd + 16);
    let mut out = Vec::new();
    for _ in 0..count {
        assert_eq!(u32_at(at), 0x0201_4b50);
        assert_eq!(u16_at(at + 10), 0, "stored");
        let crc = u32_at(at + 16) as u32;
        let size = u32_at(at + 20);
        let name_len = u16_at(at + 28);
        let local = u32_at(at + 42);
        let name = String::from_utf8(zip[at + 46..at + 46 + name_len].to_vec()).unwrap();
        assert_eq!(u32_at(local), 0x0403_4b50);
        let data_at = local + 30 + u16_at(local + 26) + u16_at(local + 28);
        let data = zip[data_at..data_at + size].to_vec();
        assert_eq!(crc32fast::hash(&data), crc, "{name}");
        out.push((name, data));
        at += 46 + name_len;
    }
    out
}

#[test]
fn the_signal_ready_zip_has_images_emoji_and_a_signal_cli_manifest() {
    let pack = sample_pack();
    let zip = signal_ready_zip(&pack).unwrap();
    let files = read_zip(&zip);
    let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        [
            "001.webp",
            "002.png",
            "003.png",
            "cover.webp",
            "manifest.json",
            "emoji.txt",
            "README.txt"
        ]
    );
    assert_eq!(files[0].1, pack.stickers[0].bytes);
    assert_eq!(files[2].1, pack.stickers[2].bytes);
    assert_eq!(files[3].1, pack.cover.clone().unwrap());

    let manifest: serde_json::Value = serde_json::from_slice(&files[4].1).unwrap();
    assert_eq!(manifest["title"], "Cats");
    assert_eq!(manifest["author"], "stickpile");
    assert_eq!(manifest["stickers"][0]["file"], "001.webp");
    assert_eq!(manifest["stickers"][0]["emoji"], "😼");
    assert_eq!(manifest["stickers"][0]["contentType"], "image/webp");
    assert_eq!(manifest["stickers"][2]["contentType"], "image/png");
    assert_eq!(manifest["cover"]["file"], "cover.webp");
    assert_eq!(
        String::from_utf8(files[5].1.clone()).unwrap(),
        "001.webp 😼\n002.png 😾\n003.png 🙀\n"
    );
    let readme = String::from_utf8(files[6].1.clone()).unwrap();
    assert!(readme.contains("Create/upload sticker pack") && readme.contains("uploadStickerPack"));
    assert!(readme.contains("Title: Cats"));

    // Without a separate cover, the first sticker is named as the cover.
    let files = signal_ready_files(&UploadPack { cover: None, ..pack }).unwrap();
    let manifest: serde_json::Value =
        serde_json::from_slice(&files.iter().find(|f| f.path == "manifest.json").unwrap().bytes).unwrap();
    assert_eq!(manifest["cover"]["file"], "001.webp");
    assert!(files.iter().all(|f| !f.path.starts_with("cover")));

    // Invalid packs are refused like uploads.
    assert!(signal_ready_zip(&UploadPack {
        title: String::new(),
        ..sample_pack()
    })
    .is_err());
}

// ---------------------------------------------------------------------------
// Sets larger than one pack
// ---------------------------------------------------------------------------

/// Sticker `k` of a large set: a file and an emoji of its own (U+1F300
/// onwards), so a pack read back says which sticker is which.
fn nth(k: u32) -> UploadSticker {
    let mut bytes = png(512, None, (k % 251) as u8);
    bytes.extend(k.to_be_bytes());
    upload_sticker(bytes, &char::from_u32(0x1F300 + k).unwrap().to_string())
}

fn index_of(s: &stickpile_signal::ImportedSticker) -> u32 {
    s.emoji.as_deref().and_then(|e| e.chars().next()).unwrap() as u32 - 0x1F300
}

fn set_of(n: u32, cover: Option<Vec<u8>>) -> UploadPack {
    UploadPack {
        title: "Cats".into(),
        author: "stickpile".into(),
        stickers: (0..n).map(nth).collect(),
        cover,
    }
}

/// Uploads `whole` as parts and reads every part back through its link:
/// (title, sticker indices, cover id, separate cover) per part.
async fn round_trip(whole: UploadPack) -> (Vec<(String, Vec<u32>, Option<u32>, bool)>, Shared) {
    let (service, cdn, state) = start().await;
    let c = client(&service, &cdn);
    let parts = split_parts(whole);
    let up = upload_parts(&c, &creds(), &parts).await.unwrap();
    assert!(up.failed.is_none(), "{:?}", up.failed);
    let numbers: Vec<u32> = up.uploaded.iter().map(|u| u.number).collect();
    assert_eq!(numbers, (1..=parts.len() as u32).collect::<Vec<_>>());
    let mut out = Vec::new();
    for (u, part) in up.uploaded.iter().zip(&parts) {
        assert_eq!(u.pack.stickers, part.stickers.len());
        let back = import_pack(&c, &PackLink::parse(&u.pack.link.url()).unwrap(), 1 << 20)
            .await
            .unwrap();
        assert!(back.skipped.is_empty(), "{:?}", back.skipped);
        assert_eq!(back.title, u.title);
        assert_eq!(back.author, "stickpile");
        for (got, want) in back.stickers.iter().zip(&part.stickers) {
            assert_eq!(got.bytes, want.bytes);
        }
        out.push((
            back.title.clone(),
            back.stickers.iter().map(index_of).collect(),
            back.cover_id,
            back.cover.is_some(),
        ));
    }
    (out, state)
}

#[tokio::test]
async fn signal_200_items_one_pack() {
    let (parts, state) = round_trip(set_of(200, None)).await;
    assert_eq!(parts.len(), 1);
    let (title, order, cover, separate) = &parts[0];
    assert_eq!(title, "Cats", "one pack keeps the title as it is");
    assert_eq!(order, &(0..200).collect::<Vec<_>>());
    assert_eq!((*cover, *separate), (Some(0), false));
    assert_eq!(state.lock().unwrap().forms.len(), 1);
}

#[tokio::test]
async fn signal_201_items_two_packs() {
    let cover = webp(512, 9);
    let (parts, state) = round_trip(set_of(201, Some(cover))).await;
    let got: Vec<(&str, usize)> = parts.iter().map(|p| (p.0.as_str(), p.1.len())).collect();
    assert_eq!(got, [("Cats", 200), ("Cats 2", 1)]);
    assert_eq!(parts[1].1, [200]);
    // Each part has the set's cover, as a file after its stickers.
    assert_eq!((parts[0].2, parts[0].3), (Some(200), true));
    assert_eq!((parts[1].2, parts[1].3), (Some(1), true));
    let forms: Vec<usize> = state.lock().unwrap().forms.iter().map(|f| f.1).collect();
    assert_eq!(forms, [201, 2]);
}

#[tokio::test]
async fn signal_450_items_three_packs_with_links() {
    let (parts, state) = round_trip(set_of(450, None)).await;
    let titles: Vec<&str> = parts.iter().map(|p| p.0.as_str()).collect();
    assert_eq!(titles, ["Cats", "Cats 2", "Cats 3"]);
    // Every sticker exactly once across the packs, in order.
    let all: Vec<u32> = parts.iter().flat_map(|p| p.1.clone()).collect();
    assert_eq!(all, (0..450).collect::<Vec<_>>());
    let sizes: Vec<usize> = parts.iter().map(|p| p.1.len()).collect();
    assert_eq!(sizes, [200, 200, 50]);
    // Without a cover each part shows its own first sticker.
    assert!(parts.iter().all(|p| p.2 == Some(0) && !p.3));
    let st = state.lock().unwrap();
    let forms: Vec<usize> = st.forms.iter().map(|f| f.1).collect();
    assert_eq!(forms, [200, 200, 50]);
    assert!(st.refused.is_empty(), "{:?}", st.refused);
    let packs: std::collections::HashSet<&str> = st
        .objects
        .keys()
        .filter_map(|k| k.strip_suffix("/manifest.proto"))
        .collect();
    assert_eq!(packs.len(), 3, "three new packs");
}

#[tokio::test]
async fn signal_failure_in_second_part_keeps_the_first() {
    let (service, cdn, state) = start().await;
    let c = client(&service, &cdn);
    let parts = split_parts(set_of(450, None));
    state.lock().unwrap().fail_form_after.insert(1, 413);
    let up = upload_parts(&c, &creds(), &parts).await.unwrap();
    assert_eq!(up.uploaded.len(), 1);
    assert_eq!((up.uploaded[0].number, up.uploaded[0].title.as_str()), (1, "Cats"));
    let (number, err) = up.failed.unwrap();
    assert_eq!(number, 2);
    assert!(
        matches!(
            err,
            SignalError::RateLimited {
                retry_after: Some(4320)
            }
        ),
        "{err:?}"
    );
    // Part 1 is on Signal and reads back; part 3 was not tried.
    let back = import_pack(&c, &up.uploaded[0].pack.link, 1 << 20).await.unwrap();
    assert_eq!(back.stickers.len(), 200);
    assert_eq!(state.lock().unwrap().forms.len(), 1);

    // An invalid part: nothing is sent, and the error names the part.
    let mut bad = split_parts(set_of(201, None));
    bad[1].stickers[0].bytes = png(1024, None, 1);
    let err = upload_parts(&c, &creds(), &bad).await.unwrap_err();
    assert!(err.to_string().starts_with("part 2 of 2: sticker 1"), "{err}");
    assert_eq!(
        state.lock().unwrap().forms.len(),
        1,
        "no form for an invalid set of parts"
    );
}
