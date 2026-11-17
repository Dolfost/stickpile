//! [`LibraryError`], the one error of the interface, and how
//! [`stickpile_core::CoreError`] maps onto it (see the crate docs for the
//! table).

use stickpile_core::config::ConfigError;
use stickpile_core::CoreError;

/// Everything that can go wrong in a `StickpileLibrary` call. Each variant is
/// a kind the app turns into its own (translated) text; `detail` says what
/// happened in English, for logs only, and never contains a token or a
/// password. URLs in it are cut down to their scheme, host, port and path:
/// a request's query (a search query) or a URL's user name and password
/// never reach the app's logs.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum LibraryError {
    /// No account: no token is stored and the server does not know this
    /// device by its tailnet identity. Sign in first.
    #[error("not signed in: {detail}")]
    NotSignedIn { detail: String },
    /// The server refused the credentials: a revoked or expired token, a
    /// wrong password or two-factor code, a wrong, expired or used pairing
    /// code (HTTP 401).
    #[error("unauthorized: {detail}")]
    Unauthorized { detail: String },
    /// The server could not be reached (no network, refused, timed out, a
    /// proxy's 502/503/504), and the offline cache cannot answer instead.
    #[error("server unreachable: {detail}")]
    Unreachable { detail: String },
    /// No such item or pack, or not one this account may see (HTTP 404).
    #[error("not found: {detail}")]
    NotFound { detail: String },
    /// Too many requests; try again later (HTTP 429).
    #[error("rate limited: {detail}")]
    RateLimited { detail: String },
    /// `prepare`: the item has no rendition of a type the text field accepts.
    #[error("nothing fits: {detail}")]
    NothingFits { detail: String },
    /// An argument is not acceptable. `field` names it: `server_url`,
    /// `code` (a pairing code), `token_name`, `totp` (the account needs a
    /// two-factor code and none was given), `variant`, `file` (an upload the
    /// server refused: not a picture or video, too large), `config_file` (it
    /// cannot be read as a stickpile config).
    #[error("invalid {field}: {detail}")]
    InvalidInput { field: String, detail: String },
    /// Any other HTTP error from the server, with its status.
    #[error("server error {status}: {detail}")]
    Server { status: u16, detail: String },
    /// Local storage failed: the cache database, a file, the token file.
    #[error("storage: {detail}")]
    Io { detail: String },
    /// Anything else.
    #[error("{detail}")]
    Other { detail: String },
}

/// The kind of a `LibraryError`, without its details
/// (`SyncState.last_error`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum LibraryErrorKind {
    NotSignedIn,
    Unauthorized,
    Unreachable,
    NotFound,
    RateLimited,
    NothingFits,
    InvalidInput,
    Server,
    Io,
    Other,
}

impl LibraryError {
    /// The variant without its fields.
    pub fn kind(&self) -> LibraryErrorKind {
        match self {
            LibraryError::NotSignedIn { .. } => LibraryErrorKind::NotSignedIn,
            LibraryError::Unauthorized { .. } => LibraryErrorKind::Unauthorized,
            LibraryError::Unreachable { .. } => LibraryErrorKind::Unreachable,
            LibraryError::NotFound { .. } => LibraryErrorKind::NotFound,
            LibraryError::RateLimited { .. } => LibraryErrorKind::RateLimited,
            LibraryError::NothingFits { .. } => LibraryErrorKind::NothingFits,
            LibraryError::InvalidInput { .. } => LibraryErrorKind::InvalidInput,
            LibraryError::Server { .. } => LibraryErrorKind::Server,
            LibraryError::Io { .. } => LibraryErrorKind::Io,
            LibraryError::Other { .. } => LibraryErrorKind::Other,
        }
    }

    pub(crate) fn invalid(field: &str, detail: impl Into<String>) -> LibraryError {
        LibraryError::InvalidInput {
            field: field.into(),
            detail: redact_urls(&detail.into()),
        }
    }

    pub(crate) fn not_found(detail: impl Into<String>) -> LibraryError {
        LibraryError::NotFound {
            detail: redact_urls(&detail.into()),
        }
    }

    pub(crate) fn other(detail: impl Into<String>) -> LibraryError {
        LibraryError::Other {
            detail: redact_urls(&detail.into()),
        }
    }

    /// [`From<CoreError>`], except that the server refusing the request as
    /// malformed (HTTP 400, 413, 415, 422) is [`LibraryError::InvalidInput`]
    /// for `field`: the argument of this call the request carried.
    pub(crate) fn input(e: CoreError, field: &str) -> LibraryError {
        match e.status() {
            Some(400 | 413 | 415 | 422) => LibraryError::invalid(field, e.to_string()),
            _ => e.into(),
        }
    }
}

/// `text` with every URL in it (`scheme://...` up to the next whitespace)
/// cut down to its scheme, host, port and path: a user name and password,
/// a query and a fragment are dropped. Punctuation right after the URL
/// (reqwest writes `for url (<url>): ...`) stays.
pub(crate) fn redact_urls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(sep) = rest.find("://") {
        let scheme_len = rest[..sep]
            .bytes()
            .rev()
            .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
            .count();
        let start = sep - scheme_len;
        let end = rest[sep..].find(char::is_whitespace).map_or(rest.len(), |i| sep + i);
        out.push_str(&rest[..start]);
        if scheme_len == 0 {
            // `://` without a scheme: not a URL.
            out.push_str(&rest[start..end]);
        } else {
            out.push_str(&redact_url(&rest[start..end], sep - start));
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// One URL of [`redact_urls`]; `sep` is where its `://` starts.
fn redact_url(url: &str, sep: usize) -> String {
    let (scheme, after) = url.split_at(sep + 3);
    let authority_len = after.find(['/', '?', '#']).unwrap_or(after.len());
    let (authority, tail) = after.split_at(authority_len);
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let path_len = tail.find(['?', '#']).unwrap_or(tail.len());
    let (path, dropped) = tail.split_at(path_len);
    // What closes the text around the URL, e.g. `)` or `):`.
    let closing = dropped.len()
        - dropped
            .trim_end_matches([')', ']', '>', '"', '\'', ',', ';', ':', '.'])
            .len();
    format!("{scheme}{host}{path}{}", &dropped[dropped.len() - closing..])
}

impl From<CoreError> for LibraryError {
    fn from(e: CoreError) -> Self {
        let detail = redact_urls(&e.to_string());
        match e {
            CoreError::NotLoggedIn => LibraryError::NotSignedIn { detail },
            CoreError::Offline(_) | CoreError::NotOnDevice { .. } => LibraryError::Unreachable { detail },
            CoreError::Api { status, .. } => match status {
                401 => LibraryError::Unauthorized { detail },
                404 => LibraryError::NotFound { detail },
                429 => LibraryError::RateLimited { detail },
                status => LibraryError::Server { status, detail },
            },
            CoreError::Cache(_) | CoreError::Io(_) => LibraryError::Io { detail },
            CoreError::Other(_) => LibraryError::Other { detail },
        }
    }
}

impl From<ConfigError> for LibraryError {
    fn from(e: ConfigError) -> Self {
        let detail = redact_urls(&e.to_string());
        match e {
            ConfigError::Parse { .. } | ConfigError::Invalid(_) | ConfigError::Edit { .. } => {
                LibraryError::invalid("config_file", detail)
            }
            ConfigError::Io { .. } | ConfigError::ReadOnly { .. } | ConfigError::Write { .. } => {
                LibraryError::Io { detail }
            }
        }
    }
}

impl From<tokio::task::JoinError> for LibraryError {
    fn from(e: tokio::task::JoinError) -> Self {
        LibraryError::other(format!("background task: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stickpile_api::ApiError;

    fn api(status: u16, code: &str) -> CoreError {
        CoreError::Api {
            status,
            error: ApiError {
                code: code.into(),
                message: "m".into(),
            },
        }
    }

    #[test]
    fn core_errors_map_to_kinds() {
        let kind = |e: CoreError| LibraryError::from(e).kind();
        assert_eq!(kind(CoreError::NotLoggedIn), LibraryErrorKind::NotSignedIn);
        assert_eq!(kind(CoreError::Offline("x".into())), LibraryErrorKind::Unreachable);
        assert_eq!(kind(api(401, "unauthorized")), LibraryErrorKind::Unauthorized);
        assert_eq!(kind(api(401, "totp_required")), LibraryErrorKind::Unauthorized);
        assert_eq!(kind(api(404, "not_found")), LibraryErrorKind::NotFound);
        assert_eq!(kind(api(429, "rate_limited")), LibraryErrorKind::RateLimited);
        assert_eq!(kind(api(400, "bad_request")), LibraryErrorKind::Server);
        assert_eq!(kind(api(403, "forbidden")), LibraryErrorKind::Server);
        assert_eq!(kind(api(500, "internal")), LibraryErrorKind::Server);
        assert_eq!(kind(CoreError::Cache("x".into())), LibraryErrorKind::Io);
        assert_eq!(kind(std::io::Error::other("x").into()), LibraryErrorKind::Io);
        assert_eq!(kind(CoreError::Other("x".into())), LibraryErrorKind::Other);
        assert_eq!(
            LibraryError::from(api(503, "unavailable")),
            LibraryError::Server {
                status: 503,
                detail: "503: m (unavailable)".into()
            }
        );
    }

    #[test]
    fn urls_in_details_lose_their_query_and_userinfo() {
        for (text, want) in [
            (
                "error sending request for url (http://127.0.0.1:8080/api/v1/search?q=my+secret+crush&limit=50): client error (Connect)",
                "error sending request for url (http://127.0.0.1:8080/api/v1/search): client error (Connect)",
            ),
            (
                "https://alice:hunter2@s.example/base/api#frag and https://t.example/x?y=1",
                "https://s.example/base/api and https://t.example/x",
            ),
            ("(https://u@h?q=x).", "(https://h)."),
            ("http://h:81/p", "http://h:81/p"),
            ("no url: ://x and a?b=c", "no url: ://x and a?b=c"),
            ("\u{43f}\u{43e}\u{448}\u{443}\u{43a}: https://h/s?q=\u{43a}\u{456}\u{442}", "\u{43f}\u{43e}\u{448}\u{443}\u{43a}: https://h/s"),
            ("", ""),
        ] {
            assert_eq!(redact_urls(text), want, "{text:?}");
        }
    }

    /// A real transport error of the search, as reqwest words it, reaches
    /// the app without the query.
    #[tokio::test]
    async fn search_errors_do_not_carry_the_query() {
        let dead = stickpile_core::mock::dead_url();
        let client = stickpile_core::ApiClient::new(
            &dead.replace("http://", "http://alice:hunter2@"),
            std::time::Duration::from_secs(2),
        )
        .unwrap();
        let params = stickpile_core::client::SearchParams {
            q: "my secret crush".into(),
            ..Default::default()
        };
        let e = client.search(&params).await.unwrap_err();
        let raw = e.to_string();
        assert!(raw.contains("secret") && raw.contains("/api/v1/search"), "{raw}");
        let e = LibraryError::from(e);
        assert_eq!(e.kind(), LibraryErrorKind::Unreachable);
        let shown = e.to_string();
        assert!(shown.contains("/api/v1/search"), "{shown}");
        for secret in ["secret", "crush", "q=", "alice", "hunter2"] {
            assert!(!shown.contains(secret), "{secret}: {shown}");
        }
        // Through `input` and the other helpers too.
        let e = LibraryError::input(CoreError::Offline(raw.clone()), "file");
        assert!(!e.to_string().contains("secret"), "{e}");
        assert!(!LibraryError::other(raw).to_string().contains("secret"));
    }

    #[test]
    fn malformed_requests_name_the_field() {
        for status in [400, 413, 415, 422] {
            assert_eq!(
                LibraryError::input(api(status, "bad_request"), "file"),
                LibraryError::InvalidInput {
                    field: "file".into(),
                    detail: format!("{status}: m (bad_request)")
                }
            );
        }
        assert_eq!(
            LibraryError::input(api(401, "unauthorized"), "code").kind(),
            LibraryErrorKind::Unauthorized
        );
        assert_eq!(
            LibraryError::input(CoreError::Offline("x".into()), "code").kind(),
            LibraryErrorKind::Unreachable
        );
    }
}
