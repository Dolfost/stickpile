//! stickpile server library. `main.rs` is a thin CLI over [`run`] and the
//! admin subcommands; integration tests build the real app with
//! [`build_state`] + [`router`] / [`serve_tcp`] / [`serve_unix`].
//!
//! Layout:
//! * [`config`] — the TOML configuration (mirrored by the Nix module).
//! * [`app`] — startup, router, listeners, background tasks.
//! * [`state`] — [`AppState`]: config, SQLite pool, search index, AI, limiters.
//! * [`auth`] — passwords, TOTP, tokens, sessions, tailnet identity, the
//!   `CurrentUser` extractor and CSRF; [`authz`] — the visibility table.
//! * [`security`] — listeners, client IP, rate limits, headers, origin checks.
//! * [`service`] — business logic shared by [`api`] (JSON) and [`web`] (HTML).
//! * [`jobs`] — DB-backed job queue and handlers; [`index`] — search sync.

pub mod api;
pub mod app;
pub mod assets;
pub mod auth;
pub mod authz;
pub mod config;
pub mod db;
pub mod error;
pub mod files;
pub mod i18n;
pub mod ids;
pub mod index;
pub mod jobs;
pub mod security;
pub mod service;
pub mod state;
pub mod web;

pub use app::{
    bind_unix, build_state, cleanup_once, router, run, serve_tcp, serve_unix, spawn_background, EnricherSource,
    StartOptions,
};
pub use config::Config;
pub use error::{AppError, AppResult};
pub use state::AppState;
