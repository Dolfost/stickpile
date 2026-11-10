//! Quick-search popup logic shared by every UI shell (GTK now; SwiftUI and
//! Kotlin later via UniFFI). No toolkit, no I/O except [`focus`].
//!
//! * [`model`] — state machine: query, results, selection, target app,
//!   sticker toggle, status; events in, commands out.
//! * [`focus`] — focused-window detection (Hyprland IPC, Sway IPC, X11).

pub mod focus;
pub mod model;
