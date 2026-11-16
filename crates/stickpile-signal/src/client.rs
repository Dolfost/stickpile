//! HTTP access to Signal's CDN (pack files) and chat service (upload forms).
//!
//! Hosts (Signal-Desktop `config/production.json`): the service is
//! `https://chat.signal.org` and pack files live on CDN 0,
//! `https://cdn.signal.org`, under `stickers/<pack id>/manifest.proto` and
//! `stickers/<pack id>/full/<sticker id>` (Signal-Desktop
//! `ts/textsecure/WebAPI.preload.ts`: `getStickerPackManifest`,
//! `getSticker`).
//!
//! TLS: Signal's clients trust only Signal's own root certificate for these
//! hosts. Signal-Desktop passes `certificateAuthority` from
//! `config/default.json` as the `ca` of every CDN request (which replaces
//! the default roots), and signalstickers-client verifies both hosts against
//! the same certificate (`utils/ca/cacert.pem`; SHA-256 of the DER below).
//! The certificate is public and pinned here as the only trust anchor. Set
//! [`SignalConfig::trust_public_roots`] to also accept the public web PKI,
//! should Signal move a host to a public CA. This crate's tests never
//! connect to Signal; the pinning was not checked against the live hosts.

use std::time::Duration;

use reqwest::StatusCode;
use serde::Deserialize;

use crate::SignalError;

pub const DEFAULT_SERVICE_URL: &str = "https://chat.signal.org";
pub const DEFAULT_CDN_URL: &str = "https://cdn.signal.org";

/// "Signal Messenger, LLC" root CA (valid 2022-01-26 to 2032-01-24), from
/// Signal-Desktop `config/default.json` (`certificateAuthority`).
pub const SIGNAL_ROOT_CA_PEM: &str = include_str!("signal_root_ca.pem");
/// SHA-256 of [`SIGNAL_ROOT_CA_PEM`]'s DER bytes. The same certificate ships
/// in signalstickers-client.
pub const SIGNAL_ROOT_CA_SHA256: &str = "ddb0f92bb95c8d6fd202ea6e8cc5ccd182b544f8cd696f47d580659ddc9df65a";

/// Encrypted files larger than this are refused (Signal-Desktop
/// `MAX_STICKER_CIPHERTEXT_BYTE_LENGTH` and
/// `MAX_STICKER_PACK_MANIFEST_CIPHERTEXT_BYTE_LENGTH`: 1 MiB each).
pub const MAX_CIPHERTEXT_BYTES: u64 = 1 << 20;

/// Where to reach Signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalConfig {
    pub service_url: String,
    pub cdn_url: String,
    /// Trust the public web PKI in addition to Signal's root (default:
    /// Signal's root only).
    pub trust_public_roots: bool,
}

impl Default for SignalConfig {
    fn default() -> SignalConfig {
        SignalConfig {
            service_url: DEFAULT_SERVICE_URL.into(),
            cdn_url: DEFAULT_CDN_URL.into(),
            trust_public_roots: false,
        }
    }
}

/// One signed S3 POST policy from the upload form.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct FormItem {
    #[serde(default)]
    pub id: i64,
    pub key: String,
    pub credential: String,
    pub acl: String,
    pub algorithm: String,
    pub date: String,
    pub policy: String,
    pub signature: String,
}

/// `GET /v1/sticker/pack/form/{count}` (Signal-Server `StickerController`,
/// `StickerPackFormUploadAttributes`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UploadForm {
    pub pack_id: String,
    pub manifest: FormItem,
    pub stickers: Vec<FormItem>,
}

#[derive(Clone)]
pub struct SignalClient {
    http: reqwest::Client,
    service: String,
    cdn: String,
}

impl std::fmt::Debug for SignalClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignalClient")
            .field("service", &self.service)
            .field("cdn", &self.cdn)
            .finish_non_exhaustive()
    }
}

/// reqwest errors carry the URL, which names the pack.
fn http_err(e: reqwest::Error) -> SignalError {
    SignalError::Http(e.without_url())
}

fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers.get("retry-after")?.to_str().ok()?.trim().parse().ok()
}

impl SignalClient {
    pub fn new(config: &SignalConfig) -> Result<SignalClient, SignalError> {
        let ca = reqwest::Certificate::from_pem(SIGNAL_ROOT_CA_PEM.as_bytes())
            .map_err(|_| SignalError::Invalid("the pinned Signal root certificate does not parse".into()))?;
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(120))
            .user_agent(concat!("stickpile/", env!("CARGO_PKG_VERSION")))
            .tls_built_in_root_certs(config.trust_public_roots)
            .add_root_certificate(ca)
            .build()
            .map_err(http_err)?;
        Ok(SignalClient {
            http,
            service: config.service_url.trim_end_matches('/').to_string(),
            cdn: config.cdn_url.trim_end_matches('/').to_string(),
        })
    }

    /// A client for Signal's production hosts.
    pub fn production() -> Result<SignalClient, SignalError> {
        SignalClient::new(&SignalConfig::default())
    }

    /// GET `<cdn>/<path>` capped at `cap` bytes. Signal's CDN answers 403
    /// for files that do not exist (signalstickers-client `downloader.py`);
    /// 403 and 404 become [`SignalError::NotFound`].
    pub(crate) async fn get_cdn(&self, path: &str, cap: u64) -> Result<Vec<u8>, SignalError> {
        let response = self
            .http
            .get(format!("{}/{path}", self.cdn))
            .send()
            .await
            .map_err(http_err)?;
        match response.status() {
            s if s.is_success() => read_capped(response, cap).await,
            StatusCode::FORBIDDEN | StatusCode::NOT_FOUND => Err(SignalError::NotFound),
            s => Err(SignalError::Status {
                status: s.as_u16(),
                what: "a CDN download".into(),
            }),
        }
    }

    /// Asks the service for `count` sticker upload policies plus one for
    /// the manifest (1 <= count <= 201: up to 200 stickers and a cover).
    pub(crate) async fn upload_form(
        &self,
        credentials: &crate::Credentials,
        count: usize,
    ) -> Result<UploadForm, SignalError> {
        let response = self
            .http
            .get(format!("{}/v1/sticker/pack/form/{count}", self.service))
            .basic_auth(credentials.username(), Some(credentials.password()))
            .send()
            .await
            .map_err(http_err)?;
        let status = response.status();
        match status {
            s if s.is_success() => {}
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => return Err(SignalError::Unauthorized),
            // Signal answers rate limits with 413 (older) or 429.
            StatusCode::PAYLOAD_TOO_LARGE | StatusCode::TOO_MANY_REQUESTS => {
                return Err(SignalError::RateLimited {
                    retry_after: retry_after(response.headers()),
                })
            }
            s => {
                return Err(SignalError::Status {
                    status: s.as_u16(),
                    what: "the sticker upload form request".into(),
                })
            }
        }
        let body = read_capped(response, 4 << 20).await?;
        serde_json::from_slice(&body)
            .map_err(|_| SignalError::Invalid("Signal's sticker upload form is not in the expected shape".into()))
    }

    /// POSTs `data` to the CDN with the S3 policy fields, in the order
    /// Signal-Desktop sends them (`makePutParams`): key, x-amz-credential,
    /// acl, x-amz-algorithm, x-amz-date, policy, x-amz-signature,
    /// Content-Type, then the file.
    pub(crate) async fn post_cdn(&self, item: &FormItem, data: Vec<u8>) -> Result<(), SignalError> {
        let file = reqwest::multipart::Part::bytes(data)
            .mime_str("application/octet-stream")
            .map_err(http_err)?;
        let form = reqwest::multipart::Form::new()
            .text("key", item.key.clone())
            .text("x-amz-credential", item.credential.clone())
            .text("acl", item.acl.clone())
            .text("x-amz-algorithm", item.algorithm.clone())
            .text("x-amz-date", item.date.clone())
            .text("policy", item.policy.clone())
            .text("x-amz-signature", item.signature.clone())
            .text("Content-Type", "application/octet-stream")
            .part("file", file);
        let response = self
            .http
            .post(format!("{}/", self.cdn))
            .multipart(form)
            .send()
            .await
            .map_err(http_err)?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(SignalError::Status {
                status: response.status().as_u16(),
                what: "a CDN upload".into(),
            })
        }
    }
}

/// Reads a response body, failing as soon as it exceeds `cap` bytes.
async fn read_capped(mut response: reqwest::Response, cap: u64) -> Result<Vec<u8>, SignalError> {
    if response.content_length().is_some_and(|len| len > cap) {
        return Err(SignalError::TooLarge(cap));
    }
    let mut out = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(http_err)? {
        if out.len() as u64 + chunk.len() as u64 > cap {
            return Err(SignalError::TooLarge(cap));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn the_pinned_root_is_the_known_certificate() {
        let pem = SIGNAL_ROOT_CA_PEM;
        let body: String = pem
            .lines()
            .filter(|l| !l.starts_with("-----"))
            .collect::<Vec<_>>()
            .concat();
        assert!(pem.starts_with("-----BEGIN CERTIFICATE-----"));
        assert_eq!(pem.matches("BEGIN CERTIFICATE").count(), 1);
        let der = base64_decode(&body);
        assert_eq!(hex::encode(Sha256::digest(&der)), SIGNAL_ROOT_CA_SHA256);
        assert!(reqwest::Certificate::from_pem(pem.as_bytes()).is_ok());
        assert!(SignalClient::production().is_ok());
    }

    /// Standard base64 without padding checks (test only).
    fn base64_decode(s: &str) -> Vec<u8> {
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = Vec::new();
        let mut acc = 0u32;
        let mut bits = 0;
        for c in s.bytes().filter(|c| *c != b'=') {
            let v = alphabet.iter().position(|a| *a == c).expect("base64") as u32;
            acc = acc << 6 | v;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
                acc &= (1 << bits) - 1;
            }
        }
        out
    }
}
