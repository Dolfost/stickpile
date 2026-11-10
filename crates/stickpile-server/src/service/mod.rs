//! Business logic shared by the JSON API ([`crate::api`]) and the web UI
//! ([`crate::web`]). Handlers only extract, call one of these, and render.
//! Every function that takes a [`crate::auth::CurrentUser`] performs its own
//! authorization through [`crate::authz`].

pub mod account;
pub mod admin;
pub mod ai;
pub mod backup;
pub mod gitpack;
pub mod hydrate;
pub mod items;
pub mod search;
pub mod sets;
pub mod stats;
pub mod telegram;
pub mod transfer;
pub mod ui;
pub mod upload;
pub mod users;
