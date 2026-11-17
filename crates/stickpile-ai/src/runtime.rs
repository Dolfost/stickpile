//! ONNX Runtime loading, shared by CLIP (feature `clip`) and OCR (feature
//! `ocr`): one library and one environment per process.
//!
//! ONNX Runtime is loaded once per process (`OnceLock`) from the given
//! library, else `ORT_DYLIB_PATH`, else `libonnxruntime.so` via the dynamic
//! loader. The first real load attempt fixes the outcome for the process (see
//! [`init_runtime`]); later loads reuse it. Failures, including the panics
//! `ort` raises for an unloadable library, surface as [`AiError::Model`].

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ort::environment::Environment;

use crate::AiError;

/// The loaded runtime. `ort` only keeps a weak reference to the environment
/// and every session needs it, so it lives here for the whole process.
pub(crate) struct Runtime {
    pub(crate) env: Environment,
    library: String,
}

static RUNTIME: OnceLock<Result<Runtime, String>> = OnceLock::new();

/// Loads ONNX Runtime and creates the global environment (once per process).
/// Returns the library that was used.
///
/// A path that does not exist is reported without touching ONNX Runtime, so
/// a later call with a corrected path still works. The outcome of an actual
/// load attempt (success or failure) is final for the process: ONNX Runtime
/// cannot be unloaded or swapped, so fixing a broken library needs a restart.
pub fn init_runtime(library: Option<&Path>) -> Result<&'static str, AiError> {
    runtime(library).map(|rt| rt.library.as_str())
}

pub(crate) fn runtime(library: Option<&Path>) -> Result<&'static Runtime, AiError> {
    let path: Option<PathBuf> = library
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("ORT_DYLIB_PATH").map(PathBuf::from));
    if RUNTIME.get().is_none() {
        if let Some(p) = &path {
            if !p.is_file() {
                return Err(AiError::Model(format!(
                    "ONNX Runtime library {} does not exist",
                    p.display()
                )));
            }
        }
    }
    let res = RUNTIME.get_or_init(|| {
        let shown = path
            .as_ref()
            .map_or_else(|| "libonnxruntime.so (system)".to_owned(), |p| p.display().to_string());
        // Without a path ort falls back to ORT_DYLIB_PATH and the loader
        // itself, and panics if that fails, hence `catch_panic`.
        let built = catch_panic(|| -> Result<Environment, String> {
            let builder = match &path {
                Some(p) => ort::init_from(p).map_err(|e| e.to_string())?,
                None => ort::init(),
            };
            builder
                .with_name("stickpile")
                .with_telemetry(false)
                .build()
                .map_err(|e| e.to_string())
        });
        match built {
            Ok(Ok(env)) => {
                tracing::info!(library = %shown, "ONNX Runtime loaded");
                Ok(Runtime { env, library: shown })
            }
            Ok(Err(e)) => Err(format!("ONNX Runtime ({shown}): {e}")),
            Err(panic) => Err(format!("cannot load ONNX Runtime ({shown}): {panic}")),
        }
    });
    match res {
        Ok(rt) => {
            if let Some(wanted) = library {
                if wanted.display().to_string() != rt.library {
                    tracing::warn!(wanted = %wanted.display(), loaded = %rt.library, "ONNX Runtime already loaded; keeping it");
                }
            }
            Ok(rt)
        }
        Err(e) => Err(AiError::Model(e.clone())),
    }
}

/// `ort` panics instead of returning errors in a few places (e.g. when the
/// dynamic library cannot be opened); a server must not go down for that.
pub(crate) fn catch_panic<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|p| {
        p.downcast_ref::<String>()
            .cloned()
            .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_else(|| "panic".to_owned())
    })
}

pub(crate) fn inference<E: std::fmt::Display>(e: E) -> AiError {
    AiError::Inference(e.to_string())
}
