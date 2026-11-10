//! Keeping offers on the clipboard after the CLI exits.
//!
//! On Wayland the clipboard contents die with the process that set them, and
//! an X11 selection needs an owner that answers requests. The CLI therefore
//! re-executes itself with a hidden subcommand (`stickpile __serve-clipboard`)
//! via [`spawn_detached`]: the helper reads a [`ClipSpec`] from stdin, starts
//! a new session (`setsid`), takes the selection, reports `ok` on stdout,
//! points its stdio at `/dev/null`, and serves paste requests until another
//! client takes the clipboard ([`serve_stdin`]).
//!
//! Backend: Wayland (`wlr`/`ext` data-control through wl-clipboard-rs; works
//! on Hyprland, Sway, KDE) when `WAYLAND_DISPLAY` is set, else X11 when
//! `DISPLAY` is set. If the compositor lacks data-control (GNOME), the X11
//! backend is used through XWayland when `DISPLAY` is set.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::ClipOffer;
use crate::{CoreError, Result};

/// What the helper process serves.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClipSpec {
    pub offers: Vec<SpecOffer>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpecOffer {
    pub mime: String,
    /// Base64 (standard, padded).
    pub data: String,
}

impl ClipSpec {
    pub fn new(offers: &[ClipOffer]) -> ClipSpec {
        ClipSpec {
            offers: offers
                .iter()
                .map(|o| SpecOffer {
                    mime: o.mime.clone(),
                    data: data_encoding::BASE64.encode(&o.data),
                })
                .collect(),
        }
    }

    pub fn offers(&self) -> Result<Vec<ClipOffer>> {
        self.offers
            .iter()
            .map(|o| {
                let data = data_encoding::BASE64
                    .decode(o.data.as_bytes())
                    .map_err(|e| CoreError::Other(format!("clipboard spec: {e}")))?;
                Ok(ClipOffer {
                    mime: o.mime.clone(),
                    data,
                })
            })
            .collect()
    }
}

/// Which clipboard backend serves the offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Wayland,
    X11,
}

/// Backend for this session from `WAYLAND_DISPLAY` / `DISPLAY`, considering
/// only compiled-in backends.
pub fn detect_backend(wayland_display: Option<&str>, display: Option<&str>) -> Option<Backend> {
    let set = |v: Option<&str>| v.is_some_and(|s| !s.is_empty());
    if cfg!(feature = "wayland") && set(wayland_display) {
        return Some(Backend::Wayland);
    }
    if cfg!(feature = "x11") && set(display) {
        return Some(Backend::X11);
    }
    None
}

fn env_backend() -> Option<Backend> {
    let w = std::env::var("WAYLAND_DISPLAY").ok();
    let d = std::env::var("DISPLAY").ok();
    detect_backend(w.as_deref(), d.as_deref())
}

/// Start `exe args…` as a detached clipboard helper serving `offers`, and
/// wait (up to `wait`) until it owns the clipboard.
pub fn spawn_detached(exe: &Path, args: &[&str], offers: &[ClipOffer], wait: Duration) -> Result<()> {
    if offers.is_empty() {
        return Err(CoreError::Other(
            "nothing to put on the clipboard (no usable offers)".into(),
        ));
    }
    if env_backend().is_none() {
        return Err(CoreError::Other(
            "no clipboard available: neither WAYLAND_DISPLAY nor DISPLAY is set (or support was not compiled in)"
                .into(),
        ));
    }
    let spec = serde_json::to_vec(&ClipSpec::new(offers)).map_err(|e| CoreError::Other(e.to_string()))?;
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe; nothing else runs between fork and exec.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| CoreError::Other(format!("starting clipboard helper {}: {e}", exe.display())))?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| CoreError::Other("clipboard helper stdin".into()))?;
        stdin
            .write_all(&spec)
            .map_err(|e| CoreError::Other(format!("clipboard helper: {e}")))?;
    }
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| CoreError::Other("clipboard helper stdout".into()))?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let res = BufReader::new(stdout).read_line(&mut line).map(|_| line);
        let _ = tx.send(res);
    });
    match rx.recv_timeout(wait) {
        Ok(Ok(line)) => {
            let line = line.trim();
            if line == "ok" {
                Ok(())
            } else if let Some(msg) = line.strip_prefix("error: ") {
                Err(CoreError::Other(format!("clipboard: {msg}")))
            } else if line.is_empty() {
                Err(CoreError::Other(
                    "clipboard helper exited without taking the clipboard".into(),
                ))
            } else {
                Err(CoreError::Other(format!("clipboard helper: unexpected reply {line:?}")))
            }
        }
        Ok(Err(e)) => Err(CoreError::Other(format!("clipboard helper: {e}"))),
        Err(_) => {
            let _ = child.kill();
            Err(CoreError::Other(format!(
                "clipboard helper did not take the clipboard within {}s",
                wait.as_secs()
            )))
        }
    }
}

/// Entry point of the helper: read the spec from stdin, serve until the
/// selection is taken by someone else. Reports `ok` / `error: …` on stdout.
pub fn serve_stdin() -> Result<()> {
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input)?;
    let result = serde_json::from_slice::<ClipSpec>(&input)
        .map_err(|e| CoreError::Other(format!("clipboard spec: {e}")))
        .and_then(|spec| spec.offers())
        .and_then(|offers| serve_offers(offers, report_ready));
    if let Err(e) = &result {
        let mut out = std::io::stdout();
        let _ = writeln!(out, "error: {e}");
        let _ = out.flush();
    }
    result
}

fn report_ready() {
    let mut out = std::io::stdout();
    let _ = writeln!(out, "ok");
    let _ = out.flush();
    detach_stdio();
}

/// Point stdin/stdout/stderr at /dev/null so the parent's pipes close.
fn detach_stdio() {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        if let Ok(null) = std::fs::OpenOptions::new().read(true).write(true).open("/dev/null") {
            for fd in 0..3 {
                // SAFETY: dup2 onto the standard descriptors with a valid fd.
                unsafe {
                    libc::dup2(null.as_raw_fd(), fd);
                }
            }
        }
    }
}

/// Take the clipboard with `offers` using the session's backend, call
/// `ready` once owned, and block until ownership is lost.
pub fn serve_offers(offers: Vec<ClipOffer>, ready: impl FnOnce()) -> Result<()> {
    let backend = env_backend().ok_or_else(|| CoreError::Other("no WAYLAND_DISPLAY or DISPLAY".into()))?;
    let mut ready = Some(ready);
    let mut signal = move || {
        if let Some(r) = ready.take() {
            r()
        }
    };
    match backend {
        #[cfg(feature = "wayland")]
        Backend::Wayland => match super::wayland::serve(offers.clone(), &mut signal) {
            Ok(()) => Ok(()),
            Err(super::wayland::WaylandError::Unsupported(msg)) => {
                let has_x = std::env::var("DISPLAY").is_ok_and(|d| !d.is_empty());
                if cfg!(feature = "x11") && has_x {
                    tracing::debug!("{msg}; falling back to X11");
                    serve_x11(offers, &mut signal)
                } else {
                    Err(CoreError::Other(msg))
                }
            }
            Err(super::wayland::WaylandError::Other(msg)) => Err(CoreError::Other(msg)),
        },
        #[cfg(feature = "x11")]
        Backend::X11 => serve_x11(offers, &mut signal),
        #[allow(unreachable_patterns)]
        _ => {
            let _ = (offers, &mut signal);
            Err(CoreError::Other("clipboard backend not compiled in".into()))
        }
    }
}

#[cfg(feature = "x11")]
fn serve_x11(offers: Vec<ClipOffer>, ready: &mut dyn FnMut()) -> Result<()> {
    super::x11::serve(offers, ready)
}

#[cfg(all(feature = "wayland", not(feature = "x11")))]
fn serve_x11(_offers: Vec<ClipOffer>, _ready: &mut dyn FnMut()) -> Result<()> {
    Err(CoreError::Other("X11 clipboard support not compiled in".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_round_trips_binary_data() {
        let offers = vec![
            ClipOffer {
                mime: "image/png".into(),
                data: vec![0, 1, 2, 255, 128],
            },
            ClipOffer {
                mime: "text/uri-list".into(),
                data: b"file:///a%20b\r\n".to_vec(),
            },
        ];
        let spec = ClipSpec::new(&offers);
        let json = serde_json::to_string(&spec).unwrap();
        let back: ClipSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(back.offers().unwrap(), offers);
    }

    #[test]
    fn backend_from_environment() {
        assert_eq!(detect_backend(Some("wayland-1"), Some(":0")), Some(Backend::Wayland));
        assert_eq!(detect_backend(Some(""), Some(":0")), Some(Backend::X11));
        assert_eq!(detect_backend(None, Some(":0")), Some(Backend::X11));
        assert_eq!(detect_backend(None, None), None);
        assert_eq!(detect_backend(Some(""), Some("")), None);
    }

    #[test]
    fn spawn_refuses_empty_offers() {
        let err = spawn_detached(Path::new("/bin/true"), &[], &[], Duration::from_secs(1)).unwrap_err();
        assert!(err.to_string().contains("no usable offers"));
    }
}
