//! Video ingest: probe, remux without metadata, representative frame.

use std::ffi::OsString;
use std::path::Path;

use image::RgbaImage;
use stickpile_api::{MediaFormat, MediaKind};

use crate::tools::{self, demuxer, file_arg, ProbeJson};
use crate::{raster, Limits, MediaError, Probe, Tools};

pub(crate) fn muxer(fmt: MediaFormat) -> &'static str {
    match fmt {
        MediaFormat::Webm => "webm",
        _ => "mp4",
    }
}

fn args(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

/// Input arguments for a stored video: forced demuxer, file protocol only.
/// VP9 with alpha is decoded with libvpx, the native decoder drops alpha.
pub(crate) fn input_args(fmt: MediaFormat, path: &Path, libvpx_alpha: bool) -> Vec<OsString> {
    let mut a = Vec::new();
    if libvpx_alpha {
        a.extend(args(&["-c:v", "libvpx-vp9"]));
    }
    a.extend(args(&["-f", demuxer(fmt), "-protocol_whitelist", "file", "-i"]));
    a.push(file_arg(path));
    a
}

pub(crate) fn needs_libvpx(fmt: MediaFormat, probe: &ProbeJson) -> bool {
    fmt == MediaFormat::Webm
        && probe
            .video()
            .is_some_and(|v| v.has_alpha() && v.codec_name.as_deref() == Some("vp9"))
}

pub(crate) struct VideoIngest {
    pub original: Vec<u8>,
    pub probe: Probe,
    pub frame: RgbaImage,
}

pub(crate) fn ingest(
    bytes: &[u8],
    fmt: MediaFormat,
    limits: &Limits,
    tools: &Tools,
) -> Result<VideoIngest, MediaError> {
    let dir = tools::scratch(tools)?;
    let ext = fmt.extension();
    let input = dir.path().join(format!("upload.{ext}"));
    std::fs::write(&input, bytes)?;
    let info = tools::ffprobe(tools, dir.path(), fmt, &input)?;
    let v = info
        .video()
        .ok_or_else(|| MediaError::Invalid("no video stream".into()))?;
    let (w, h) = (v.width.unwrap_or(0), v.height.unwrap_or(0));
    raster::check_pixels(w, h, limits)?;
    if let Some(ms) = info.duration_ms() {
        check_duration(ms, limits)?;
    }

    // Remux: first video stream, optional first audio stream, no metadata,
    // chapters, subtitles, data or attachments.
    let out = dir.path().join(format!("original.{ext}"));
    let mut a = input_args(fmt, &input, false);
    a.extend(args(&[
        "-map",
        "0:V:0",
        "-map",
        "0:a:0?",
        "-map_metadata",
        "-1",
        "-map_chapters",
        "-1",
        "-c",
        "copy",
        "-fflags",
        "+bitexact",
    ]));
    if fmt == MediaFormat::Mp4 {
        a.extend(args(&["-movflags", "+faststart"]));
    }
    // `-map_metadata -1` also drops the stream tag that makes the WebM muxer
    // write AlphaMode; without it players ignore the VP9 alpha layer.
    if fmt == MediaFormat::Webm && v.has_alpha() {
        a.extend(args(&["-metadata:s:v:0", "alpha_mode=1"]));
    }
    a.extend(args(&["-f", muxer(fmt), "-y"]));
    a.push(file_arg(&out));
    tools::ffmpeg(tools, dir.path(), a, None)?;

    let clean = tools::ffprobe(tools, dir.path(), fmt, &out)?;
    let cv = clean
        .video()
        .ok_or_else(|| MediaError::Invalid("no video stream after remux".into()))?;
    let duration_ms = clean
        .duration_ms()
        .or_else(|| info.duration_ms())
        .ok_or_else(|| MediaError::Invalid("unknown video duration".into()))?;
    check_duration(duration_ms, limits)?;
    let has_alpha = cv.has_alpha() || v.has_alpha();
    let fps = cv.fps().or_else(|| v.fps());
    let frame_count = cv.frames().or_else(|| v.frames());
    let frame = representative_frame(
        &out,
        fmt,
        duration_ms,
        needs_libvpx(fmt, &clean),
        limits,
        tools,
        dir.path(),
    )?;
    let original = std::fs::read(&out)?;
    let probe = Probe {
        format: fmt,
        kind: MediaKind::Video,
        width: w,
        height: h,
        frame_count,
        duration_ms: Some(duration_ms),
        fps,
        has_alpha,
    };
    Ok(VideoIngest { original, probe, frame })
}

fn check_duration(ms: u64, limits: &Limits) -> Result<(), MediaError> {
    if ms > limits.max_video_ms {
        return Err(MediaError::TooLarge(format!(
            "video is {} ms, limit {} ms",
            ms, limits.max_video_ms
        )));
    }
    Ok(())
}

/// One PNG frame at ~10 % of the duration (falls back to the first frame).
fn representative_frame(
    video: &Path,
    fmt: MediaFormat,
    duration_ms: u64,
    libvpx_alpha: bool,
    limits: &Limits,
    tools: &Tools,
    dir: &Path,
) -> Result<RgbaImage, MediaError> {
    let png = dir.join("frame.png");
    let at = duration_ms / 10;
    for seek in [at, 0] {
        let _ = std::fs::remove_file(&png);
        let mut a = args(&["-ss", &format!("{:.3}", seek as f64 / 1000.0)]);
        a.extend(input_args(fmt, video, libvpx_alpha));
        a.extend(args(&[
            "-map",
            "0:V:0",
            "-frames:v",
            "1",
            "-an",
            "-sn",
            "-dn",
            "-c:v",
            "png",
            "-f",
            "image2",
            "-update",
            "1",
            "-y",
        ]));
        a.push(file_arg(&png));
        tools::ffmpeg(tools, dir, a, None)?;
        if let Ok(bytes) = std::fs::read(&png) {
            if !bytes.is_empty() {
                return Ok(raster::decode_still(&bytes, MediaFormat::Png, limits)?.to_rgba8());
            }
        }
        if seek == 0 {
            break;
        }
    }
    Err(MediaError::Invalid("video has no decodable frame".into()))
}
