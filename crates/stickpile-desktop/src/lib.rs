//! stickpile Linux desktop client: a resident GTK4/libadwaita quick-search
//! popup. See `docs/desktop.md`.
//!
//! Display-free (unit-tested) modules:
//! * [`cli`] — command line and what each invocation does
//! * [`keys`] — key press → popup event
//! * [`offers`] — clipboard / drag payloads from `PreparedCopy`
//! * [`thumbs`] — thumbnail decoding to RGBA
//! * [`sync`] — background sync schedule
//! * [`glue`] — config → model, focus capture → target, window geometry, history file
//! * [`backend`] — the `Backend` trait over `stickpile_core::Session`
//! * [`demo`] — `--demo DIR` backend serving a directory of files
//! * [`pages`]: texts and decisions of the Me and Settings tabs
//!
//! GTK modules: [`app`] (resident application) and [`ui`] (window, tabs,
//! grids, pages, drag source).

pub mod app;
pub mod backend;
pub mod cli;
pub mod demo;
pub mod glue;
pub mod keys;
pub mod offers;
pub mod pages;
pub mod sync;
pub mod thumbs;
pub mod ui;
