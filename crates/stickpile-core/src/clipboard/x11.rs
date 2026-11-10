//! X11 backend: own the `CLIPBOARD` selection from an unmapped window and
//! answer `TARGETS`, `TIMESTAMP` and every offered MIME type (ICCCM §2).
//! Payloads larger than what fits in one request are sent with the `INCR`
//! protocol. Serving stops when another client takes the selection
//! (`SelectionClear`), after pending `INCR` transfers finish or time out.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, Property,
    SelectionNotifyEvent, SelectionRequestEvent, Timestamp, Window, WindowClass, SELECTION_NOTIFY_EVENT,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use super::ClipOffer;
use crate::{CoreError, Result};

/// Give up on an INCR transfer whose requestor stopped reading.
const INCR_STALL: Duration = Duration::from_secs(30);
/// Upper bound for a single property write.
const MAX_CHUNK: usize = 4 * 1024 * 1024;

/// How a payload of `len` bytes is sent when requests may be at most
/// `max_request_bytes`: `None` = in one property, `Some(chunk)` = INCR in
/// chunks of `chunk` bytes.
pub fn incr_plan(len: usize, max_request_bytes: usize) -> Option<usize> {
    // ChangeProperty has a 24-byte header; keep some slack.
    let limit = max_request_bytes.saturating_sub(64).clamp(1024, MAX_CHUNK);
    (len > limit).then_some(limit)
}

/// MIME type plus extra X11 target names that map onto the same payload.
pub fn target_names(mime: &str) -> Vec<&str> {
    match mime {
        "text/plain" => vec![
            "text/plain",
            "text/plain;charset=utf-8",
            "UTF8_STRING",
            "STRING",
            "TEXT",
        ],
        other => vec![other],
    }
}

fn err<E: std::fmt::Display>(e: E) -> CoreError {
    CoreError::Other(format!("X11 clipboard: {e}"))
}

struct Transfer {
    target_type: Atom,
    data: std::sync::Arc<[u8]>,
    offset: usize,
    chunk: usize,
    last_activity: Instant,
    done: bool,
}

struct Atoms {
    clipboard: Atom,
    targets: Atom,
    timestamp: Atom,
    incr: Atom,
    probe: Atom,
}

pub(crate) fn serve(offers: Vec<ClipOffer>, ready: &mut dyn FnMut()) -> Result<()> {
    let (conn, screen_num) = x11rb::connect(None).map_err(err)?;
    let root = conn.setup().roots[screen_num].root;
    let win = conn.generate_id().map_err(err)?;
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        win,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_OUTPUT,
        x11rb::COPY_FROM_PARENT,
        &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
    )
    .map_err(err)?;

    let intern = |name: &str| -> Result<Atom> {
        Ok(conn
            .intern_atom(false, name.as_bytes())
            .map_err(err)?
            .reply()
            .map_err(err)?
            .atom)
    };
    let atoms = Atoms {
        clipboard: intern("CLIPBOARD")?,
        targets: intern("TARGETS")?,
        timestamp: intern("TIMESTAMP")?,
        incr: intern("INCR")?,
        probe: intern("_STICKPILE_TIMESTAMP")?,
    };
    // target atom -> (type atom to answer with, payload)
    let mut payloads: Vec<(Atom, Atom, std::sync::Arc<[u8]>)> = Vec::new();
    for offer in offers {
        let data: std::sync::Arc<[u8]> = offer.data.into();
        for name in target_names(&offer.mime) {
            let atom = intern(name)?;
            if !payloads.iter().any(|(a, _, _)| *a == atom) {
                payloads.push((atom, atom, data.clone()));
            }
        }
    }

    let time = server_time(&conn, win, atoms.probe)?;
    conn.set_selection_owner(win, atoms.clipboard, time).map_err(err)?;
    conn.flush().map_err(err)?;
    let owner = conn
        .get_selection_owner(atoms.clipboard)
        .map_err(err)?
        .reply()
        .map_err(err)?
        .owner;
    if owner != win {
        return Err(CoreError::Other(
            "X11 clipboard: could not take ownership of CLIPBOARD".into(),
        ));
    }
    ready();

    let max_req = conn.maximum_request_bytes();
    let mut transfers: HashMap<(Window, Atom), Transfer> = HashMap::new();
    let mut lost_at: Option<Instant> = None;
    loop {
        transfers.retain(|_, t| !t.done && t.last_activity.elapsed() < INCR_STALL);
        if lost_at.is_some() && transfers.is_empty() {
            return Ok(());
        }
        let event = if transfers.is_empty() {
            Some(conn.wait_for_event().map_err(err)?)
        } else {
            match conn.poll_for_event().map_err(err)? {
                Some(e) => Some(e),
                None => {
                    std::thread::sleep(Duration::from_millis(10));
                    None
                }
            }
        };
        let Some(event) = event else { continue };
        match event {
            Event::SelectionRequest(req) => {
                answer(&conn, &atoms, time, &payloads, max_req, &mut transfers, &req)?;
            }
            Event::SelectionClear(c) if c.selection == atoms.clipboard => {
                lost_at.get_or_insert_with(Instant::now);
            }
            Event::PropertyNotify(p) if p.state == Property::DELETE => {
                if let Some(t) = transfers.get_mut(&(p.window, p.atom)) {
                    send_chunk(&conn, p.window, p.atom, t)?;
                }
            }
            _ => {}
        }
    }
}

/// Get a server timestamp by appending to a property on our own window.
fn server_time(conn: &RustConnection, win: Window, probe: Atom) -> Result<Timestamp> {
    conn.change_property8(PropMode::APPEND, win, probe, AtomEnum::STRING, &[])
        .map_err(err)?;
    conn.flush().map_err(err)?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        match conn.poll_for_event().map_err(err)? {
            Some(Event::PropertyNotify(p)) if p.window == win && p.atom == probe => return Ok(p.time),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    Ok(x11rb::CURRENT_TIME)
}

fn answer(
    conn: &RustConnection,
    atoms: &Atoms,
    time: Timestamp,
    payloads: &[(Atom, Atom, std::sync::Arc<[u8]>)],
    max_req: usize,
    transfers: &mut HashMap<(Window, Atom), Transfer>,
    req: &SelectionRequestEvent,
) -> Result<()> {
    // Obsolete clients pass property None: use the target atom (ICCCM).
    let property = if req.property == x11rb::NONE {
        req.target
    } else {
        req.property
    };
    let mut reply = property;
    if req.selection != atoms.clipboard {
        reply = x11rb::NONE;
    } else if req.target == atoms.targets {
        let mut list = vec![atoms.targets, atoms.timestamp];
        list.extend(payloads.iter().map(|(a, _, _)| *a));
        conn.change_property32(PropMode::REPLACE, req.requestor, property, AtomEnum::ATOM, &list)
            .map_err(err)?;
    } else if req.target == atoms.timestamp {
        conn.change_property32(PropMode::REPLACE, req.requestor, property, AtomEnum::INTEGER, &[time])
            .map_err(err)?;
    } else if let Some((_, ty, data)) = payloads.iter().find(|(a, _, _)| *a == req.target) {
        match incr_plan(data.len(), max_req) {
            None => {
                conn.change_property8(PropMode::REPLACE, req.requestor, property, *ty, data)
                    .map_err(err)?;
            }
            Some(chunk) => {
                conn.change_window_attributes(
                    req.requestor,
                    &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
                )
                .map_err(err)?;
                conn.change_property32(
                    PropMode::REPLACE,
                    req.requestor,
                    property,
                    atoms.incr,
                    &[data.len() as u32],
                )
                .map_err(err)?;
                transfers.insert(
                    (req.requestor, property),
                    Transfer {
                        target_type: *ty,
                        data: data.clone(),
                        offset: 0,
                        chunk,
                        last_activity: Instant::now(),
                        done: false,
                    },
                );
            }
        }
    } else {
        reply = x11rb::NONE;
    }
    let ev = SelectionNotifyEvent {
        response_type: SELECTION_NOTIFY_EVENT,
        sequence: 0,
        time: req.time,
        requestor: req.requestor,
        selection: req.selection,
        target: req.target,
        property: reply,
    };
    conn.send_event(false, req.requestor, EventMask::NO_EVENT, ev)
        .map_err(err)?;
    conn.flush().map_err(err)?;
    Ok(())
}

/// Write the next INCR chunk after the requestor deleted the property; an
/// empty write ends the transfer.
fn send_chunk(conn: &RustConnection, requestor: Window, property: Atom, t: &mut Transfer) -> Result<()> {
    let end = (t.offset + t.chunk).min(t.data.len());
    let piece = &t.data[t.offset..end];
    conn.change_property8(PropMode::REPLACE, requestor, property, t.target_type, piece)
        .map_err(err)?;
    conn.flush().map_err(err)?;
    if piece.is_empty() {
        t.done = true;
    }
    t.offset = end;
    t.last_activity = Instant::now();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_payloads_go_in_one_property() {
        assert_eq!(incr_plan(1000, 262_140), None);
        assert_eq!(incr_plan(262_000, 262_140), None);
    }

    #[test]
    fn large_payloads_use_incr_chunks_within_request_size() {
        let chunk = incr_plan(1_000_000, 262_140).unwrap();
        assert!(chunk < 262_140);
        assert!(chunk > 200_000);
        // big-requests servers: chunks are capped
        assert_eq!(incr_plan(100 * 1024 * 1024, 16 * 1024 * 1024 * 4), Some(MAX_CHUNK));
    }

    #[test]
    fn text_plain_has_x11_aliases() {
        assert!(target_names("text/plain").contains(&"UTF8_STRING"));
        assert_eq!(target_names("image/png"), vec!["image/png"]);
    }
}
