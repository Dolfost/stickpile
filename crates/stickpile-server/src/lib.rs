//! stickpile server library. `main.rs` is a thin CLI over [`run`] and the
//! admin subcommands; integration tests start the real app via [`app`].

pub mod config;

pub use config::Config;
