# Media pipeline (`stickpile-media`)

`stickpile-media` turns an uploaded file into a stored media item: it
identifies the format, validates and sanitises the file, computes the
perceptual hashes and the quality score, and renders the variants clients
paste into chat apps. The library is blocking and synchronous; the server
calls it from `spawn_blocking`.

Public API (see `crates/stickpile-media/src/lib.rs`):

| Function | Purpose |
|---|---|
| `sniff(bytes)` | Format from magic bytes, or `None`. |
| `ingest(bytes, &IngestOptions, &Tools)` | Validate, sanitise and analyse an upload. Returns the sanitised original, `Probe`, sha256, pHash, dHash, quality report, thumbnail and the representative frame. |
| `generate_variants(path, &Probe, &VariantOptions, &Tools)` | All renditions except `Original` and `Thumb`, sorted by `Variant`. |
| `decode_frame(bytes, &Limits)` | First frame of a still or animation as RGBA (used to re-process Telegram stickers). |
| `Tools::from_env(tmp_dir, timeout)` | `STICKPILE_FFMPEG` / `STICKPILE_FFPROBE`, or `ffmpeg` / `ffprobe` on `PATH`. |
| `hash::{phash, dhash, hamming, to_hex, from_hex}` | 64-bit perceptual hashes. |
| `quality::{assess, jpeg_quality_estimate, total}` | Quality score, see [quality.md](quality.md). |

Errors: `Unsupported` (not a format we accept), `TooLarge` (a limit was
hit), `Invalid` (the file does not decode or is structurally broken),
`Ffmpeg` (ffmpeg/ffprobe failed, timed out or could not be started; the
message carries the first 1500 characters of its stderr), `Io`.

## 1. Sniffing

Only the leading bytes count; the file name and the client's MIME type are
ignored.

| Format | Rule |
|---|---|
| PNG | `89 50 4E 47 0D 0A 1A 0A` |
| JPEG | `FF D8 FF` |
| GIF | `GIF87a` or `GIF89a` |
| WebP | `RIFF` .... `WEBP` |
| MP4 | ISO-BMFF `ftyp` box at offset 4 whose major or compatible brands include a video brand (`isom`, `iso2`–`iso9`, `mp41`, `mp42`, `avc1`, `dash`, `M4V `, `M4VH`, `M4VP`, `mmp4`, `MSNV`, `qt  `, `3gp4`–`3gp6`, `3g2a`) and **no** HEIF/AVIF brand (`heic`, `heix`, `hevc`, `hevx`, `heim`, `heis`, `hevm`, `hevs`, `mif1`, `mif2`, `msf1`, `avif`, `avis`, `avio`, `MiHE`, `MiHA`, `MiHB`, `miaf`). QuickTime `.mov` (`qt  `) is accepted and remuxed into MP4. Audio-only files with a video-compatible brand pass the sniff and are rejected at ingest (no video stream). |
| WebM | EBML header `1A 45 DF A3` whose `DocType` element is `webm`. Generic Matroska (`matroska`) is rejected. |

## 2. Ingest

Order of operations:

1. `limits.max_bytes` is checked before anything else.
2. sha256 of the bytes **as uploaded** (lowercase hex): the dedup key.
3. Sniff; unknown formats fail with `Unsupported`.
4. Format-specific sanitising (below). Every still and animation is fully
   decoded; the dimensions are read from the header first and checked
   against `max_pixels` before any pixel buffer is allocated.
5. pHash and dHash of the representative frame (the image; the first frame
   of an animation; the frame at about 10 % of a video).
6. `quality::assess` on the uploaded bytes and the representative frame.
7. Thumbnail (`Variant::Thumb`): the representative frame bounded to
   `thumb_px` on the longest side (never upscaled), lossy WebP q=80, alpha
   kept (RGB only when the frame is opaque).

### Limits

| Limit | Applies to |
|---|---|
| `max_bytes` | the upload, and `decode_frame` input |
| `max_pixels` | width × height of the image, animation canvas, every GIF frame, and video frames (from ffprobe) |
| `max_frames` | animations (GIF, animated WebP); not videos, which are bounded by duration |
| `max_video_ms` | videos, checked on the input probe when it reports a duration and always on the remuxed file |

There is also a fixed decode budget for animations: the sum of all decoded
frame areas may not exceed `100 × max_pixels`. It bounds CPU time for LZW
and VP8 "bombs" that are small on disk.

### Sanitising rules

The stored original never contains bytes that were not either re-encoded by
us or copied from a whitelisted, structurally validated part of the input.
Metadata, comments, unknown chunks and anything after the logical end of
the file (ZIP/HTML polyglots) are dropped. The tests append ZIP and HTML
payloads to every format and assert they are gone.

| Format | Rule | Lossless? |
|---|---|---|
| PNG | Decoded with the `png` crate (no transformations) and re-encoded with the same colour type, bit depth, palette and `tRNS`. Everything else is dropped: text chunks, `eXIf`, `iCCP`, `gAMA`/`cHRM`/`sRGB`, `tIME`, `pHYs`, APNG frames (only the default image is kept), private chunks. | yes (pixels identical) |
| GIF | Decoded frame by frame with the `gif` crate as indexed pixels and re-encoded: global and local palettes, transparency index, disposal, delay, offsets, "user input" flag and the NETSCAPE loop count are kept. Comment, XMP and other application extensions are dropped. Interlaced frames are written de-interlaced. | yes (identical composited frames) |
| JPEG | Split into marker segments; after a successful full decode of the *rebuilt* file it consists of SOI, DQT, DHT, DAC, SOFn, DRI, SOS + entropy-coded data, EOI, in their original order. All APPn (JFIF, EXIF, XMP, ICC, Photoshop/IPTC, …) and COM segments, bytes between segments and everything after EOI are dropped. Two synthetic segments without user data are written when needed so the image still renders the same: a canonical Adobe APP14 carrying only the original colour-transform byte, and a 32-byte EXIF block holding only the Orientation tag (when it is not 1). | yes (entropy data copied, no new generation) |
| WebP | RIFF chunks are parsed and the file is rebuilt from `VP8 `, `VP8L`, `VP8X`, `ALPH`, `ANIM`, `ANMF` only (inside `ANMF` only `ALPH`, `VP8 `, `VP8L`). The ICC, EXIF and XMP flags and the reserved bits of `VP8X` are cleared, every size field is recomputed, the RIFF size is fixed, trailing bytes are dropped. Then every frame of the rebuilt file is decoded. | yes |
| MP4 / WebM | Written to a fresh scratch directory and probed with ffprobe (one video stream that is not cover art is required; dimensions and duration checked), then remuxed: `-map 0:V:0 -map 0:a:0? -map_metadata -1 -map_chapters -1 -c copy -fflags +bitexact` (+ `-movflags +faststart` for MP4), output format forced (`mp4`/`webm`). Subtitles, data tracks, attachments, chapters, global and stream metadata are gone. For WebM with VP9 alpha the single tag `alpha_mode=1` is set again, because `-map_metadata -1` would otherwise stop the muxer from flagging the alpha layer. | yes (stream copy) |

Notes:

- The decoded result is what gets validated: PNG, GIF and WebP are decoded
  after rebuilding, so the stored bytes are known to decode.
- JPEG EXIF orientation is applied to the representative frame (thumbnail,
  hashes, `Probe` dimensions) and kept in the minimal EXIF block. PNG
  `eXIf` and WebP `EXIF` orientation are dropped like all other metadata.
- Dropping ICC profiles means wide-gamut images are shown as sRGB. This is
  intentional: profiles are a common carrier for hidden data and stickers
  are sRGB in practice.
- A JPEG that ends without EOI is accepted if it decodes (the rebuilt file
  gets an EOI); truncated headers are rejected.

### Probe

| Field | Still | Animation | Video |
|---|---|---|---|
| `kind` | `Still` (also single-frame GIF/WebP) | `Animation` (> 1 frame) | `Video` |
| `width`, `height` | after EXIF orientation | canvas | first video stream |
| `frame_count` | `None` | frames | `nb_frames` when the container has it (MP4), else `None` |
| `duration_ms` | `None` | sum of frame delays | container duration |
| `fps` | `None` | frames / duration | `avg_frame_rate` |
| `has_alpha` | PNG RGBA/GA or `tRNS`; WebP alpha; GIF transparency index | GIF transparency index; WebP alpha | `yuva*`/`rgba*` pixel format or WebM `alpha_mode=1` |

Frame delays of 10 ms or less count as 100 ms, as in browsers.

## 3. Variants

`generate_variants` reads the stored sanitised original and returns the
renditions that apply, sorted by `Variant` (`Png`, `TgSticker`,
`TgVideoSticker`, `DiscordGif`, `Mp4`). A rendition that cannot meet its
constraint is skipped with a `tracing` warning; an ffmpeg failure is an
error. Telegram limits come from `stickpile_api::profiles`.

| Variant | Kind | When | Constraints / encoding |
|---|---|---|---|
| `Png` | still | original is not PNG | Lossless PNG of the (oriented) frame, RGB when opaque, RGBA otherwise. |
| `TgSticker` | still | always | Longest side exactly 512 (up- or downscaled, Lanczos3), lossy WebP via libwebp with alpha kept; quality 95, else the highest quality that fits ≤ 512 KiB (binary search). |
| `Mp4` | animation | always | H.264 (libx264, preset medium, CRF 20), `yuv420p`, even dimensions (odd pixel cropped; longest side capped at 1920), frame rate from the shortest frame delay capped at 50 fps, no audio, `+faststart`. Transparency is **flattened on white**. |
| `Mp4` | video | original is WebM, or not 8-bit 4:2:0 H.264 | Same encoding as above from the video; VP9 alpha is decoded with libvpx and flattened on white. |
| `DiscordGif` | animation | unless the original is a GIF within `discord_gif_max_bytes` | ffmpeg `palettegen` (256 colours, `stats_mode=diff`, transparent slot when the source has alpha) + `paletteuse` (Bayer dither, `diff_mode=rectangle`, alpha threshold 128), infinite loop. Tried at (longest side, fps cap) = (640, 30), (480, 25), (400, 20), (320, 15), (240, 12), (160, 10), never upscaling, until the file fits the limit. |
| `DiscordGif` | video | duration ≤ 10 s | Same ladder from the video. |
| `TgVideoSticker` | animation | always | VP9 WebM (libvpx-vp9), longest side exactly 512 (other side even), first 3 s, ≤ 30 fps, no audio, `yuva420p` when the source has alpha. Bitrate search: start at 85 % of 256 KiB × 8 / duration, re-encode with the bitrate scaled by limit/size × 0.85 up to 6 times; skipped with a warning if it never fits 256 KiB. |
| `TgVideoSticker` | video | duration ≤ 10 s | Same from the first 3 s of the video (`-t 3`, `fps=30` only when the source is faster). |

Animations are decoded in Rust (ffmpeg 8.1 cannot decode animated WebP),
resampled to a constant frame rate by repeating or dropping frames at the
output timestamps, and streamed to ffmpeg as raw video on stdin. Videos are
read by ffmpeg from a copy of the stored original inside `tmp_dir`.

## 4. ffmpeg / ffprobe invocation policy

- `std::process::Command` with an argument array; never a shell. Every
  argument is a fixed string or a number we computed (dimensions, frame
  rate, bitrate, seek time). Nothing from the upload (names, metadata) is
  ever an argument.
- Every ffmpeg call starts with `-nostdin -hide_banner -loglevel error`;
  ffprobe runs with `-v error`.
- Inputs: the demuxer is forced (`-f mov` for MP4, `-f matroska` for WebM,
  `-f rawvideo` for raw frames) so ffmpeg never probes for another format
  (HLS playlists, concat lists, image sequences). File inputs use
  `-protocol_whitelist file` and a `file:` URL; the raw-frame input uses
  `-protocol_whitelist pipe` and `pipe:0`. The MOV demuxer's external data
  references stay disabled (ffmpeg default).
- Paths: only files we created in a fresh `tempfile` directory under
  `tools.tmp_dir` (removed afterwards), passed as `file:` URLs so no part of
  the path can be read as a protocol or an image2 pattern. The working
  directory is that scratch directory.
- Outputs: the muxer is forced (`-f mp4`, `-f webm`, `-f gif`, `-f image2`
  with `-update 1`), `-map_metadata -1`, `-fflags +bitexact`.
- Environment: `env_clear()`, then only `PATH`.
- stdin is `/dev/null` (or our raw-frame pipe), stdout and stderr are
  captured on separate threads; stderr is truncated into
  `MediaError::Ffmpeg`.
- Timeout: `tools.timeout` per invocation; the process is killed when it
  expires. Variant generation runs several invocations (bitrate and size
  searches), so a job may take a multiple of the timeout.

## 5. Runtime dependencies

- `ffmpeg` and `ffprobe` (tested with ffmpeg 8.1 `ffmpeg-headless`) built
  with `libx264`, `libvpx` (VP9 encoder and decoder), the `gif`, `png`,
  `mov`/`mp4`, `matroska`/`webm`, `rawvideo` and `image2` (de)muxers, and
  the `palettegen`, `paletteuse`, `scale`, `crop`, `fps`, `lutrgb`,
  `overlay`, `format`, `setsar` and `split` filters. Only needed for videos
  and animated variants; stills never start a process.
- libwebp is compiled into the binary (`libwebp-sys`, bundled).
- The test suite (`cargo test -p stickpile-media`) needs `ffmpeg`/`ffprobe`
  on `PATH` (or `STICKPILE_FFMPEG`/`STICKPILE_FFPROBE`), plus `sleep`, `env`,
  `ls` and `cat` for the process-runner tests. All fixtures are generated at
  run time; no network.
