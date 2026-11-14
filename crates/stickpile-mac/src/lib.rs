//! stickpile for macOS, the Rust side: the GTK client's display-free parts
//! (`stickpile-desktop` without its `gtk` feature) and the popup model
//! (`stickpile-quick`), driven by [`driver::Driver`] and offered to the Swift
//! app in `app/` through UniFFI ([`bridge`]). See `docs/macos.md`.

uniffi::setup_scaffolding!();

pub mod bridge;
pub mod driver;
pub mod keys;
pub mod me;
pub mod pasteboard;
pub mod settings_page;

#[cfg(test)]
mod driver_tests;
