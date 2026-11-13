//! Quick-search popup logic shared by every UI shell (GTK now; SwiftUI and
//! Kotlin later via UniFFI). No toolkit, no I/O except [`focus`].
//!
//! * [`model`]: state machine: tabs, modes, query, results, selection,
//!   target app, sticker toggle, status; events in, commands out.
//! * [`tabs`]: the tabs and the Insert / Normal modes.
//! * [`vim`]: Normal-mode key sequences (counts, `g` prefixes) and the key help.
//! * [`focus`]: focused-window detection (Hyprland IPC, Sway IPC, X11).
//! * [`history`]: per-window "last used here" ordering of recent items.

pub mod focus;
pub mod history;
pub mod model;
pub mod tabs;
pub mod vim;

pub use focus::{FocusBackend, WindowInfo};
pub use model::{Command, Event, Model, ModelConfig, Nav, OpenInfo, Results};
pub use tabs::{Mode, PageKey, Tab, View};
pub use vim::{HelpSection, NormalKey, HELP};
