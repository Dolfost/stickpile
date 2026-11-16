//! Quick-search popup logic shared by every UI shell (GTK now; SwiftUI and
//! Kotlin later via UniFFI). No toolkit, no I/O except [`focus`].
//!
//! * [`model`]: state machine: tabs, modes, query, results, selection,
//!   target app, sticker toggle, status; events in, commands out.
//! * [`tabs`]: the tabs and the Insert / Normal modes.
//! * [`vim`]: Normal-mode key sequences (counts, `g` prefixes) and the key help.
//! * [`focus`]: focused-window detection (Hyprland IPC, Sway IPC, X11).
//! * [`history`]: per-window "last used here" ordering of recent items.
//! * [`suggest`]: the values offered while a `key:` filter is typed.
//! * [`within`]: the search syntax over the items of an opened pack.

pub mod focus;
pub mod history;
pub mod model;
pub mod suggest;
pub mod tabs;
pub mod vim;
pub mod within;

pub use focus::{FocusBackend, WindowInfo};
pub use model::{Command, Event, Model, ModelConfig, Nav, OpenInfo, Results};
pub use tabs::{Mode, PageKey, Tab, View};
pub use vim::{HelpSection, NormalKey, HELP, SYNTAX};

/// The client's version (the workspace's), shown next to the help button.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The project page the version opens.
pub const PROJECT_URL: &str = env!("CARGO_PKG_REPOSITORY");

/// The version as the popup shows it: `v0.5.1`.
pub fn version_label() -> String {
    format!("v{VERSION}")
}

/// What hovering the version says.
pub fn version_tooltip() -> String {
    stickpile_i18n::global::tf("Stickpile {version}: open the project page", &[("version", &VERSION)])
}

#[cfg(test)]
mod version_tests {
    #[test]
    fn the_version_links_to_the_project() {
        assert_eq!(super::version_label(), format!("v{}", super::VERSION));
        assert!(
            super::PROJECT_URL.starts_with("https://github.com/"),
            "{}",
            super::PROJECT_URL
        );
        assert!(super::version_tooltip().contains(super::VERSION));
    }
}
