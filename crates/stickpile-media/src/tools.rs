//! Running ffmpeg/ffprobe: argument arrays only (no shell), a cleared
//! environment (PATH only), stdin closed or fed by us, a hard timeout, and
//! stderr captured into the error.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;
use stickpile_api::MediaFormat;

use crate::{MediaError, Tools};

const MAX_STDOUT: usize = 8 << 20;
const MAX_STDERR_KEPT: usize = 16 << 10;
const MAX_STDERR_REPORTED: usize = 1500;

/// Common leading arguments for every ffmpeg call.
pub(crate) const FFMPEG_PRELUDE: &[&str] = &["-nostdin", "-hide_banner", "-loglevel", "error"];

impl Tools {
    /// Tools from `STICKPILE_FFMPEG` / `STICKPILE_FFPROBE`, falling back to
    /// `ffmpeg` / `ffprobe` looked up on `PATH`.
    pub fn from_env(tmp_dir: impl Into<PathBuf>, timeout: Duration) -> Tools {
        let pick = |var: &str, default: &str| {
            std::env::var_os(var)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| default.into())
        };
        Tools {
            ffmpeg: pick("STICKPILE_FFMPEG", "ffmpeg"),
            ffprobe: pick("STICKPILE_FFPROBE", "ffprobe"),
            timeout,
            tmp_dir: tmp_dir.into(),
        }
    }
}

/// A scratch directory inside `tools.tmp_dir`, removed on drop.
pub(crate) fn scratch(tools: &Tools) -> Result<tempfile::TempDir, MediaError> {
    std::fs::create_dir_all(&tools.tmp_dir)?;
    Ok(tempfile::Builder::new()
        .prefix("stickpile-media-")
        .tempdir_in(&tools.tmp_dir)?)
}

/// `file:` URL for a server-generated path (never interpreted as another
/// protocol or an image2 pattern).
pub(crate) fn file_arg(p: &Path) -> OsString {
    let mut s = OsString::from("file:");
    s.push(p.as_os_str());
    s
}

/// Demuxer name to force for a stored original.
pub(crate) fn demuxer(fmt: MediaFormat) -> &'static str {
    match fmt {
        MediaFormat::Webm => "matroska",
        _ => "mov",
    }
}

fn read_capped(mut r: impl Read, keep: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        match r.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = keep.saturating_sub(out.len());
                out.extend_from_slice(&buf[..n.min(room)]);
            }
        }
    }
    out
}

fn stderr_text(raw: &[u8]) -> String {
    let s = String::from_utf8_lossy(raw);
    let s = s.trim();
    if s.len() <= MAX_STDERR_REPORTED {
        return s.to_string();
    }
    let mut cut = MAX_STDERR_REPORTED;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &s[..cut])
}

fn wait_with_deadline(child: &mut Child, timeout: Duration) -> io::Result<Option<std::process::ExitStatus>> {
    let deadline = Instant::now() + timeout;
    let mut nap = Duration::from_millis(1);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(nap);
        nap = (nap * 2).min(Duration::from_millis(25));
    }
}

/// Feeds the child's stdin (raw frames). Returning an error stops the feed.
pub(crate) type Feeder<'a> = &'a mut (dyn FnMut(&mut ChildStdin) -> Result<(), MediaError> + Send);

/// Run `program args...` with `cwd` as working directory. Returns stdout.
pub(crate) fn run(
    program: &Path,
    args: &[OsString],
    cwd: &Path,
    timeout: Duration,
    feed: Option<Feeder<'_>>,
) -> Result<Vec<u8>, MediaError> {
    let name = program
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "tool".into());
    let mut cmd = Command::new(program);
    cmd.args(args).current_dir(cwd).env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        cmd.env("PATH", path);
    }
    cmd.stdin(if feed.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| MediaError::Ffmpeg(format!("cannot run {}: {e}", program.display())))?;
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    let stdin = child.stdin.take();

    std::thread::scope(|s| {
        let out_h = s.spawn(move || read_capped(stdout, MAX_STDOUT));
        let err_h = s.spawn(move || read_capped(stderr, MAX_STDERR_KEPT));
        let feed_h = match (stdin, feed) {
            (Some(mut pipe), Some(feed)) => Some(s.spawn(move || {
                let r = feed(&mut pipe);
                drop(pipe);
                r
            })),
            _ => None,
        };
        let status = wait_with_deadline(&mut child, timeout);
        let stdout = out_h.join().unwrap_or_default();
        let stderr = err_h.join().unwrap_or_default();
        let fed = feed_h.map(|h| {
            h.join()
                .unwrap_or_else(|_| Err(MediaError::Ffmpeg("frame feeder panicked".into())))
        });
        match status? {
            None => Err(MediaError::Ffmpeg(format!("{name} timed out after {:?}", timeout))),
            Some(st) if !st.success() => {
                // A feeder error that caused the failure is more useful than "broken pipe".
                if let Some(Err(e)) = fed {
                    if !is_broken_pipe(&e) {
                        return Err(e);
                    }
                }
                Err(MediaError::Ffmpeg(format!(
                    "{name} failed ({st}): {}",
                    stderr_text(&stderr)
                )))
            }
            Some(_) => match fed {
                // ffmpeg may stop reading early (e.g. `-t 3`): a broken pipe is fine.
                Some(Err(e)) if !is_broken_pipe(&e) => Err(e),
                _ => Ok(stdout),
            },
        }
    })
}

fn is_broken_pipe(e: &MediaError) -> bool {
    matches!(e, MediaError::Io(io) if io.kind() == io::ErrorKind::BrokenPipe)
}

/// Run ffmpeg with the fixed prelude prepended.
pub(crate) fn ffmpeg(
    tools: &Tools,
    cwd: &Path,
    args: Vec<OsString>,
    feed: Option<Feeder<'_>>,
) -> Result<(), MediaError> {
    let mut full: Vec<OsString> = FFMPEG_PRELUDE.iter().map(OsString::from).collect();
    full.extend(args);
    run(&tools.ffmpeg, &full, cwd, tools.timeout, feed).map(|_| ())
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ProbeJson {
    #[serde(default)]
    pub streams: Vec<StreamJson>,
    #[serde(default)]
    pub format: FormatJson,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct StreamJson {
    #[serde(default)]
    pub codec_type: Option<String>,
    #[serde(default)]
    pub codec_name: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub pix_fmt: Option<String>,
    #[serde(default)]
    pub avg_frame_rate: Option<String>,
    #[serde(default)]
    pub r_frame_rate: Option<String>,
    #[serde(default)]
    pub nb_frames: Option<String>,
    #[serde(default)]
    pub duration: Option<String>,
    #[serde(default)]
    pub disposition: HashMap<String, i64>,
    #[serde(default)]
    pub tags: HashMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct FormatJson {
    #[serde(default)]
    pub duration: Option<String>,
}

fn rate(s: &Option<String>) -> Option<f32> {
    let s = s.as_deref()?;
    let (n, d) = s.split_once('/').unwrap_or((s, "1"));
    let (n, d) = (n.parse::<f64>().ok()?, d.parse::<f64>().ok()?);
    (d > 0.0 && n > 0.0).then(|| (n / d) as f32)
}

fn seconds_to_ms(s: &Option<String>) -> Option<u64> {
    let v = s.as_deref()?.parse::<f64>().ok()?;
    (v.is_finite() && v >= 0.0).then(|| (v * 1000.0).round() as u64)
}

impl StreamJson {
    pub fn is_video(&self) -> bool {
        self.codec_type.as_deref() == Some("video") && self.disposition.get("attached_pic").copied().unwrap_or(0) == 0
    }
    pub fn fps(&self) -> Option<f32> {
        rate(&self.avg_frame_rate).or_else(|| rate(&self.r_frame_rate))
    }
    pub fn has_alpha(&self) -> bool {
        let fmt_alpha = self.pix_fmt.as_deref().is_some_and(|p| {
            p.starts_with("yuva")
                || p.starts_with("rgba")
                || p.starts_with("bgra")
                || p.starts_with("argb")
                || p.starts_with("abgr")
        });
        let tag_alpha = self
            .tags
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("alpha_mode") && v.trim() == "1");
        fmt_alpha || tag_alpha
    }
    pub fn frames(&self) -> Option<u32> {
        self.nb_frames.as_deref()?.parse().ok()
    }
}

impl ProbeJson {
    pub fn video(&self) -> Option<&StreamJson> {
        self.streams.iter().find(|s| s.is_video())
    }
    pub fn duration_ms(&self) -> Option<u64> {
        seconds_to_ms(&self.format.duration).or_else(|| self.video().and_then(|v| seconds_to_ms(&v.duration)))
    }
}

/// `ffprobe` a server-generated file with the demuxer forced.
pub(crate) fn ffprobe(tools: &Tools, cwd: &Path, fmt: MediaFormat, path: &Path) -> Result<ProbeJson, MediaError> {
    let args: Vec<OsString> = [
        "-v",
        "error",
        "-hide_banner",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
        "-f",
        demuxer(fmt),
        "-protocol_whitelist",
        "file",
    ]
    .iter()
    .map(OsString::from)
    .chain(std::iter::once(file_arg(path)))
    .collect();
    let out = run(&tools.ffprobe, &args, cwd, tools.timeout, None)?;
    serde_json::from_slice(&out).map_err(|e| MediaError::Ffmpeg(format!("ffprobe output: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn timeout_kills_the_child() {
        let dir = tempfile::tempdir().unwrap();
        let start = Instant::now();
        let err = run(
            Path::new("sleep"),
            &["5".into()],
            dir.path(),
            Duration::from_millis(200),
            None,
        )
        .unwrap_err();
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(
            matches!(err, MediaError::Ffmpeg(ref m) if m.contains("timed out")),
            "{err}"
        );
    }

    #[test]
    fn failure_carries_stderr_and_env_is_cleared() {
        let dir = tempfile::tempdir().unwrap();
        // `env` prints the environment it got: PATH only.
        let out = run(Path::new("env"), &[], dir.path(), Duration::from_secs(5), None).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.lines().all(|l| l.starts_with("PATH=")), "{text}");
        let err = run(
            Path::new("ls"),
            &["/definitely/not/here".into()],
            dir.path(),
            Duration::from_secs(5),
            None,
        )
        .unwrap_err();
        assert!(
            matches!(err, MediaError::Ffmpeg(ref m) if m.contains("not/here")),
            "{err}"
        );
        let err = run(
            Path::new("/no/such/tool"),
            &[],
            dir.path(),
            Duration::from_secs(5),
            None,
        )
        .unwrap_err();
        assert!(
            matches!(err, MediaError::Ffmpeg(ref m) if m.contains("cannot run")),
            "{err}"
        );
    }

    #[test]
    fn feeds_stdin() {
        let dir = tempfile::tempdir().unwrap();
        let mut feed = |pipe: &mut ChildStdin| pipe.write_all(b"hello through the pipe").map_err(MediaError::Io);
        let out = run(
            Path::new("cat"),
            &[],
            dir.path(),
            Duration::from_secs(5),
            Some(&mut feed),
        )
        .unwrap();
        assert_eq!(out, b"hello through the pipe");
    }

    #[test]
    fn parses_probe_json() {
        let j = r#"{"streams":[
            {"codec_type":"video","codec_name":"mjpeg","disposition":{"attached_pic":1}},
            {"codec_type":"video","codec_name":"vp9","width":320,"height":240,"pix_fmt":"yuv420p",
             "avg_frame_rate":"30000/1001","tags":{"ALPHA_MODE":"1"}},
            {"codec_type":"audio","codec_name":"opus"}],
            "format":{"duration":"2.500000"}}"#;
        let p: ProbeJson = serde_json::from_str(j).unwrap();
        let v = p.video().unwrap();
        assert_eq!(v.codec_name.as_deref(), Some("vp9"));
        assert!(v.has_alpha());
        assert!((v.fps().unwrap() - 29.97).abs() < 0.01);
        assert_eq!(p.duration_ms(), Some(2500));
    }
}
