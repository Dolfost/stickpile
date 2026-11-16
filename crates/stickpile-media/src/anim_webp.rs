//! Animated WebP through libwebp's `WebPAnimEncoder`, one frame at a time.
//!
//! The `webp` crate's `AnimEncoder` keeps every raw frame (and a converted
//! copy) until the end; for a 10 s, 512x512 sticker that is hundreds of
//! megabytes. libwebp itself copies each frame when it is added and keeps
//! only what it needs to diff the next one, so this wrapper adds frames as
//! they are produced and frees each picture right away.

use std::ffi::{c_int, CStr};

use libwebp_sys as sys;

use crate::MediaError;

fn failed(what: &str) -> MediaError {
    MediaError::Invalid(format!("webp animation: {what}"))
}

/// An animated WebP being encoded: lossy frames of `width` x `height`,
/// looping forever, transparent background.
pub(crate) struct AnimWriter {
    enc: *mut sys::WebPAnimEncoder,
    config: sys::WebPConfig,
    width: u32,
    height: u32,
    /// Timestamp of the next frame (ms).
    next_ms: u64,
    frames: u32,
}

impl AnimWriter {
    /// A new encoder for lossy frames at `quality` (0 to 100).
    pub(crate) fn new(width: u32, height: u32, quality: f32) -> Result<AnimWriter, MediaError> {
        if width == 0 || height == 0 || width > 16383 || height > 16383 {
            return Err(failed("bad canvas size"));
        }
        let mut config = sys::WebPConfig::new().map_err(|()| failed("bad libwebp version"))?;
        config.lossless = 0;
        config.quality = quality.clamp(0.0, 100.0);
        // SAFETY: `config` is an initialised WebPConfig.
        if unsafe { sys::WebPValidateConfig(&config) } == 0 {
            return Err(failed("invalid encoder settings"));
        }
        let mut opts = std::mem::MaybeUninit::<sys::WebPAnimEncoderOptions>::zeroed();
        // SAFETY: the pointer is valid for writes of the options struct.
        let ok =
            unsafe { sys::WebPAnimEncoderOptionsInitInternal(opts.as_mut_ptr(), sys::WEBP_MUX_ABI_VERSION as c_int) };
        if ok == 0 {
            return Err(failed("bad libwebp mux version"));
        }
        // SAFETY: initialised by the call above.
        let mut opts = unsafe { opts.assume_init() };
        opts.anim_params.loop_count = 0;
        opts.anim_params.bgcolor = 0;
        opts.allow_mixed = 0;
        opts.minimize_size = 0;
        // SAFETY: valid options; the result is checked for null.
        let enc = unsafe {
            sys::WebPAnimEncoderNewInternal(
                width as c_int,
                height as c_int,
                &opts,
                sys::WEBP_MUX_ABI_VERSION as c_int,
            )
        };
        if enc.is_null() {
            return Err(failed("cannot create the encoder"));
        }
        Ok(AnimWriter {
            enc,
            config,
            width,
            height,
            next_ms: 0,
            frames: 0,
        })
    }

    fn error(&self) -> MediaError {
        // SAFETY: `enc` is live; the message is a NUL-terminated string
        // owned by the encoder.
        let msg = unsafe {
            let p = sys::WebPAnimEncoderGetError(self.enc);
            if p.is_null() {
                "unknown error".to_string()
            } else {
                CStr::from_ptr(p).to_string_lossy().into_owned()
            }
        };
        failed(&msg)
    }

    /// Add a frame of RGBA pixels shown for `duration_ms`.
    pub(crate) fn add(&mut self, rgba: &[u8], duration_ms: u32) -> Result<(), MediaError> {
        let stride = self.width as usize * 4;
        if rgba.len() != stride * self.height as usize {
            return Err(failed("frame size differs from the canvas"));
        }
        let timestamp = c_int::try_from(self.next_ms).map_err(|_| failed("animation too long"))?;
        let mut pic = sys::WebPPicture::new().map_err(|()| failed("bad libwebp version"))?;
        pic.use_argb = 1;
        pic.width = self.width as c_int;
        pic.height = self.height as c_int;
        // SAFETY: `rgba` holds width*height*4 bytes with the given stride;
        // the picture is freed below whatever happens. WebPAnimEncoderAdd
        // copies the frame, so nothing refers to `pic` afterwards.
        unsafe {
            if sys::WebPPictureImportRGBA(&mut pic, rgba.as_ptr(), stride as c_int) == 0 {
                sys::WebPPictureFree(&mut pic);
                return Err(failed("out of memory"));
            }
            let ok = sys::WebPAnimEncoderAdd(self.enc, &mut pic, timestamp, &self.config);
            sys::WebPPictureFree(&mut pic);
            if ok == 0 {
                return Err(self.error());
            }
        }
        self.next_ms += u64::from(duration_ms.max(1));
        self.frames += 1;
        Ok(())
    }

    /// The finished file. The last frame lasts its own duration (the
    /// closing timestamp is the end of the animation).
    pub(crate) fn finish(self) -> Result<Vec<u8>, MediaError> {
        if self.frames == 0 {
            return Err(failed("no frames"));
        }
        let end = c_int::try_from(self.next_ms).map_err(|_| failed("animation too long"))?;
        // SAFETY: `enc` is live until `self` drops; `data` is freed with
        // WebPDataClear after copying it out.
        unsafe {
            if sys::WebPAnimEncoderAdd(self.enc, std::ptr::null_mut(), end, std::ptr::null()) == 0 {
                return Err(self.error());
            }
            let mut data = sys::WebPData::default();
            if sys::WebPAnimEncoderAssemble(self.enc, &mut data) == 0 {
                return Err(self.error());
            }
            let out = if data.bytes.is_null() {
                Vec::new()
            } else {
                std::slice::from_raw_parts(data.bytes, data.size).to_vec()
            };
            sys::WebPDataClear(&mut data);
            if out.is_empty() {
                return Err(failed("empty output"));
            }
            Ok(out)
        }
    }
}

impl Drop for AnimWriter {
    fn drop(&mut self) {
        // SAFETY: created by WebPAnimEncoderNewInternal and deleted once.
        unsafe { sys::WebPAnimEncoderDelete(self.enc) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: u32, h: u32, shade: u8) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let a = if x < w / 4 { 0 } else { 255 };
                v.extend_from_slice(&[shade, (x * 3) as u8, (y * 5) as u8, a]);
            }
        }
        v
    }

    #[test]
    fn encodes_frames_with_their_durations() {
        let mut w = AnimWriter::new(64, 48, 70.0).unwrap();
        for (i, d) in [100u32, 40, 250].into_iter().enumerate() {
            w.add(&frame(64, 48, i as u8 * 80), d).unwrap();
        }
        assert_eq!(w.frames, 3);
        let out = w.finish().unwrap();
        let (_, layout) = crate::webp::rebuild(&out).unwrap();
        assert!(layout.animated);
        assert_eq!(layout.durations, vec![100, 40, 250]);
        let dec = image::codecs::webp::WebPDecoder::new(std::io::Cursor::new(&out)).unwrap();
        assert_eq!(image::ImageDecoder::dimensions(&dec), (64, 48));
    }

    #[test]
    fn rejects_a_wrong_frame_size() {
        let mut w = AnimWriter::new(8, 8, 50.0).unwrap();
        assert!(w.add(&[0; 10], 100).is_err());
        assert!(AnimWriter::new(8, 8, 50.0).unwrap().finish().is_err());
    }
}
