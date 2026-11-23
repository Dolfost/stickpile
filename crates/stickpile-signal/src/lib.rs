//! Signal sticker packs for stickpile (docs/signal.md).
//!
//! * Import: [`import_pack`] reads a pack from its link
//!   (`https://signal.art/addstickers/#pack_id=...&pack_key=...` or
//!   `sgnl://addstickers/?pack_id=...&pack_key=...`, see [`PackLink`]):
//!   the encrypted manifest and stickers come from Signal's CDN and are
//!   decrypted with the pack key ([`crypto`], [`manifest`]).
//! * Upload: [`upload_pack`] creates a new pack with the credentials of a
//!   registered Signal account or linked device ([`Credentials`]) and
//!   returns its link. Packs are immutable: an update is a new pack.
//! * More than 200 stickers: [`split_parts`] makes several packs of them,
//!   [`upload_parts`] uploads each as a new pack ([`parts`]).
//! * Without credentials: [`signal_ready_zip`] builds a folder for Signal
//!   Desktop's sticker creator and for signal-cli
//!   ([`signal_ready_parts_files`]: one folder per part).
//! * Files: [`STICKER_REQUIREMENTS`] and [`check_sticker`] state what Signal
//!   accepts so the server can render fitting files.
//!
//! The service and CDN URLs are configurable ([`SignalConfig`]) so tests run
//! against a mock; production TLS trusts only Signal's root certificate.

pub mod client;
pub mod credentials;
pub mod crypto;
pub mod folder;
pub mod image;
pub mod link;
pub mod manifest;
pub mod pack;
pub mod parts;

pub use client::{
    SignalClient, SignalConfig, DEFAULT_CDN_URL, DEFAULT_SERVICE_URL, MAX_CIPHERTEXT_BYTES, SIGNAL_ROOT_CA_PEM,
    SIGNAL_ROOT_CA_SHA256,
};
pub use credentials::Credentials;
pub use crypto::CryptoError;
pub use folder::{signal_ready_files, signal_ready_parts_files, signal_ready_zip, zip_stored, PackFile};
pub use image::{inspect, ImageFormat, ImageInfo};
pub use link::{PackKey, PackLink};
pub use manifest::{Manifest, ManifestSticker};
pub use pack::{
    check_sticker, import_pack, is_emoji, sticker_emoji, upload_pack, ImportedPack, ImportedSticker, Requirements,
    UploadPack, UploadSticker, UploadedPack, DEFAULT_EMOJI, MAX_ANIMATION_MS, MAX_STICKERS, MAX_STICKER_BYTES,
    STICKER_REQUIREMENTS, STICKER_SIDE_PX,
};
pub use parts::{part_title, split_parts, upload_parts, PartsUpload, UploadedPart};

#[derive(Debug, thiserror::Error)]
pub enum SignalError {
    /// The link is not a sticker pack link. Never contains the key.
    #[error("invalid sticker pack link: {0}")]
    InvalidLink(String),
    /// The CDN has no such pack or file (it answers 403).
    #[error("sticker pack not found")]
    NotFound,
    /// A MAC mismatch on the manifest: the key does not fit the pack.
    #[error("cannot decrypt the sticker pack: {0}")]
    Crypto(CryptoError),
    #[error("invalid sticker pack manifest: {0}")]
    Manifest(String),
    /// The credentials file is missing, unreadable or malformed. Never
    /// contains the password.
    #[error("Signal credentials: {0}")]
    Credentials(String),
    #[error("Signal refused the account credentials")]
    Unauthorized,
    #[error("Signal rate limit reached; try again {}", retry_text(.retry_after))]
    RateLimited { retry_after: Option<u64> },
    #[error("Signal answered HTTP {status} to {what}")]
    Status { status: u16, what: String },
    /// Network or TLS failure (the URL is stripped).
    #[error("http: {0}")]
    Http(reqwest::Error),
    #[error("file is larger than {0} bytes")]
    TooLarge(u64),
    #[error("{0}")]
    Invalid(String),
}

fn retry_text(after: &Option<u64>) -> String {
    match after {
        Some(s) => format!("in {s} s"),
        None => "later".to_string(),
    }
}
