//! A broken ONNX Runtime setup must surface as `AiError::Model`, never as a
//! panic or abort. Model-free: runs in the default test suite. Kept in its own
//! test binary because the runtime is loaded once per process.
#![cfg(feature = "clip")]

use stickpile_ai::clip::ClipEnricher;
use stickpile_ai::{layout, AiError, ClipOptions, TextEncoderKind};

#[test]
fn bad_runtime_library_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    // Placeholder model files: the runtime is loaded before any model is read.
    for rel in TextEncoderKind::Clip.required_files() {
        let p = dir.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"not a model").unwrap();
    }
    let opts = |lib: std::path::PathBuf| ClipOptions {
        model_dir: dir.path().into(),
        text_encoder: TextEncoderKind::Clip,
        onnxruntime_library: Some(lib),
        threads: 1,
    };

    // Missing library: reported, and not cached.
    let missing = dir.path().join("nope/libonnxruntime.so");
    let err = ClipEnricher::load(&opts(missing.clone())).unwrap_err();
    assert!(
        matches!(err, AiError::Model(ref m) if m.contains("does not exist")),
        "{err}"
    );

    // A file that is not a shared library: ort panics internally; we get an error.
    let bogus = dir.path().join("libonnxruntime.so");
    std::fs::write(&bogus, b"definitely not ELF").unwrap();
    let err = ClipEnricher::load(&opts(bogus.clone())).unwrap_err();
    assert!(
        matches!(err, AiError::Model(ref m) if m.contains("ONNX Runtime")),
        "{err}"
    );
    // The failed load is final for this process and keeps failing cleanly.
    assert!(ClipEnricher::load(&opts(bogus)).is_err());
    assert!(stickpile_ai::clip::init_runtime(None).is_err());
    let _ = layout::VISION;
}
