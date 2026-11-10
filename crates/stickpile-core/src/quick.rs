//! Toolkit-free state machine of the quick-search popup (owned by the desktop
//! agent): query text, results, selection, target app, sticker toggle,
//! offline/status messages. GTK (and later SwiftUI/Kotlin) shells render it
//! and feed it events; all logic is unit-tested here without a display.
