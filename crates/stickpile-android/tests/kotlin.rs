//! The Kotlin bindings generated from the built library carry exactly the
//! interface of docs/android.md ("Bridge interface"): every function with
//! its Kotlin signature, every record, enum and error variant. A change to
//! the exported interface fails here until the contract and this list
//! agree again. One known difference: the contract's `Library` is
//! `StickpileLibrary` (Kotlin's `Library` is `com.sun.jna.Library`).
//!
//! The bindings are made like the Gradle build makes them (library mode,
//! the crate's `uniffi.toml`), from the `libstickpile_android` cdylib that
//! cargo builds next to this test.
//!
//! Needs the `bindgen` feature, so it is not part of the default test run:
//! `cargo nextest run -p stickpile-android --features bindgen`. The reason
//! is the Nix sandbox: `uniffi_bindgen` does not compile there (askama 0.16
//! computes its `askama.toml` path lexically, which breaks on crane's
//! symlinked vendor directory). `tests/interface.rs` checks the same
//! interface, from UniFFI's metadata, in every run.

#![cfg(feature = "bindgen")]

use std::path::{Path, PathBuf};

/// Every method of `StickpileLibraryInterface`, as the bindings declare it.
const METHODS: &[&str] = &[
    // account
    "fun `serverUrl`(): kotlin.String",
    "suspend fun `setServerUrl`(`url`: kotlin.String)",
    "suspend fun `account`(): Account?",
    "suspend fun `loginTailnet`(): Account?",
    "suspend fun `loginPassword`(`username`: kotlin.String, `password`: kotlin.String, `totp`: kotlin.String?): Account",
    "suspend fun `loginPairing`(`serverUrl`: kotlin.String, `code`: kotlin.String): Account",
    "suspend fun `logout`()",
    // reading
    "suspend fun `search`(`query`: kotlin.String, `target`: Target, `limit`: kotlin.UInt): ItemList",
    "fun `recent`(`limit`: kotlin.UInt): List<Item>",
    "suspend fun `favorites`(`limit`: kotlin.UInt): ItemList",
    "suspend fun `mostUsed`(`limit`: kotlin.UInt): ItemList",
    "suspend fun `packs`(): PackList",
    "suspend fun `packItems`(`packId`: kotlin.String): ItemList",
    "fun `suggestions`(`query`: kotlin.String): Suggestions?",
    // files
    "suspend fun `thumbnail`(`itemId`: kotlin.String): kotlin.String",
    "suspend fun `prepare`(`itemId`: kotlin.String, `editorPackage`: kotlin.String, `acceptedMimeTypes`: List<kotlin.String>, `sticker`: kotlin.Boolean): Prepared",
    // writing
    "fun `recordUse`(`itemId`: kotlin.String, `variant`: kotlin.String, `editorPackage`: kotlin.String)",
    "suspend fun `toggleLike`(`itemId`: kotlin.String): kotlin.Boolean",
    "suspend fun `upload`(`setId`: kotlin.String, `file`: UploadFile): Item",
    // pairing input
    "fun `parsePairingLink`(`text`: kotlin.String): PairingLink?",
    "fun `normalizePairingCode`(`input`: kotlin.String): kotlin.String?",
    // sync
    "suspend fun `syncIfDue`(): SyncState",
    "suspend fun `syncNow`(): SyncState",
    "fun `syncState`(): SyncState",
    "fun `pendingUsage`(): kotlin.UInt",
    "suspend fun `ownPacks`(): PackList",
    // cache
    "fun `cacheBytes`(): kotlin.ULong",
    "fun `clearCache`(): kotlin.ULong",
];

/// The methods that do not throw (`throws` is missing in the contract).
const NOT_THROWING: &[&str] = &[
    "serverUrl",
    "recent",
    "suggestions",
    "parsePairingLink",
    "normalizePairingCode",
    "syncState",
    "pendingUsage",
];

/// Records with their fields in order, as `name: type`.
const RECORDS: &[(&str, &[&str])] = &[
    (
        "LibraryDirs",
        &[
            "configFile: kotlin.String",
            "cacheDir: kotlin.String",
            "tokenFile: kotlin.String",
        ],
    ),
    (
        "Account",
        &[
            "username: kotlin.String",
            "displayName: kotlin.String?",
            "serverUrl: kotlin.String",
            "auth: AuthKind",
            "isAdmin: kotlin.Boolean",
        ],
    ),
    (
        "Item",
        &[
            "id: kotlin.String",
            "name: kotlin.String",
            "setId: kotlin.String",
            "setName: kotlin.String",
            "emojis: List<kotlin.String>",
            "keywords: List<kotlin.String>",
            "vibe: kotlin.String?",
            "tags: List<kotlin.String>",
            "kind: MediaKind",
            "width: kotlin.UInt",
            "height: kotlin.UInt",
            "likes: kotlin.ULong",
            "copies: kotlin.ULong",
        ],
    ),
    ("ItemList", &["items: List<Item>", "offline: kotlin.Boolean"]),
    (
        "Pack",
        &[
            "id: kotlin.String",
            "slug: kotlin.String",
            "name: kotlin.String",
            "owner: kotlin.String",
            "itemCount: kotlin.UInt",
            "mine: kotlin.Boolean",
        ],
    ),
    ("PackList", &["packs: List<Pack>", "offline: kotlin.Boolean"]),
    (
        "Suggestions",
        &[
            "key: kotlin.String",
            "words: List<kotlin.String>",
            "chosen: kotlin.UInt?",
        ],
    ),
    ("PairingLink", &["serverUrl: kotlin.String", "code: kotlin.String"]),
    (
        "Prepared",
        &[
            "path: kotlin.String",
            "mimeType: kotlin.String",
            "displayName: kotlin.String",
            "target: Target",
            "variant: kotlin.String",
            "sticker: kotlin.Boolean",
        ],
    ),
    (
        "UploadFile",
        &[
            "path: kotlin.String",
            "name: kotlin.String?",
            "keywords: List<kotlin.String>",
            "emojis: List<kotlin.String>",
        ],
    ),
    (
        "SyncState",
        &[
            "lastSync: kotlin.Long?",
            "lastError: LibraryErrorKind?",
            "pendingUsage: kotlin.UInt",
            "due: kotlin.Boolean",
        ],
    ),
];

const ENUMS: &[(&str, &[&str])] = &[
    ("AuthKind", &["TOKEN", "TAILNET"]),
    (
        "Target",
        &["TELEGRAM", "DISCORD", "SIGNAL", "WHATSAPP", "SLACK", "GENERIC"],
    ),
    ("MediaKind", &["STILL", "ANIMATION", "VIDEO"]),
    (
        "LibraryErrorKind",
        &[
            "NOT_SIGNED_IN",
            "UNAUTHORIZED",
            "UNREACHABLE",
            "NOT_FOUND",
            "RATE_LIMITED",
            "NOTHING_FITS",
            "INVALID_INPUT",
            "SERVER",
            "IO",
            "OTHER",
        ],
    ),
];

/// `LibraryError` (Kotlin: `LibraryException`): its variants and their fields.
const ERRORS: &[(&str, &[&str])] = &[
    ("NotSignedIn", &["detail: kotlin.String"]),
    ("Unauthorized", &["detail: kotlin.String"]),
    ("Unreachable", &["detail: kotlin.String"]),
    ("NotFound", &["detail: kotlin.String"]),
    ("RateLimited", &["detail: kotlin.String"]),
    ("NothingFits", &["detail: kotlin.String"]),
    ("InvalidInput", &["field: kotlin.String", "detail: kotlin.String"]),
    ("Server", &["status: kotlin.UShort", "detail: kotlin.String"]),
    ("Io", &["detail: kotlin.String"]),
    ("Other", &["detail: kotlin.String"]),
];

/// The top-level functions besides `StickpileLibrary`.
const FUNCTIONS: &[&str] = &[
    "fun `targetForPackage`(`package`: kotlin.String): Target",
    "fun `version`(): kotlin.String",
    "fun `clientName`(): kotlin.String",
];

fn cdylib() -> PathBuf {
    let name = format!(
        "{}stickpile_android{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    );
    // This test is target/<profile>/deps/bindings-<hash>; cargo puts the
    // library into deps/ and target/<profile>/.
    let exe = std::env::current_exe().unwrap();
    let deps = exe.parent().unwrap();
    [deps, deps.parent().unwrap()]
        .iter()
        .map(|d| d.join(&name))
        .find(|p| p.exists())
        .unwrap_or_else(|| panic!("{name} not found next to {}", exe.display()))
}

fn generate(out: &Path) -> String {
    uniffi::generate(uniffi::GenerateOptions {
        languages: vec![uniffi::TargetLanguage::Kotlin],
        source: cdylib().display().to_string().into(),
        out_dir: out.display().to_string().into(),
        config_override: None,
        format: false,
        crate_filter: Some("stickpile_android".into()),
        // Only to find this crate's uniffi.toml; nothing to fetch.
        metadata_no_deps: true,
    })
    .unwrap();
    let file = out.join("io/github/dolfost/stickpile/rust/stickpile_android.kt");
    std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()))
}

/// `text` without comments, with whitespace runs made one space and no
/// space next to brackets, commas and semicolons.
fn squash(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        rest = rest[start..].find("*/").map_or("", |end| &rest[start + end + 2..]);
    }
    out.push_str(rest);
    let out: String = out
        .lines()
        .map(|l| l.split("//").next().unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" ");
    let mut squashed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    for p in ["(", ")", "{", "}", ",", ";", "<", ">"] {
        squashed = squashed.replace(&format!(" {p}"), p).replace(&format!("{p} "), p);
    }
    squashed
}

#[test]
fn the_kotlin_bindings_carry_the_contract() {
    let out = tempfile::tempdir().unwrap();
    let kt = generate(out.path());
    assert!(kt.contains("package io.github.dolfost.stickpile.rust"));

    // The Rust docs become KDoc unescaped: a `/*` in them opens a nested
    // comment in Kotlin, a `*/` ends the KDoc early.
    for (n, line) in kt.lines().enumerate() {
        let body = line.trim_start();
        // UniFFI's own KDoc may close with `* */`.
        if (body.starts_with("* ") || body == "*") && body != "* */" {
            assert!(
                !body.contains("/*") && !body.contains("*/"),
                "line {}: a comment delimiter in a doc comment: {line}",
                n + 1
            );
        }
    }

    // No exported type may take the name of one the file imports
    // (`com.sun.jna.Library` is why the object is `StickpileLibrary`).
    let imported: Vec<&str> = kt
        .lines()
        .filter_map(|l| l.strip_prefix("import "))
        .filter_map(|l| l.trim().rsplit('.').next())
        .collect();
    let exported = RECORDS.iter().chain(ENUMS).map(|(name, _)| *name).chain([
        "StickpileLibrary",
        "StickpileLibraryInterface",
        "LibraryException",
    ]);
    for name in exported {
        assert!(!imported.contains(&name), "{name} clashes with an import");
    }

    // Exactly the contract's methods.
    let interface = kt
        .split("public interface StickpileLibraryInterface {")
        .nth(1)
        .and_then(|s| s.split("companion object").next())
        .expect("StickpileLibraryInterface");
    let mut declared: Vec<&str> = interface
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("fun ") || l.starts_with("suspend fun "))
        .collect();
    let mut want = METHODS.to_vec();
    declared.sort_unstable();
    want.sort_unstable();
    assert_eq!(declared, want, "StickpileLibraryInterface differs from the contract");

    // Which of them throw LibraryException.
    let squashed = squash(&kt);
    for method in METHODS {
        let name = method.split('`').nth(1).unwrap();
        let decl = if method.starts_with("suspend") {
            format!("override suspend fun `{name}`(")
        } else {
            format!("override fun `{name}`(")
        };
        let at = squashed.find(&squash(&decl)).unwrap_or_else(|| panic!("no {decl}"));
        // The annotations right before the declaration.
        let mut before = squashed[..at].trim_end();
        let mut throws = false;
        loop {
            if let Some(b) = before.strip_suffix("@Throws(LibraryException::class)") {
                throws = true;
                before = b.trim_end();
            } else if let Some(b) = before.strip_suffix("@Suppress(\"ASSIGNED_BUT_NEVER_ACCESSED_VARIABLE\")") {
                before = b.trim_end();
            } else {
                break;
            }
        }
        assert_eq!(throws, !NOT_THROWING.contains(&name), "{name}: throws");
    }

    // The primary constructor (not async: UniFFI cannot make it so for Kotlin).
    assert!(squashed.contains(&squash(
        "open class StickpileLibrary: Disposable, AutoCloseable, StickpileLibraryInterface {"
    )));
    assert!(
        squashed.contains(&squash(
            "constructor(`dirs`: LibraryDirs) : this(UniffiWithHandle, uniffiRustCallWithError(LibraryException)"
        )),
        "constructor(dirs: LibraryDirs) throws LibraryException"
    );

    for (name, fields) in RECORDS {
        let fields: Vec<String> = fields
            .iter()
            .map(|f| {
                let (n, t) = f.split_once(": ").unwrap();
                format!("var `{n}`: {t}")
            })
            .collect();
        let want = squash(&format!("data class {name} ({})", fields.join(", ")));
        assert!(squashed.contains(&want), "record {name}: {want}");
    }
    for (name, variants) in ENUMS {
        let want = squash(&format!("enum class {name} {{ {}; ", variants.join(", ")));
        assert!(squashed.contains(&want), "enum {name}: {want}");
    }
    assert!(squashed.contains(&squash("sealed class LibraryException: kotlin.Exception() {")));
    let variants = squashed.matches(&squash(": LibraryException() {")).count();
    assert_eq!(variants, ERRORS.len(), "LibraryException's variants");
    for (name, fields) in ERRORS {
        let fields: Vec<String> = fields
            .iter()
            .map(|f| {
                let (n, t) = f.split_once(": ").unwrap();
                format!("val `{n}`: {t}")
            })
            .collect();
        let want = squash(&format!("class {name}({}) : LibraryException() {{", fields.join(", ")));
        assert!(squashed.contains(&want), "error {name}: {want}");
    }
    for f in FUNCTIONS {
        assert!(squashed.contains(&squash(f)), "{f}");
    }
}
