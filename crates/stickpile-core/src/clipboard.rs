//! Clipboard offers: which MIME types carry which bytes for an item (driven by
//! `ClientConfig::clipboard`, provisional until docs/clipboard.md is filled),
//! and CLI clipboard backends. The GTK client sets offers through GDK itself.

/// One MIME representation offered on the clipboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipOffer {
    pub mime: String,
    pub data: Vec<u8>,
}
