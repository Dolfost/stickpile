//! `--wl-data-control`: copy through the wlr/ext data-control protocol with
//! wl-clipboard-rs, like `wl-copy`. No window and no keyboard focus needed,
//! so this is the comparison point for the GDK path (and what the CLI's
//! `stickpile copy` would use).

use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Result};
use wl_clipboard_rs::copy::{MimeSource, MimeType, Options, Source};

use crate::payload;

pub fn copy(path: &Path, mimes: &[String], quit_after: Option<u64>) -> Result<()> {
    let refs: Vec<&str> = mimes.iter().map(String::as_str).collect();
    let (offers, skipped) = payload::build(path, &refs)?;
    if !skipped.is_empty() {
        eprintln!(
            "clipspike: cannot produce {} for this file, skipped",
            skipped.join(", ")
        );
    }
    if offers.is_empty() {
        bail!("nothing to offer");
    }
    eprintln!("clipspike: offering via data-control: {}", payload::summary(&offers));
    let sources = offers
        .into_iter()
        .map(|o| MimeSource {
            source: Source::Bytes(o.data.into_boxed_slice()),
            mime_type: MimeType::Specific(o.mime),
        })
        .collect();
    let mut opts = Options::new();
    // Offer exactly the chosen types (no automatic text/plain variants).
    opts.omit_additional_text_mime_types(true);
    match quit_after {
        None => {
            opts.foreground(true);
            eprintln!("clipspike: serving until the clipboard is replaced (Ctrl+C to stop)");
            opts.copy_multi(sources)?;
        }
        Some(secs) => {
            opts.foreground(false);
            opts.copy_multi(sources)?;
            eprintln!("clipspike: serving for {secs} s, then exiting");
            std::thread::sleep(Duration::from_secs(secs));
            eprintln!("clipspike: exited the owner — paste now to test persistence");
        }
    }
    Ok(())
}
