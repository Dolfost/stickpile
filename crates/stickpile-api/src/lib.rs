//! Shared contract between the stickpile server and every client.
//!
//! * [`model`]: REST request/response DTOs (JSON). Timestamps are unix seconds
//!   (`i64`), IDs are opaque server-generated strings (26 lowercase base32 chars).
//! * [`routes`]: every REST path, so server and clients cannot drift.
//! * [`query`]: the keyboard-first search syntax (`e:😂 set:cats q>70 ...`).
//! * [`profiles`]: what each target chat app accepts.
//!
//! Enable the `openapi` feature to derive `utoipa::ToSchema` on all DTOs.

pub mod model;
pub mod profiles;
pub mod query;
pub mod routes;

pub use model::*;
