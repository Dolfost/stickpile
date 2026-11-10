//! Wayland backend: data-control protocol through wl-clipboard-rs, served in
//! the foreground of the (detached) helper process until another client sets
//! the selection.
//!
//! Note: wl-clipboard-rs keeps offers in a hash map, so the order in which the
//! MIME types are announced is not the configured order. Chat apps choose by
//! type, not by position, so this has not mattered in practice.

use wl_clipboard_rs::copy::{ClipboardType, Error, MimeSource, MimeType, Options, Seat, Source};

use super::ClipOffer;

pub(crate) enum WaylandError {
    /// The compositor lacks the data-control protocol (e.g. GNOME): try X11.
    Unsupported(String),
    Other(String),
}

pub(crate) fn serve(offers: Vec<ClipOffer>, ready: &mut dyn FnMut()) -> Result<(), WaylandError> {
    let sources: Vec<MimeSource> = offers
        .into_iter()
        .map(|o| MimeSource {
            source: Source::Bytes(o.data.into_boxed_slice()),
            mime_type: MimeType::Specific(o.mime),
        })
        .collect();
    let mut opts = Options::new();
    opts.clipboard(ClipboardType::Regular)
        .seat(Seat::All)
        .foreground(true)
        // Offer exactly the configured types: text/uri-list must not also
        // appear as text/plain, UTF8_STRING, ...
        .omit_additional_text_mime_types(true);
    let prepared = opts.prepare_copy_multi(sources).map_err(|e| match e {
        Error::MissingProtocol { .. } | Error::WaylandConnection(_) | Error::SocketOpenError(_) => {
            WaylandError::Unsupported(format!("Wayland clipboard: {e}"))
        }
        other => WaylandError::Other(format!("Wayland clipboard: {other}")),
    })?;
    ready();
    prepared
        .serve()
        .map_err(|e| WaylandError::Other(format!("Wayland clipboard: {e}")))
}
