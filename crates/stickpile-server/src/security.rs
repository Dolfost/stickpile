//! Transport-level security: which listener a request came through, the
//! client IP, rate limiting, security headers and same-origin checks.

use std::net::{IpAddr, SocketAddr};
use std::num::NonZeroU32;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, Method};
use axum::middleware::Next;
use axum::response::Response;
use governor::clock::{Clock, DefaultClock};
use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Which listener accepted the connection. Inserted by a per-listener layer;
/// absent (= [`Listener::Tcp`] without a peer) for in-process test requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listener {
    /// The public TCP listener (`server.listen`).
    Tcp,
    /// The trusted unix socket of `tailscale serve` (`trusted_proxy.socket`).
    TrustedSocket,
}

/// Connection facts for one request.
#[derive(Debug, Clone, Copy)]
pub struct Peer {
    pub listener: Listener,
    /// TCP peer address; `None` on the unix socket.
    pub addr: Option<SocketAddr>,
}

impl Peer {
    pub fn from_request(req: &Request) -> Peer {
        req.extensions().get::<Peer>().copied().unwrap_or(Peer {
            listener: Listener::Tcp,
            addr: None,
        })
    }
}

/// Middleware for the TCP listener: turns axum's `ConnectInfo` into a [`Peer`].
pub async fn tag_tcp(req: Request, next: Next) -> Response {
    let addr = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0);
    let mut req = req;
    req.extensions_mut().insert(Peer {
        listener: Listener::Tcp,
        addr,
    });
    next.run(req).await
}

/// Middleware for the trusted unix socket.
pub async fn tag_trusted_socket(mut req: Request, next: Next) -> Response {
    req.extensions_mut().insert(Peer {
        listener: Listener::TrustedSocket,
        addr: None,
    });
    next.run(req).await
}

/// True if the request came from the trusted proxy: through the unix socket,
/// or over TCP from a loopback peer carrying the configured shared secret
/// (compared in constant time).
pub fn from_trusted_proxy(state: &AppState, peer: &Peer, headers: &HeaderMap) -> bool {
    match peer.listener {
        Listener::TrustedSocket => true,
        Listener::Tcp => {
            let (Some(secret), Some(addr)) = (state.proxy_secret.as_deref(), peer.addr) else {
                return false;
            };
            if !addr.ip().is_loopback() || secret.is_empty() {
                return false;
            }
            let Ok(name) = HeaderName::from_bytes(state.config.trusted_proxy.secret_header.as_bytes()) else {
                return false;
            };
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| crate::ids::ct_eq(v, secret))
        }
    }
}

/// Client IP for rate limiting and the audit log. `X-Forwarded-For` is only
/// honoured from the trusted proxy (tailscale serve sets it to the tailnet IP).
pub fn client_ip(state: &AppState, peer: &Peer, headers: &HeaderMap) -> Option<IpAddr> {
    if from_trusted_proxy(state, peer, headers) {
        if let Some(ip) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .and_then(|v| v.trim().parse::<IpAddr>().ok())
        {
            return Some(ip);
        }
    }
    peer.addr.map(|a| a.ip())
}

// ---------------------------------------------------------------------------
// Rate limiting
// ---------------------------------------------------------------------------

pub struct RateLimiters {
    login_ip: DefaultKeyedRateLimiter<String>,
    login_user: DefaultKeyedRateLimiter<String>,
    /// One shared bucket for logins whose client IP is unknown (the trusted
    /// unix socket without `X-Forwarded-For`).
    login_unknown_ip: DefaultKeyedRateLimiter<String>,
    upload_user: DefaultKeyedRateLimiter<String>,
    ai_suggest_user: DefaultKeyedRateLimiter<String>,
    webhook_ip: DefaultKeyedRateLimiter<String>,
}

/// Logins without a known client IP share one bucket this many times the
/// per-IP quota.
pub const UNKNOWN_IP_LOGIN_FACTOR: u32 = 5;
/// Unauthenticated git webhook calls per client IP (or for all callers with
/// an unknown IP together) and minute.
pub const WEBHOOKS_PER_MINUTE: u32 = 60;
const UNKNOWN_IP_KEY: &str = "unknown";

fn per_minute(n: u32) -> Quota {
    Quota::per_minute(NonZeroU32::new(n.max(1)).expect("non-zero"))
}

fn check(limiter: &DefaultKeyedRateLimiter<String>, key: String) -> AppResult<()> {
    limiter.check_key(&key).map_err(|not_until| {
        let wait = not_until.wait_time_from(DefaultClock::default().now());
        AppError::rate_limited(wait.as_secs() + u64::from(wait.subsec_nanos() > 0))
    })
}

impl RateLimiters {
    pub fn new(limits: &crate::config::LimitsConfig) -> RateLimiters {
        RateLimiters {
            login_ip: RateLimiter::keyed(per_minute(limits.login_per_minute)),
            login_user: RateLimiter::keyed(per_minute(limits.login_per_minute)),
            login_unknown_ip: RateLimiter::keyed(per_minute(
                limits.login_per_minute.saturating_mul(UNKNOWN_IP_LOGIN_FACTOR),
            )),
            upload_user: RateLimiter::keyed(per_minute(limits.upload_per_minute)),
            ai_suggest_user: RateLimiter::keyed(per_minute(limits.ai_suggest_per_minute)),
            webhook_ip: RateLimiter::keyed(per_minute(WEBHOOKS_PER_MINUTE)),
        }
    }

    /// Password logins, token requests and registrations: per client IP and
    /// per (lowercased) username. Every attempt counts, successful or not.
    ///
    /// Without a known IP (trusted socket without `X-Forwarded-For`) the
    /// per-username limit applies plus one bucket shared by all such callers
    /// with [`UNKNOWN_IP_LOGIN_FACTOR`] times the per-IP quota: it caps
    /// password spraying across usernames, while one caller still cannot lock
    /// out a particular account of someone else beyond its own username bucket.
    pub fn check_login(&self, ip: Option<IpAddr>, username: &str) -> AppResult<()> {
        match ip {
            Some(ip) => check(&self.login_ip, ip.to_string())?,
            None => check(&self.login_unknown_ip, UNKNOWN_IP_KEY.to_string())?,
        }
        check(&self.login_user, username.trim().to_lowercase())
    }

    /// Unauthenticated git webhook calls, per client IP (callers with an
    /// unknown IP share one bucket). Checked before the signature.
    pub fn check_webhook(&self, ip: Option<IpAddr>) -> AppResult<()> {
        let key = ip.map(|i| i.to_string()).unwrap_or_else(|| UNKNOWN_IP_KEY.to_string());
        check(&self.webhook_ip, key)
    }

    /// One unit per upload or import request.
    pub fn check_upload(&self, user_id: &str) -> AppResult<()> {
        check(&self.upload_user, user_id.to_string())
    }

    /// One unit per AI suggestion request (`POST /api/v1/ai/suggest`).
    pub fn check_ai_suggest(&self, user_id: &str) -> AppResult<()> {
        check(&self.ai_suggest_user, user_id.to_string())
    }

    /// Drop idle keys (called by the periodic cleanup).
    pub fn prune(&self) {
        self.login_ip.retain_recent();
        self.login_user.retain_recent();
        self.login_unknown_ip.retain_recent();
        self.upload_user.retain_recent();
        self.ai_suggest_user.retain_recent();
        self.webhook_ip.retain_recent();
    }
}

// ---------------------------------------------------------------------------
// Headers
// ---------------------------------------------------------------------------

pub const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data: blob:; \
     media-src 'self' blob:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'";
pub const PERMISSIONS_POLICY: &str = "camera=(), microphone=(), geolocation=(), payment=(), usb=()";

/// Adds the security headers to every response (handlers may set a stricter
/// value first; existing headers are kept).
pub async fn security_headers(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    let mut set = |name: HeaderName, value: &'static str| {
        if !h.contains_key(&name) {
            h.insert(name, HeaderValue::from_static(value));
        }
    };
    set(header::CONTENT_SECURITY_POLICY, CSP);
    set(header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    set(header::REFERRER_POLICY, "same-origin");
    set(header::X_FRAME_OPTIONS, "DENY");
    set(HeaderName::from_static("permissions-policy"), PERMISSIONS_POLICY);
    set(HeaderName::from_static("cross-origin-opener-policy"), "same-origin");
    if state.config.server.public_url.starts_with("https://") {
        set(header::STRICT_TRANSPORT_SECURITY, "max-age=31536000");
    }
    resp
}

// ---------------------------------------------------------------------------
// Same-origin checks
// ---------------------------------------------------------------------------

/// `scheme://host[:port]` of a URL, lowercased, default ports dropped.
pub fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?.to_ascii_lowercase();
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let authority = match (scheme.as_str(), authority.rsplit_once(':')) {
        ("http", Some((host, "80"))) | ("https", Some((host, "443"))) => host.to_string(),
        _ => authority,
    };
    Some(format!("{scheme}://{authority}"))
}

/// Same-origin check for state-changing browser requests (CSRF defence in
/// depth, and the login/register CSRF defence for the web UI):
///
/// * `Sec-Fetch-Site: cross-site` or `same-site` -> rejected;
/// * an `Origin` header must equal the origin of `server.public_url` or of the
///   request's own `Host` (any scheme) -> otherwise rejected;
/// * no `Origin` and no `Sec-Fetch-Site` (non-browser clients) -> allowed.
pub fn check_same_origin(state: &AppState, headers: &HeaderMap) -> AppResult<()> {
    if let Some(site) = headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()) {
        if site.eq_ignore_ascii_case("cross-site") || site.eq_ignore_ascii_case("same-site") {
            return Err(AppError::csrf("cross-site request rejected"));
        }
    }
    let Some(origin) = headers.get(header::ORIGIN) else {
        return Ok(());
    };
    let origin = origin.to_str().map_err(|_| AppError::csrf("invalid Origin header"))?;
    if origin == "null" {
        return Err(AppError::csrf("opaque origin rejected"));
    }
    let Some(origin) = origin_of(origin) else {
        return Err(AppError::csrf("invalid Origin header"));
    };
    if origin_of(&state.config.server.public_url).as_deref() == Some(origin.as_str()) {
        return Ok(());
    }
    if let Some(host) = headers.get(header::HOST).and_then(|v| v.to_str().ok()) {
        let host = host.to_ascii_lowercase();
        for scheme in ["http", "https"] {
            if origin_of(&format!("{scheme}://{host}")).as_deref() == Some(origin.as_str()) {
                return Ok(());
            }
        }
    }
    Err(AppError::csrf("cross-origin request rejected"))
}

pub fn is_unsafe_method(m: &Method) -> bool {
    !matches!(*m, Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_normalise() {
        assert_eq!(
            origin_of("https://Example.com:443/x").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            origin_of("http://localhost:8080").as_deref(),
            Some("http://localhost:8080")
        );
        assert_eq!(origin_of("http://a@b").as_deref(), None);
        assert_eq!(origin_of("ftp://x").as_deref(), None);
    }

    #[test]
    fn limiter_returns_retry_after() {
        let l = RateLimiters::new(&crate::config::LimitsConfig {
            login_per_minute: 2,
            ..Default::default()
        });
        assert!(l.check_login(None, "a").is_ok());
        assert!(l.check_login(None, "b").is_ok());
        let e = l
            .check_login(None, "a")
            .and_then(|_| l.check_login(None, "a"))
            .unwrap_err();
        assert_eq!(e.code, "rate_limited");
        assert!(e.retry_after.unwrap() >= 1);
        // A known IP is limited across usernames.
        let ip = Some("192.0.2.7".parse().unwrap());
        assert!(l.check_login(ip, "x1").is_ok());
        assert!(l.check_login(ip, "x2").is_ok());
        assert_eq!(l.check_login(ip, "x3").unwrap_err().code, "rate_limited");
        assert!(l.check_login(Some("192.0.2.8".parse().unwrap()), "x4").is_ok());
    }

    #[test]
    fn unknown_ips_share_a_larger_bucket() {
        let l = RateLimiters::new(&crate::config::LimitsConfig {
            login_per_minute: 2,
            ..Default::default()
        });
        // Spraying many usernames without a known IP stops at 5 x 2 attempts.
        let ok = (0..20)
            .filter(|i| l.check_login(None, &format!("user{i}")).is_ok())
            .count();
        assert_eq!(ok, 2 * UNKNOWN_IP_LOGIN_FACTOR as usize);
        // Callers with a known IP are not affected.
        assert!(l.check_login(Some("192.0.2.9".parse().unwrap()), "user0").is_ok());
    }

    #[test]
    fn webhooks_limited_per_ip() {
        let l = RateLimiters::new(&crate::config::LimitsConfig::default());
        let ip = Some("198.51.100.1".parse().unwrap());
        let ok = (0..100).filter(|_| l.check_webhook(ip).is_ok()).count();
        assert_eq!(ok, WEBHOOKS_PER_MINUTE as usize);
        assert!(l.check_webhook(Some("198.51.100.2".parse().unwrap())).is_ok());
        assert!(l.check_webhook(None).is_ok());
    }
}
