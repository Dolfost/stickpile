//! Directory import/export.

use std::fs;
use std::path::Path;

use stickpile_setfmt::{
    read_dir, write_dir, FieldSource, ItemMeta, Manifest, SetBundle, SetFile, SetFmtError, Visibility, MANIFEST,
};

fn put(dir: &Path, name: &str, bytes: &[u8]) {
    fs::write(dir.join(name), bytes).unwrap();
}

const MANIFEST_TEXT: &str = r#"
version = 7
name = "Cats"
visibility = "public"
custom = "kept"

[items."001-a.png"]
name = "Alpha"
keywords = ["a"]
keywords_source = "ai"

[items."002-b.gif"]
emojis = ["🐱"]

[items."gone.png"]
name = "missing"
"#;

#[test]
#[cfg(unix)]
fn reads_a_set_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("cats");
    fs::create_dir(&dir).unwrap();
    put(&dir, MANIFEST, MANIFEST_TEXT.as_bytes());
    put(&dir, "002-b.gif", b"GIF89a");
    put(&dir, "001-a.png", b"png-a");
    put(&dir, "10-c.mp4", b"mp4");
    put(&dir, "cat.WEBP", b"webp");
    put(&dir, "notes.txt", b"text");
    put(&dir, ".hidden.png", b"hidden");
    put(&dir, ".DS_Store", b"junk");
    fs::create_dir(dir.join("sub")).unwrap();
    put(&dir.join("sub"), "x.png", b"nested");
    fs::create_dir(dir.join(".git")).unwrap();
    let outside = tmp.path().join("secret.png");
    put(tmp.path(), "secret.png", b"secret");
    std::os::unix::fs::symlink(&outside, dir.join("link.png")).unwrap();

    let b = read_dir(&dir).unwrap();
    let names: Vec<&str> = b.files.iter().map(|f| f.filename.as_str()).collect();
    assert_eq!(names, ["001-a.png", "002-b.gif", "10-c.mp4", "cat.WEBP"]);
    assert_eq!(b.manifest.version, 7);
    assert_eq!(b.manifest.name, "Cats");
    assert_eq!(b.manifest.visibility, Visibility::Public);
    assert_eq!(b.manifest.extra["custom"].as_str(), Some("kept"));

    assert_eq!(b.files[0].meta.name, "Alpha");
    assert_eq!(b.files[0].meta.keywords_source, FieldSource::Ai);
    assert_eq!(b.files[0].bytes, b"png-a");
    // Entry without a name gets the display stem; files without entry get defaults.
    assert_eq!(b.files[1].meta.name, "b");
    assert_eq!(b.files[1].meta.emojis, ["🐱"]);
    assert_eq!(
        b.files[2].meta,
        ItemMeta {
            name: "c".into(),
            ..Default::default()
        }
    );
    assert_eq!(b.files[3].meta.name, "cat");

    // manifest.items mirrors the files exactly.
    assert_eq!(b.manifest.items.keys().collect::<Vec<_>>(), names);
    assert_eq!(b.manifest.items["001-a.png"], b.files[0].meta);

    let w = b.warnings.join("\n");
    assert!(w.contains("gone.png"), "{w}");
    assert!(w.contains("notes.txt"), "{w}");
    assert!(w.contains("sub/"), "{w}");
    assert!(w.contains("link.png"), "{w}");
    assert!(
        !w.contains("hidden") && !w.contains("DS_Store") && !w.contains(".git"),
        "{w}"
    );
    assert!(!b.files.iter().any(|f| f.bytes == b"secret"));
}

#[test]
fn missing_manifest_uses_directory_name() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("my-memes");
    fs::create_dir(&dir).unwrap();
    put(&dir, "b.gif", b"b");
    put(&dir, "001-a.jpg", b"a");
    let b = read_dir(&dir).unwrap();
    assert_eq!(b.manifest.name, "my-memes");
    assert_eq!(b.manifest.version, 1);
    assert_eq!(
        b.files.iter().map(|f| f.meta.name.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert!(b.warnings.is_empty(), "{:?}", b.warnings);
}

#[test]
fn manifest_without_name_falls_back_to_directory_name() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("dogs");
    fs::create_dir(&dir).unwrap();
    put(&dir, MANIFEST, b"description = \"d\"\n");
    assert_eq!(read_dir(&dir).unwrap().manifest.name, "dogs");
}

#[test]
#[cfg(unix)]
fn symlinked_manifest_is_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("s");
    fs::create_dir(&dir).unwrap();
    put(tmp.path(), "evil.toml", b"name = \"evil\"");
    std::os::unix::fs::symlink(tmp.path().join("evil.toml"), dir.join(MANIFEST)).unwrap();
    let b = read_dir(&dir).unwrap();
    assert_eq!(b.manifest.name, "s");
    assert!(b.warnings.iter().any(|w| w.contains(MANIFEST)));
}

#[test]
fn invalid_manifest_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    put(tmp.path(), MANIFEST, b"name = [");
    assert!(matches!(read_dir(tmp.path()), Err(SetFmtError::Parse(_))));
    assert!(matches!(
        read_dir(&tmp.path().join("nope")),
        Err(SetFmtError::Io { .. })
    ));
}

#[test]
fn bad_aspect_ratio_is_a_warning() {
    let tmp = tempfile::tempdir().unwrap();
    put(tmp.path(), MANIFEST, b"name = \"x\"\naspect_ratio = \"wide\"\n");
    let b = read_dir(tmp.path()).unwrap();
    assert_eq!(b.manifest.aspect(), None);
    assert!(b.warnings.iter().any(|w| w.contains("aspect_ratio")));
}

#[test]
fn manifest_keys_match_decomposed_filenames() {
    let tmp = tempfile::tempdir().unwrap();
    // set.toml written on Linux (NFC), files copied from macOS (NFD).
    put(
        tmp.path(),
        MANIFEST,
        "name = \"x\"\n[items.\"café.png\"]\nname = \"coffee\"\n".as_bytes(),
    );
    put(tmp.path(), "cafe\u{301}.png", b"nfd");
    let b = read_dir(tmp.path()).unwrap();
    assert_eq!(b.files.len(), 1);
    assert_eq!(b.files[0].meta.name, "coffee");
    assert!(b.warnings.is_empty(), "{:?}", b.warnings);
}

fn sample_bundle() -> SetBundle {
    let mut manifest = Manifest::new("Sample");
    manifest.version = 4;
    manifest.description = "desc".into();
    manifest.set_aspect(Some((1, 1)));
    manifest.extra.insert("x-custom".into(), toml::Value::Integer(9));
    let mut files = vec![];
    for (i, (stem, ext)) in [("cat", "png"), ("naahh", "gif"), ("sigh", "mp4")].iter().enumerate() {
        let mut meta = ItemMeta {
            name: stem.to_string(),
            ..Default::default()
        };
        meta.keywords = vec![format!("k{i}")];
        meta.keywords_source = FieldSource::Ai;
        meta.extra.insert("note".into(), toml::Value::String(format!("n{i}")));
        let filename = stickpile_setfmt::export_filename(i + 1, 3, stem, ext);
        files.push(SetFile {
            filename,
            meta,
            bytes: format!("bytes-{i}").into_bytes(),
        });
    }
    let mut b = SetBundle {
        manifest,
        files,
        warnings: vec![],
    };
    b.manifest = b.export_manifest();
    b
}

#[test]
fn write_then_read_round_trips() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("out");
    let bundle = sample_bundle();
    write_dir(&dir, &bundle).unwrap();
    assert!(dir.join("001-cat.png").is_file());
    let back = read_dir(&dir).unwrap();
    assert_eq!(back, bundle);
    let text = fs::read_to_string(dir.join(MANIFEST)).unwrap();
    // Writing again is a no-op and gives identical text.
    write_dir(&dir, &back).unwrap();
    assert_eq!(fs::read_to_string(dir.join(MANIFEST)).unwrap(), text);
    assert_eq!(read_dir(&dir).unwrap(), bundle);
}

#[test]
fn write_removes_stale_media_only() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    put(dir, "000-old.png", b"stale");
    put(dir, "002-naahh.gif", b"old content");
    put(dir, "README.md", b"keep");
    put(dir, ".hidden.png", b"keep");
    fs::create_dir(dir.join(".git")).unwrap();
    put(&dir.join(".git"), "HEAD", b"ref: refs/heads/main");
    fs::create_dir(dir.join("extras.png")).unwrap(); // a directory with a media-looking name
    let bundle = sample_bundle();
    write_dir(dir, &bundle).unwrap();
    assert!(!dir.join("000-old.png").exists());
    assert_eq!(fs::read(dir.join("002-naahh.gif")).unwrap(), b"bytes-1");
    assert!(dir.join("README.md").exists());
    assert!(dir.join(".hidden.png").exists());
    assert!(dir.join(".git/HEAD").exists());
    assert!(dir.join("extras.png").is_dir());
    // No temp files left behind.
    let leftovers: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.contains("stickpile-tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
#[cfg(unix)]
fn write_replaces_symlinks_instead_of_following_them() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("set");
    fs::create_dir(&dir).unwrap();
    let victim = tmp.path().join("victim");
    put(tmp.path(), "victim", b"precious");
    std::os::unix::fs::symlink(&victim, dir.join("001-cat.png")).unwrap();
    std::os::unix::fs::symlink(&victim, dir.join(MANIFEST)).unwrap();
    std::os::unix::fs::symlink(&victim, dir.join("009-stale.png")).unwrap();
    write_dir(&dir, &sample_bundle()).unwrap();
    assert_eq!(fs::read(&victim).unwrap(), b"precious");
    assert!(fs::symlink_metadata(dir.join("001-cat.png")).unwrap().is_file());
    assert!(fs::symlink_metadata(dir.join(MANIFEST)).unwrap().is_file());
    assert!(fs::symlink_metadata(dir.join("009-stale.png")).is_err());
}

#[test]
fn write_rejects_unsafe_bundles() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("set");
    for bad in [
        "../escape.png",
        "/abs.png",
        "a/b.png",
        "set.toml",
        "notes.txt",
        ".hidden.png",
        "",
        "x\n.png",
    ] {
        let mut b = sample_bundle();
        b.files[0].filename = bad.to_string();
        let err = write_dir(&dir, &b).unwrap_err();
        assert!(matches!(err, SetFmtError::BadFilename(_)), "{bad:?}: {err}");
    }
    let mut dup = sample_bundle();
    dup.files[1].filename = dup.files[0].filename.clone();
    assert!(write_dir(&dir, &dup).is_err());
    assert!(!tmp.path().join("escape.png").exists());
    assert!(!dir.exists(), "nothing is written for an invalid bundle");
}

#[test]
fn files_meta_is_authoritative_over_manifest_items() {
    let tmp = tempfile::tempdir().unwrap();
    let mut b = sample_bundle();
    b.manifest.items.clear();
    b.manifest.items.insert("999-ghost.png".into(), ItemMeta::default());
    write_dir(tmp.path(), &b).unwrap();
    let back = read_dir(tmp.path()).unwrap();
    assert_eq!(back.files.len(), 3);
    assert!(!back.manifest.items.contains_key("999-ghost.png"));
    assert!(back.warnings.is_empty(), "{:?}", back.warnings);
}
