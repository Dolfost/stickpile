//! Tar and folder-upload import, tar export, and traversal attempts.

use stickpile_setfmt::{
    export_filename, from_files, read_tar, write_tar, FieldSource, ItemMeta, Manifest, SetBundle, SetFile, SetFmtError,
    MANIFEST, TAR_MTIME,
};

const LIMIT: u64 = 1 << 20;

fn bundle() -> SetBundle {
    let mut manifest = Manifest::new("Tarred");
    manifest.version = 2;
    manifest.extra.insert("x".into(), toml::Value::Boolean(true));
    let long_stem = "a-very-long-name-".repeat(6); // slugified down to 64 bytes
    let files: Vec<SetFile> = [("cat", "png"), (long_stem.as_str(), "gif"), ("sigh", "webm")]
        .iter()
        .enumerate()
        .map(|(i, (stem, ext))| SetFile {
            filename: export_filename(i + 1, 3, stem, ext),
            meta: ItemMeta {
                name: stem.to_string(),
                emojis: vec!["😼".into()],
                emojis_source: FieldSource::Ai,
                ..Default::default()
            },
            bytes: vec![i as u8; 1000 + i],
        })
        .collect();
    let mut b = SetBundle {
        manifest,
        files,
        warnings: vec![],
    };
    b.manifest = b.export_manifest();
    b
}

#[test]
fn tar_round_trip_and_determinism() {
    let b = bundle();
    // A long top-level directory pushes paths past 100 bytes (GNU long names).
    let top = format!("tarred-{}", "x".repeat(60));
    let tar1 = write_tar(&b, &top).unwrap();
    let tar2 = write_tar(&b, &top).unwrap();
    assert_eq!(tar1, tar2, "write_tar must be deterministic");
    assert_eq!(read_tar(&tar1, LIMIT).unwrap(), b);

    let mut archive = tar::Archive::new(&tar1[..]);
    let mut paths = vec![];
    for e in archive.entries().unwrap() {
        let e = e.unwrap();
        let h = e.header();
        assert_eq!(h.mtime().unwrap(), TAR_MTIME);
        assert_eq!(h.uid().unwrap(), 0);
        assert_eq!(h.gid().unwrap(), 0);
        let mode = h.mode().unwrap();
        assert!(mode == 0o644 || mode == 0o755, "{mode:o}");
        paths.push(e.path().unwrap().to_string_lossy().into_owned());
    }
    assert_eq!(paths[0].trim_end_matches('/'), top);
    assert_eq!(paths[1], format!("{top}/set.toml"));
    assert_eq!(paths.len(), 5);
    assert!(paths[3].len() > 100);
}

#[test]
fn write_tar_rejects_bad_top_dir() {
    for bad in ["", "..", "a/b", ".hidden"] {
        assert!(write_tar(&bundle(), bad).is_err(), "{bad:?}");
    }
}

// ---------------------------------------------------------------------------
// Hand-made archives
// ---------------------------------------------------------------------------

struct Raw(tar::Builder<Vec<u8>>);

impl Raw {
    fn new() -> Raw {
        Raw(tar::Builder::new(Vec::new()))
    }
    /// Writes the path bytes verbatim (bypassing the tar crate's own checks).
    fn entry(mut self, path: &str, kind: tar::EntryType, data: &[u8], link: &str) -> Raw {
        let mut h = tar::Header::new_gnu();
        {
            let old = h.as_old_mut();
            old.name[..path.len()].copy_from_slice(path.as_bytes());
            old.linkname[..link.len()].copy_from_slice(link.as_bytes());
        }
        h.set_entry_type(kind);
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        self.0.append(&h, data).unwrap();
        self
    }
    fn file(self, path: &str, data: &[u8]) -> Raw {
        self.entry(path, tar::EntryType::Regular, data, "")
    }
    fn dir(self, path: &str) -> Raw {
        self.entry(path, tar::EntryType::Directory, b"", "")
    }
    fn done(self) -> Vec<u8> {
        self.0.into_inner().unwrap()
    }
}

#[test]
fn tar_without_top_dir_and_with_dot_prefix() {
    let tar = Raw::new()
        .file("set.toml", b"name = \"Flat\"")
        .file("b.gif", b"b")
        .file("a.png", b"a")
        .done();
    let b = read_tar(&tar, LIMIT).unwrap();
    assert_eq!(b.manifest.name, "Flat");
    assert_eq!(
        b.files.iter().map(|f| f.filename.as_str()).collect::<Vec<_>>(),
        ["a.png", "b.gif"]
    );

    let tar = Raw::new()
        .dir("./")
        .dir("./pack/")
        .file("./pack/001-x.png", b"x")
        .done();
    let b = read_tar(&tar, LIMIT).unwrap();
    assert_eq!(b.manifest.name, "pack", "name falls back to the top-level directory");
    assert_eq!(b.files[0].meta.name, "x");
}

#[test]
fn tar_skips_hidden_entries_pax_headers_and_non_media() {
    let tar = Raw::new()
        .entry(
            "pax_global_header",
            tar::EntryType::XGlobalHeader,
            b"52 comment=0123456789abcdef\n",
            "",
        )
        .dir("pack/")
        .dir("pack/.git/")
        .file("pack/.git/config", b"[core]")
        .file("pack/.DS_Store", b"junk")
        .file("pack/README.md", b"readme")
        .file("pack/001-x.png", b"x")
        .done();
    let b = read_tar(&tar, LIMIT).unwrap();
    assert_eq!(b.files.len(), 1);
    assert!(b.warnings.iter().any(|w| w.contains("README.md")));
}

#[test]
fn tar_traversal_and_special_entries_are_rejected() {
    use tar::EntryType as T;
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("absolute", Raw::new().file("/etc/x.png", b"x").done()),
        ("dotdot", Raw::new().file("../x.png", b"x").done()),
        ("dotdot inside", Raw::new().file("pack/../../x.png", b"x").done()),
        ("dotdot dir", Raw::new().dir("pack/../").done()),
        ("backslash", Raw::new().file("pack\\..\\x.png", b"x").done()),
        (
            "symlink",
            Raw::new().entry("pack/x.png", T::Symlink, b"", "/etc/passwd").done(),
        ),
        (
            "hardlink",
            Raw::new().entry("pack/x.png", T::Link, b"", "/etc/passwd").done(),
        ),
        ("chardev", Raw::new().entry("pack/x.png", T::Char, b"", "").done()),
        ("blockdev", Raw::new().entry("pack/x.png", T::Block, b"", "").done()),
        ("fifo", Raw::new().entry("pack/x.png", T::Fifo, b"", "").done()),
        ("nested file", Raw::new().file("pack/sub/x.png", b"x").done()),
        ("nested dir", Raw::new().dir("pack/sub/").done()),
        (
            "two top dirs",
            Raw::new().file("a/x.png", b"x").file("b/y.png", b"y").done(),
        ),
        (
            "root then dir",
            Raw::new().file("x.png", b"x").file("a/y.png", b"y").done(),
        ),
        (
            "dir then root",
            Raw::new().file("a/y.png", b"y").file("x.png", b"x").done(),
        ),
        (
            "duplicate",
            Raw::new().file("a/x.png", b"1").file("a/x.png", b"2").done(),
        ),
        (
            "duplicate nfc",
            Raw::new()
                .file("a/café.png", b"1")
                .file("a/cafe\u{301}.png", b"2")
                .done(),
        ),
        (
            "two manifests",
            Raw::new().file("a/set.toml", b"").file("a/./set.toml", b"").done(),
        ),
        ("control char", Raw::new().file("a/x\x01.png", b"x").done()),
    ];
    for (what, tar) in cases {
        match read_tar(&tar, LIMIT) {
            Err(SetFmtError::Archive(_) | SetFmtError::BadFilename(_)) => {}
            other => panic!("{what}: expected rejection, got {other:?}"),
        }
    }
}

#[test]
fn tar_size_limit_counts_actual_bytes() {
    let tar = Raw::new().file("a/1.png", &[1; 600]).file("a/2.png", &[2; 600]).done();
    assert!(read_tar(&tar, 1200).is_ok());
    assert!(matches!(read_tar(&tar, 1199), Err(SetFmtError::Archive(_))));
    assert!(matches!(read_tar(&tar, 500), Err(SetFmtError::Archive(_))));
    // The manifest counts too.
    let tar = Raw::new()
        .file("a/set.toml", &[b' '; 300])
        .file("a/1.png", &[1; 600])
        .done();
    assert!(read_tar(&tar, 899).is_err());
    assert!(read_tar(&tar, 900).is_ok());
}

#[test]
fn tar_truncated_or_garbage_is_an_error() {
    let tar = write_tar(&bundle(), "t").unwrap();
    assert!(read_tar(&tar[..700], LIMIT).is_err());
    assert!(read_tar(b"definitely not a tar archive, but long enough to look at", LIMIT).is_err());
}

#[test]
fn tar_with_invalid_manifest_is_an_error() {
    let tar = Raw::new().file("a/set.toml", b"name = [").done();
    assert!(matches!(read_tar(&tar, LIMIT), Err(SetFmtError::Parse(_))));
    let tar = Raw::new().file("a/set.toml", &[0xff, 0xfe]).done();
    assert!(matches!(read_tar(&tar, LIMIT), Err(SetFmtError::Parse(_))));
}

// ---------------------------------------------------------------------------
// Folder uploads
// ---------------------------------------------------------------------------

fn f(path: &str, bytes: &[u8]) -> (String, Vec<u8>) {
    (path.to_string(), bytes.to_vec())
}

#[test]
fn folder_upload_with_top_dir() {
    let b = from_files(
        None,
        vec![
            f("cats/002-b.gif", b"b"),
            f(
                "cats/set.toml",
                b"name = \"Cats\"\n[items.\"001-a.png\"]\nkeywords = [\"a\"]\n",
            ),
            f("cats/001-a.png", b"a"),
            f("cats/.DS_Store", b"junk"),
            f("cats/Thumbs.db", b"junk"),
        ],
    )
    .unwrap();
    assert_eq!(b.manifest.name, "Cats");
    assert_eq!(
        b.files.iter().map(|f| f.filename.as_str()).collect::<Vec<_>>(),
        ["001-a.png", "002-b.gif"]
    );
    assert_eq!(b.files[0].meta.keywords, ["a"]);
    assert!(b.warnings.iter().any(|w| w.contains("Thumbs.db")));
}

#[test]
fn folder_upload_without_manifest_and_flat() {
    let b = from_files(None, vec![f("dogs/a.png", b"a")]).unwrap();
    assert_eq!(b.manifest.name, "dogs");
    let b = from_files(None, vec![f("a.png", b"a"), f("b.png", b"b")]).unwrap();
    assert_eq!(b.files.len(), 2);
    assert!(!b.manifest.name.is_empty());
}

#[test]
fn folder_upload_explicit_manifest_wins() {
    let b = from_files(
        Some("name = \"Explicit\""),
        vec![f("x/set.toml", b"name = \"Inner\""), f("x/a.png", b"a")],
    )
    .unwrap();
    assert_eq!(b.manifest.name, "Explicit");
    assert!(b.warnings.iter().any(|w| w.contains(MANIFEST)));
}

#[test]
fn folder_upload_traversal_is_rejected() {
    for files in [
        vec![f("../x.png", b"x")],
        vec![f("/abs/x.png", b"x")],
        vec![f("a/../../x.png", b"x")],
        vec![f("a\\..\\x.png", b"x")],
        vec![f("a/b/c.png", b"x")],
        vec![f("a/x.png", b"x"), f("b/y.png", b"y")],
        vec![f("x.png", b"x"), f("a/y.png", b"y")],
        vec![f("a/x.png", b"x"), f("a/x.png", b"y")],
        vec![f("a/x\0.png", b"x")],
    ] {
        assert!(
            from_files(None, files.clone()).is_err(),
            "{:?}",
            files.iter().map(|x| &x.0).collect::<Vec<_>>()
        );
    }
}
