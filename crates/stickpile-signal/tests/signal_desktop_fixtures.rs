//! Decrypts the encrypted sticker packs Signal-Desktop uses in its own tests
//! (`fixtures/stickerpack-<pack id>.bin` is the manifest, `-0.bin` sticker
//! 0; the keys are in `ts/test-mock/storage/fixtures.node.ts`). Real data
//! encrypted by Signal's code, so it checks the key derivation, the MAC and
//! the manifest format against a primary source.
//!
//! The files are not part of this repository. Download them from
//! https://github.com/signalapp/Signal-Desktop/tree/main/fixtures into a
//! directory and run:
//!
//! ```sh
//! STICKPILE_SIGNAL_DESKTOP_FIXTURES=/path/to/dir cargo test -p stickpile-signal --test signal_desktop_fixtures -- --ignored
//! ```

use stickpile_signal::crypto::{decrypt, derive_keys};
use stickpile_signal::{inspect, Manifest, PackKey};

const PACKS: [(&str, &str); 2] = [
    (
        "c40ed069cdc2b91eccfccf25e6bcddfc",
        "cefadd6e81c128680aead1711eb5c92c10f63bdfbc78528a4519ba682de396e4",
    ),
    (
        "ae8fedafda4768fd3384d4b3b9db963d",
        "53f4aa8b95e1c2e75afab2328fe67eb6d7affbcd4f50cd4da89dfc325dbc73ca",
    ),
];

#[test]
#[ignore = "needs Signal-Desktop's fixture files, see the module docs"]
fn signal_desktop_fixtures_decrypt() {
    let dir = std::env::var("STICKPILE_SIGNAL_DESKTOP_FIXTURES").expect("STICKPILE_SIGNAL_DESKTOP_FIXTURES");
    let dir = std::path::Path::new(&dir);
    for (id, key) in PACKS {
        let keys = derive_keys(PackKey::from_hex(key).unwrap().as_bytes());
        let manifest_blob = std::fs::read(dir.join(format!("stickerpack-{id}.bin"))).unwrap();
        let manifest = Manifest::decode(&decrypt(&keys, &manifest_blob).unwrap()).unwrap();
        println!("{id}: {manifest:?}");
        assert!(manifest.title.as_deref().is_some_and(|t| !t.is_empty()), "{manifest:?}");
        assert!(manifest.stickers.iter().any(|s| s.id == 0), "{manifest:?}");

        let sticker_blob = std::fs::read(dir.join(format!("stickerpack-{id}-0.bin"))).unwrap();
        let sticker = decrypt(&keys, &sticker_blob).unwrap();
        let info = inspect(&sticker).expect("sticker 0 is an image");
        println!("{id}: sticker 0 {info:?}, {} bytes", sticker.len());

        // The other pack's key fails the MAC.
        let other = PACKS.iter().find(|(i, _)| *i != id).unwrap().1;
        let wrong = derive_keys(PackKey::from_hex(other).unwrap().as_bytes());
        assert!(decrypt(&wrong, &manifest_blob).is_err());
    }
}
