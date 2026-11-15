//! The interface the built library exports is exactly the contract of
//! docs/android.md ("Bridge interface"): every function and method with its
//! arguments, return type, `async` and `throws`, every record, enum and
//! error variant. Added, removed or changed items fail here until the
//! contract and the list below agree again. One known difference: the
//! contract's `Library` is `StickpileLibrary`, because Kotlin's `Library`
//! is `com.sun.jna.Library`, which the generated bindings import.
//!
//! The interface is read the way uniffi-bindgen reads it to generate the
//! Kotlin bindings: from the `UNIFFI_META_*` symbols of the
//! `libstickpile_android` cdylib that cargo builds next to this test. ELF
//! only (Linux, Android). `tests/kotlin.rs` (feature `bindgen`) checks the
//! generated Kotlin itself.

#![cfg(target_os = "linux")]

use std::collections::BTreeSet;
use std::path::PathBuf;

use uniffi_meta::{EnumShape, Metadata, Type};

/// The contract, rendered as [`render`] does. Types: `String`, `Boolean`,
/// `u16`, `u32`, `u64`, `i64`, `List<T>`, `T?`, and the records and enums
/// by name.
const CONTRACT: &[&str] = &[
    "fn version() -> String",
    "fn client_name() -> String",
    "object StickpileLibrary",
    "constructor StickpileLibrary.new(dirs: LibraryDirs) throws LibraryError",
    // account
    "fn StickpileLibrary.server_url() -> String",
    "async fn StickpileLibrary.set_server_url(url: String) throws LibraryError",
    "async fn StickpileLibrary.account() -> Account? throws LibraryError",
    "async fn StickpileLibrary.login_tailnet() -> Account? throws LibraryError",
    "async fn StickpileLibrary.login_password(username: String, password: String, totp: String?) -> Account throws LibraryError",
    "async fn StickpileLibrary.login_pairing(server_url: String, code: String) -> Account throws LibraryError",
    "async fn StickpileLibrary.logout() throws LibraryError",
    // reading
    "async fn StickpileLibrary.search(query: String, target: Target, limit: u32) -> ItemList throws LibraryError",
    "fn StickpileLibrary.recent(limit: u32) -> List<Item>",
    "async fn StickpileLibrary.favorites(limit: u32) -> ItemList throws LibraryError",
    "async fn StickpileLibrary.most_used(limit: u32) -> ItemList throws LibraryError",
    "async fn StickpileLibrary.packs() -> PackList throws LibraryError",
    "async fn StickpileLibrary.pack_items(pack_id: String) -> ItemList throws LibraryError",
    "fn StickpileLibrary.suggestions(query: String) -> Suggestions?",
    // files
    "async fn StickpileLibrary.thumbnail(item_id: String) -> String throws LibraryError",
    "async fn StickpileLibrary.prepare(item_id: String, editor_package: String, accepted_mime_types: List<String>, sticker: Boolean) -> Prepared throws LibraryError",
    // writing
    "fn StickpileLibrary.record_use(item_id: String, variant: String, editor_package: String) throws LibraryError",
    "async fn StickpileLibrary.toggle_like(item_id: String) -> Boolean throws LibraryError",
    "async fn StickpileLibrary.upload(set_id: String, file: UploadFile) -> Item throws LibraryError",
    // pairing input
    "fn StickpileLibrary.parse_pairing_link(text: String) -> PairingLink?",
    "fn StickpileLibrary.normalize_pairing_code(input: String) -> String?",
    // sync
    "async fn StickpileLibrary.sync_if_due() -> SyncState throws LibraryError",
    "async fn StickpileLibrary.sync_now() -> SyncState throws LibraryError",
    "fn StickpileLibrary.sync_state() -> SyncState",
    "fn StickpileLibrary.pending_usage() -> u32",
    "fn StickpileLibrary.cache_bytes() -> u64 throws LibraryError",
    "fn StickpileLibrary.clear_cache() -> u64 throws LibraryError",
    // records
    "record LibraryDirs { config_file: String, cache_dir: String, token_file: String }",
    "record Account { username: String, display_name: String?, server_url: String, auth: AuthKind, is_admin: Boolean }",
    "record Item { id: String, name: String, set_id: String, set_name: String, emojis: List<String>, \
     keywords: List<String>, vibe: String?, tags: List<String>, kind: MediaKind, width: u32, height: u32, \
     likes: u64, copies: u64 }",
    "record ItemList { items: List<Item>, offline: Boolean }",
    "record Pack { id: String, slug: String, name: String, owner: String, item_count: u32, mine: Boolean }",
    "record PackList { packs: List<Pack>, offline: Boolean }",
    "record Suggestions { key: String, words: List<String>, chosen: u32? }",
    "record PairingLink { server_url: String, code: String }",
    "record Prepared { path: String, mime_type: String, display_name: String, target: Target, variant: String, \
     sticker: Boolean }",
    "record UploadFile { path: String, name: String?, keywords: List<String>, emojis: List<String> }",
    "record SyncState { last_sync: i64?, last_error: LibraryErrorKind?, pending_usage: u32, due: Boolean }",
    // enums
    "enum AuthKind { Token, Tailnet }",
    "enum Target { Telegram, Discord, Signal, Whatsapp, Slack, Generic }",
    "enum MediaKind { Still, Animation, Video }",
    "enum LibraryErrorKind { NotSignedIn, Unauthorized, Unreachable, NotFound, RateLimited, NothingFits, \
     InvalidInput, Server, Io, Other }",
    // the error
    "error LibraryError { NotSignedIn { detail: String }, Unauthorized { detail: String }, \
     Unreachable { detail: String }, NotFound { detail: String }, RateLimited { detail: String }, \
     NothingFits { detail: String }, InvalidInput { field: String, detail: String }, \
     Server { status: u16, detail: String }, Io { detail: String }, Other { detail: String } }",
];

fn cdylib() -> PathBuf {
    let name = format!(
        "{}stickpile_android{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    );
    // This test is target/<profile>/deps/interface-<hash>; cargo puts the
    // library into deps/ and target/<profile>/.
    let exe = std::env::current_exe().unwrap();
    let deps = exe.parent().unwrap();
    [deps, deps.parent().unwrap()]
        .iter()
        .map(|d| d.join(&name))
        .find(|p| p.exists())
        .unwrap_or_else(|| panic!("{name} not found next to {}", exe.display()))
}

/// The UniFFI metadata of the library (as uniffi_bindgen's
/// `macro_metadata::extract_from_elf` reads it).
fn metadata() -> Vec<Metadata> {
    let path = cdylib();
    let bytes = std::fs::read(&path).unwrap();
    let elf = goblin::elf::Elf::parse(&bytes).unwrap();
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for (syms, strtab) in [(&elf.syms, &elf.strtab), (&elf.dynsyms, &elf.dynstrtab)] {
        for sym in syms.iter() {
            let Some(name) = strtab.get_at(sym.st_name) else {
                continue;
            };
            if !name.starts_with("UNIFFI_META") || !seen.insert(name.to_string()) {
                continue;
            }
            let sh = &elf.section_headers[sym.st_shndx];
            let start = (sh.sh_offset + (sym.st_value - sh.sh_addr)) as usize;
            let data = &bytes[start..start + sym.st_size as usize];
            out.push(uniffi_meta::read_metadata(data).unwrap_or_else(|e| panic!("{name}: {e}")));
        }
    }
    assert!(!out.is_empty(), "no UniFFI metadata in {}", path.display());
    out
}

fn ty(t: &Type) -> String {
    match t {
        Type::UInt8 => "u8".into(),
        Type::Int8 => "i8".into(),
        Type::UInt16 => "u16".into(),
        Type::Int16 => "i16".into(),
        Type::UInt32 => "u32".into(),
        Type::Int32 => "i32".into(),
        Type::UInt64 => "u64".into(),
        Type::Int64 => "i64".into(),
        Type::Float32 => "f32".into(),
        Type::Float64 => "f64".into(),
        Type::Boolean => "Boolean".into(),
        Type::String => "String".into(),
        Type::Object { name, .. }
        | Type::Record { name, .. }
        | Type::Enum { name, .. }
        | Type::CallbackInterface { name, .. }
        | Type::Custom { name, .. } => name.clone(),
        Type::Box { inner_type } => ty(inner_type),
        Type::Optional { inner_type } => format!("{}?", ty(inner_type)),
        Type::Sequence { inner_type } => format!("List<{}>", ty(inner_type)),
        other => format!("{other:?}"),
    }
}

fn params(inputs: &[uniffi_meta::FnParamMetadata]) -> String {
    inputs
        .iter()
        .map(|p| format!("{}: {}", p.name, ty(&p.ty)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn fields(fields: &[uniffi_meta::FieldMetadata]) -> String {
    fields
        .iter()
        .map(|f| format!("{}: {}", f.name, ty(&f.ty)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn callable(
    is_async: bool,
    name: &str,
    inputs: &[uniffi_meta::FnParamMetadata],
    ret: Option<&Type>,
    throws: Option<&Type>,
) -> String {
    let mut s = format!("{}fn {name}({})", if is_async { "async " } else { "" }, params(inputs));
    if let Some(r) = ret {
        s += &format!(" -> {}", ty(r));
    }
    if let Some(t) = throws {
        s += &format!(" throws {}", ty(t));
    }
    s
}

/// One line per exported item; `None` for what the contract does not
/// describe (the namespace).
fn render(m: &Metadata) -> Option<String> {
    Some(match m {
        Metadata::Namespace(_) => return None,
        Metadata::Func(f) => callable(
            f.is_async,
            &f.name,
            &f.inputs,
            f.return_type.as_ref(),
            f.throws.as_ref(),
        ),
        Metadata::Object(o) => format!("object {}", o.name),
        Metadata::Constructor(c) => {
            let mut s = format!(
                "{}constructor {}.{}({})",
                if c.is_async { "async " } else { "" },
                c.self_name,
                c.name,
                params(&c.inputs)
            );
            if let Some(t) = &c.throws {
                s += &format!(" throws {}", ty(t));
            }
            s
        }
        Metadata::Method(m) => callable(
            m.is_async,
            &format!("{}.{}", m.self_name, m.name),
            &m.inputs,
            m.return_type.as_ref(),
            m.throws.as_ref(),
        ),
        Metadata::Record(r) => format!("record {} {{ {} }}", r.name, fields(&r.fields)),
        Metadata::Enum(e) => {
            let variants: Vec<String> = e
                .variants
                .iter()
                .map(|v| {
                    if v.fields.is_empty() {
                        v.name.clone()
                    } else {
                        format!("{} {{ {} }}", v.name, fields(&v.fields))
                    }
                })
                .collect();
            let kind = match e.shape {
                EnumShape::Enum => "enum",
                EnumShape::Error { .. } => "error",
            };
            format!("{kind} {} {{ {} }}", e.name, variants.join(", "))
        }
        other => format!("{other:?}"),
    })
}

#[test]
fn the_library_exports_the_contract() {
    let exported: BTreeSet<String> = metadata().iter().filter_map(render).collect();
    let contract: BTreeSet<String> = CONTRACT
        .iter()
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    let missing: Vec<&String> = contract.difference(&exported).collect();
    let unexpected: Vec<&String> = exported.difference(&contract).collect();
    assert!(
        missing.is_empty() && unexpected.is_empty(),
        "the exported interface differs from the contract\n\nin the contract only:\n{}\n\nexported only:\n{}",
        missing.iter().map(|s| format!("  {s}")).collect::<Vec<_>>().join("\n"),
        unexpected
            .iter()
            .map(|s| format!("  {s}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}
