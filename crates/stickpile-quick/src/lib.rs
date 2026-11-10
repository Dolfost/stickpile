//! Quick-search popup logic shared by every UI shell (GTK now; SwiftUI and
//! Kotlin later via UniFFI). No toolkit, no I/O except [`focus`].
//!
//! * [`model`] — state machine: query, results, selection, target app,
//!   sticker toggle, status; events in, commands out.
//! * [`focus`] — focused-window detection (Hyprland IPC, Sway IPC, X11).
//! * [`history`] — per-window "last used here" ordering of recent items.

pub mod focus;
pub mod history;
pub mod model;

pub use focus::{FocusBackend, WindowInfo};
pub use model::{Command, Event, Model, ModelConfig, Nav, OpenInfo, Results};
