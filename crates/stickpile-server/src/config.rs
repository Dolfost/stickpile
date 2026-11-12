//! Server configuration, loaded from one TOML file.
//!
//! Every field here is mirrored one-to-one by the NixOS module
//! (`nix/modules/nixos.nix`, `services.stickpile.settings`). `nix flake check`
//! renders the module's example config to TOML and runs
//! `stickpile-server check-config` on it, and unknown keys are rejected, so the
//! two cannot drift. Secrets are only ever given as file paths (`*_file`).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    pub server: ServerConfig,
    pub trusted_proxy: TrustedProxyConfig,
    pub auth: AuthConfig,
    pub limits: LimitsConfig,
    pub media: MediaConfig,
    pub ai: AiConfig,
    pub telegram: TelegramConfig,
    pub git: GitConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct ServerConfig {
    /// TCP listener. Tailscale identity headers are never trusted on it
    /// (unless `trusted_proxy.secret_file` is set and the request carries it
    /// from a loopback address).
    pub listen: SocketAddr,
    /// Absolute external base URL, used in webhook and invitation links.
    pub public_url: String,
    /// All state: `stickpile.db`, `media/`, `gitpacks/`, `tmp/`.
    pub data_dir: PathBuf,
    /// tracing filter, e.g. `info` or `stickpile_server=debug,info`.
    pub log: String,
    /// Mark cookies `Secure`. Only disable for plain-http local development.
    pub secure_cookies: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            listen: "127.0.0.1:8080".parse().unwrap(),
            public_url: "http://localhost:8080".into(),
            data_dir: PathBuf::from("/var/lib/stickpile"),
            log: "info".into(),
            secure_cookies: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct TrustedProxyConfig {
    /// Extra unix-socket listener for `tailscale serve unix:<socket>`. Requests
    /// arriving here have their `Tailscale-User-Login` / `Tailscale-User-Name`
    /// headers trusted. Only root and the service user can connect.
    pub socket: Option<PathBuf>,
    /// Octal permission bits for `socket`, e.g. "0660".
    pub socket_mode: String,
    /// Alternative for a TCP reverse proxy on loopback: the proxy must send
    /// `secret_header: <contents of this file>`; only then, and only from a
    /// loopback peer, are identity headers trusted on the TCP listener.
    pub secret_file: Option<PathBuf>,
    pub secret_header: String,
}

impl Default for TrustedProxyConfig {
    fn default() -> Self {
        TrustedProxyConfig {
            socket: None,
            socket_mode: "0660".into(),
            secret_file: None,
            secret_header: "X-Stickpile-Proxy-Secret".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Registration {
    /// Only admins create accounts (plus tailscale auto-create if enabled).
    #[default]
    Closed,
    /// Anyone with an invitation link.
    Invite,
    /// Anyone who can reach the server.
    Open,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct AuthConfig {
    pub registration: Registration,
    /// Create a stickpile user the first time an unknown tailnet login arrives
    /// through the trusted proxy.
    pub tailscale_autocreate: bool,
    /// Tailnet logins (e.g. `alice@github`) that get the admin role when
    /// auto-created or on every login.
    pub tailscale_admins: Vec<String>,
    pub session_ttl_days: u32,
    /// Shortest password accepted when one is set (registration, settings,
    /// admin). 4..=128; existing passwords are not affected.
    pub min_password_length: usize,
    pub totp_issuer: String,
    /// Bootstrap: if the database has no users, create this admin on start.
    pub initial_admin_username: Option<String>,
    pub initial_admin_password_file: Option<PathBuf>,
}

/// Default of `auth.min_password_length`. (Here rather than in `auth` so this
/// file stays self-contained for the drift guard in `nix/drift-guard`.)
pub const DEFAULT_MIN_PASSWORD_LENGTH: usize = 8;
/// Lowest value `auth.min_password_length` may take.
pub const MIN_PASSWORD_LENGTH_FLOOR: usize = 4;

impl Default for AuthConfig {
    fn default() -> Self {
        AuthConfig {
            registration: Registration::Closed,
            tailscale_autocreate: true,
            tailscale_admins: Vec::new(),
            session_ttl_days: 30,
            min_password_length: DEFAULT_MIN_PASSWORD_LENGTH,
            totp_issuer: "stickpile".into(),
            initial_admin_username: None,
            initial_admin_password_file: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct LimitsConfig {
    /// Per uploaded file.
    pub max_upload_mb: u64,
    /// Per request (multi-file uploads, imports).
    pub max_request_mb: u64,
    /// Decoded still/frame size limit (width * height).
    pub max_pixels: u64,
    pub max_animation_frames: u32,
    pub max_video_seconds: u32,
    /// Per client IP and per username.
    pub login_per_minute: u32,
    /// Per user.
    pub upload_per_minute: u32,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        LimitsConfig {
            max_upload_mb: 50,
            max_request_mb: 500,
            max_pixels: 40_000_000,
            max_animation_frames: 3000,
            max_video_seconds: 120,
            login_per_minute: 10,
            upload_per_minute: 120,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct MediaConfig {
    /// ffmpeg/ffprobe binaries, invoked with fixed arguments on server-generated
    /// paths only (videos and animated variants).
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub ffmpeg_timeout_secs: u64,
    /// Longest side of grid thumbnails.
    pub thumb_px: u32,
    /// Size budget of the `discord_gif` variant.
    pub discord_gif_max_mb: u64,
    /// Concurrent background jobs (variants, enrichment, git, telegram).
    pub workers: u32,
    /// Default recommended longest side for sets that do not set one.
    pub default_recommended_px: u32,
    /// Max pHash Hamming distance considered a near-duplicate.
    pub near_duplicate_distance: u32,
}

impl Default for MediaConfig {
    fn default() -> Self {
        MediaConfig {
            ffmpeg: PathBuf::from("ffmpeg"),
            ffprobe: PathBuf::from("ffprobe"),
            ffmpeg_timeout_secs: 120,
            thumb_px: 256,
            discord_gif_max_mb: 10,
            workers: 2,
            default_recommended_px: 512,
            near_duplicate_distance: 8,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TextEncoder {
    /// OpenAI CLIP ViT-B/32 text tower (English).
    #[default]
    Clip,
    /// sentence-transformers clip-ViT-B-32-multilingual-v1 (50+ languages),
    /// aligned to the same image embedding space.
    Multilingual,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct AiConfig {
    pub enabled: bool,
    /// Directory with the ONNX models and tokenizers (layout: docs/ai.md).
    pub model_dir: PathBuf,
    pub text_encoder: TextEncoder,
    /// Path to libonnxruntime; default: `ORT_DYLIB_PATH` or the system library.
    pub onnxruntime_library: Option<PathBuf>,
    pub threads: u32,
    /// TOML with `keywords`, `emojis`, `vibes` tables replacing the built-in
    /// zero-shot vocabulary (format: docs/ai.md).
    pub vocabulary_file: Option<PathBuf>,
    /// Keywords suggested per item.
    pub max_keywords: u32,
    pub max_emojis: u32,
}

impl Default for AiConfig {
    fn default() -> Self {
        AiConfig {
            enabled: false,
            model_dir: PathBuf::from("/var/lib/stickpile/models"),
            text_encoder: TextEncoder::Clip,
            onnxruntime_library: None,
            threads: 2,
            vocabulary_file: None,
            max_keywords: 5,
            max_emojis: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct TelegramConfig {
    pub enabled: bool,
    pub bot_token_file: Option<PathBuf>,
    /// Bot API base URL (override for a local Bot API server or tests).
    pub api_url: String,
    /// Telegram user id that owns packs created on export, unless the
    /// exporting user has set their own in the web UI.
    pub default_owner_user_id: Option<i64>,
}

impl Default for TelegramConfig {
    fn default() -> Self {
        TelegramConfig {
            enabled: false,
            bot_token_file: None,
            api_url: "https://api.telegram.org".into(),
            default_owner_user_id: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct GitConfig {
    pub enabled: bool,
    pub pull_interval_minutes: u32,
    /// URL schemes users may bind: `https`, `ssh`, `file` (tests only).
    pub allowed_schemes: Vec<String>,
    /// For `ssh` remotes and pushing.
    pub ssh_key_file: Option<PathBuf>,
    /// HTTPS token used as password when pushing (`push` mode).
    pub https_token_file: Option<PathBuf>,
    pub https_username: String,
    pub author_name: String,
    pub author_email: String,
    /// Refuse repositories whose checkout exceeds this size.
    pub max_repo_mb: u64,
}

impl Default for GitConfig {
    fn default() -> Self {
        GitConfig {
            enabled: true,
            pull_interval_minutes: 60,
            allowed_schemes: vec!["https".into()],
            ssh_key_file: None,
            https_token_file: None,
            https_username: "x-access-token".into(),
            author_name: "stickpile".into(),
            author_email: "stickpile@localhost".into(),
            max_repo_mb: 1024,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("reading {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("parsing {path}: {source}")]
    Parse { path: PathBuf, source: toml::de::Error },
    #[error("invalid configuration: {0}")]
    Invalid(String),
}

impl Config {
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.into(),
            source,
        })?;
        let cfg: Config = toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.into(),
            source,
        })?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn from_toml(text: &str) -> Result<Config, ConfigError> {
        let cfg: Config = toml::from_str(text).map_err(|source| ConfigError::Parse {
            path: PathBuf::from("<string>"),
            source,
        })?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Static checks only; secret files are read (and their absence reported)
    /// at startup.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let bad = |m: String| Err(ConfigError::Invalid(m));
        if !(self.server.public_url.starts_with("http://") || self.server.public_url.starts_with("https://")) {
            return bad(format!(
                "server.public_url must be an absolute http(s) URL, got {:?}",
                self.server.public_url
            ));
        }
        if !self.server.data_dir.is_absolute() {
            return bad("server.data_dir must be an absolute path".into());
        }
        if u32::from_str_radix(self.trusted_proxy.socket_mode.trim_start_matches("0o"), 8).is_err() {
            return bad(format!(
                "trusted_proxy.socket_mode must be octal, got {:?}",
                self.trusted_proxy.socket_mode
            ));
        }
        if self.trusted_proxy.secret_header.trim().is_empty() {
            return bad("trusted_proxy.secret_header must not be empty".into());
        }
        if self.auth.session_ttl_days == 0 {
            return bad("auth.session_ttl_days must be > 0".into());
        }
        if !(MIN_PASSWORD_LENGTH_FLOOR..=128).contains(&self.auth.min_password_length) {
            return bad("auth.min_password_length must be 4..=128".into());
        }
        if self.auth.initial_admin_username.is_some() != self.auth.initial_admin_password_file.is_some() {
            return bad("auth.initial_admin_username and auth.initial_admin_password_file go together".into());
        }
        if self.limits.max_upload_mb == 0 || self.limits.max_request_mb < self.limits.max_upload_mb {
            return bad("limits.max_request_mb must be >= limits.max_upload_mb > 0".into());
        }
        if self.limits.login_per_minute == 0 || self.limits.upload_per_minute == 0 {
            return bad("rate limits must be > 0".into());
        }
        if self.media.workers == 0 {
            return bad("media.workers must be > 0".into());
        }
        if self.media.near_duplicate_distance > 32 {
            return bad("media.near_duplicate_distance must be <= 32".into());
        }
        if self.telegram.enabled && self.telegram.bot_token_file.is_none() {
            return bad("telegram.enabled requires telegram.bot_token_file".into());
        }
        for s in &self.git.allowed_schemes {
            if !matches!(s.as_str(), "https" | "http" | "ssh" | "file") {
                return bad(format!("git.allowed_schemes: unknown scheme {s:?}"));
            }
        }
        Ok(())
    }

    pub fn db_path(&self) -> PathBuf {
        self.server.data_dir.join("stickpile.db")
    }
    pub fn media_dir(&self) -> PathBuf {
        self.server.data_dir.join("media")
    }
    pub fn gitpacks_dir(&self) -> PathBuf {
        self.server.data_dir.join("gitpacks")
    }
    pub fn tmp_dir(&self) -> PathBuf {
        self.server.data_dir.join("tmp")
    }
}

/// Read a secret file, trimming one trailing newline.
pub fn read_secret(path: &Path) -> std::io::Result<String> {
    let s = std::fs::read_to_string(path)?;
    Ok(s.strip_suffix('\n').unwrap_or(&s).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_gives_defaults() {
        let c = Config::from_toml("").unwrap();
        assert_eq!(c, Config::default());
    }

    #[test]
    fn example_config_parses() {
        let text = include_str!("../config.example.toml");
        let c = Config::from_toml(text).unwrap();
        assert_eq!(c.auth.registration, Registration::Invite);
    }

    #[test]
    fn unknown_keys_rejected() {
        assert!(Config::from_toml("[server]\nlisten_addr = \"x\"").is_err());
        assert!(Config::from_toml("[nope]\n").is_err());
    }

    #[test]
    fn validation_catches_mistakes() {
        assert!(Config::from_toml("[server]\npublic_url = \"example.com\"").is_err());
        assert!(Config::from_toml("[server]\ndata_dir = \"relative\"").is_err());
        assert!(Config::from_toml("[telegram]\nenabled = true").is_err());
        assert!(Config::from_toml("[auth]\ninitial_admin_username = \"a\"").is_err());
        assert!(Config::from_toml("[git]\nallowed_schemes = [\"ftp\"]").is_err());
    }

    #[test]
    fn serialises_back_to_parseable_toml() {
        let c = Config::default();
        let s = toml::to_string(&c).unwrap();
        assert_eq!(Config::from_toml(&s).unwrap(), c);
    }
}
