//! API token file: one line with the bearer token, mode 0600.
//!
//! The file is created with mode 0600 in the same `open(2)` call (never
//! chmod-ed after the fact), written to a temporary name and renamed into
//! place, so a reader never sees a partial or world-readable token. Reading
//! refuses a token file that is accessible by group or others.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::{CoreError, Result};

/// Read the token. `Ok(None)` if the file does not exist or is empty.
pub fn read_token(path: &Path) -> Result<Option<String>> {
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(CoreError::Other(format!("token file {}: {e}", path.display()))),
    };
    check_permissions(path, &meta)?;
    let text = fs::read_to_string(path).map_err(|e| CoreError::Other(format!("token file {}: {e}", path.display())))?;
    let token = text.trim();
    Ok((!token.is_empty()).then(|| token.to_string()))
}

#[cfg(unix)]
fn check_permissions(path: &Path, meta: &fs::Metadata) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(CoreError::Other(format!(
            "token file {} is accessible by group or others (mode {mode:04o}); refusing to use it. \
             Fix with `chmod 600 {}` or log in again",
            path.display(),
            path.display()
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_permissions(_path: &Path, _meta: &fs::Metadata) -> Result<()> {
    Ok(())
}

/// Write the token atomically with mode 0600, creating parent directories (0700).
pub fn write_token(path: &Path, token: &str) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    create_private_dir(parent)?;
    let tmp = tmp_name(path);
    let result = (|| -> std::io::Result<()> {
        let mut f = open_private(&tmp)?;
        f.write_all(token.trim().as_bytes())?;
        f.write_all(b"\n")?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(CoreError::Other(format!("writing token file {}: {e}", path.display())));
    }
    Ok(())
}

/// Delete the token file. Returns whether a file was removed.
pub fn delete_token(path: &Path) -> Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(CoreError::Other(format!("removing token file {}: {e}", path.display()))),
    }
}

fn tmp_name(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "token".into());
    path.with_file_name(format!(".{name}.{}.tmp", crate::new_id()))
}

#[cfg(unix)]
fn open_private(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn open_private(path: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new().write(true).create_new(true).open(path)
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    if dir.is_dir() {
        return Ok(());
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| CoreError::Other(format!("creating {}: {e}", dir.display())))
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).map_err(|e| CoreError::Other(format!("creating {}: {e}", dir.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn write_creates_parents_with_0600() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/b/token");
        write_token(&path, "spk_secret\n").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let parent_mode = fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777;
        assert_eq!(parent_mode, 0o700);
        assert_eq!(read_token(&path).unwrap().as_deref(), Some("spk_secret"));
        // overwrite keeps 0600 and leaves no temp files behind
        write_token(&path, "spk_other").unwrap();
        assert_eq!(read_token(&path).unwrap().as_deref(), Some("spk_other"));
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn missing_or_empty_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        assert_eq!(read_token(&path).unwrap(), None);
        write_token(&path, "  ").unwrap();
        assert_eq!(read_token(&path).unwrap(), None);
    }

    #[test]
    fn refuses_group_or_world_readable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        write_token(&path, "spk_x").unwrap();
        for mode in [0o640, 0o604, 0o644, 0o660] {
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            let err = read_token(&path).unwrap_err().to_string();
            assert!(err.contains("chmod 600"), "{err}");
            assert!(err.contains("accessible by group or others"), "{err}");
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
        assert_eq!(read_token(&path).unwrap().as_deref(), Some("spk_x"));
    }

    #[test]
    fn delete_reports_presence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        assert!(!delete_token(&path).unwrap());
        write_token(&path, "t").unwrap();
        assert!(delete_token(&path).unwrap());
        assert!(!path.exists());
    }
}
