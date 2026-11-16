//! ZIP and gzip helpers for downloads (docs/set-format.md, docs/whatsapp.md):
//! a set export as ZIP holds exactly the entries of the tar export, and
//! WhatsApp `.wastickers` files are ZIPs too.
//!
//! Entries are stored, not deflated: the media inside is compressed
//! already. Timestamps are fixed (1980-01-01, the earliest a ZIP can say)
//! and permissions are 0644 / 0755, so the same content gives the same bytes.

use std::io::{Cursor, Read, Seek, Write};
use std::path::Path;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

fn zip_err(e: zip::result::ZipError) -> std::io::Error {
    match e {
        zip::result::ZipError::Io(e) => e,
        other => std::io::Error::other(other.to_string()),
    }
}

fn options(mode: u32, size: u64) -> SimpleFileOptions {
    SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(zip::DateTime::default())
        .unix_permissions(mode)
        .large_file(size >= u64::from(u32::MAX))
}

/// A ZIP being written to `W`.
pub struct Zip<W: Write + Seek> {
    inner: ZipWriter<W>,
}

impl<W: Write + Seek> Zip<W> {
    pub fn new(w: W) -> Zip<W> {
        Zip {
            inner: ZipWriter::new(w),
        }
    }

    /// A file entry (`name` uses `/` between directories).
    pub fn file(&mut self, name: &str, bytes: &[u8]) -> std::io::Result<()> {
        self.inner
            .start_file(name, options(0o644, bytes.len() as u64))
            .map_err(zip_err)?;
        self.inner.write_all(bytes)
    }

    /// A directory entry (`name` ends in `/` or gets one).
    pub fn dir(&mut self, name: &str) -> std::io::Result<()> {
        self.inner.add_directory(name, options(0o755, 0)).map_err(zip_err)
    }

    pub fn finish(self) -> std::io::Result<W> {
        self.inner.finish().map_err(zip_err)
    }
}

/// A ZIP in memory with `entries` in this order.
pub fn zip_bytes<'a>(entries: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> std::io::Result<Vec<u8>> {
    let mut z = Zip::new(Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        z.file(name, bytes)?;
    }
    Ok(z.finish()?.into_inner())
}

/// The entries of a tar (directories and regular files, in tar order) as
/// a ZIP. Set exports are written as a tar first, so both downloads hold
/// exactly the same files.
pub fn tar_to_zip(tar: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut archive = tar::Archive::new(Cursor::new(tar));
    let mut z = Zip::new(Cursor::new(Vec::new()));
    for entry in archive.entries()? {
        let mut entry = entry?;
        let name = entry.path()?.to_string_lossy().replace('\\', "/");
        match entry.header().entry_type() {
            tar::EntryType::Directory => z.dir(&name)?,
            tar::EntryType::Regular => {
                let mut bytes = Vec::with_capacity(entry.size() as usize);
                entry.read_to_end(&mut bytes)?;
                z.file(&name, &bytes)?;
            }
            _ => {}
        }
    }
    Ok(z.finish()?.into_inner())
}

/// `bytes` gzip-compressed (`.tar.gz` downloads).
pub fn gzip(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(bytes)?;
    enc.finish()
}

/// The entry `name` of the ZIP at `path`, if it has one (at most `max`
/// bytes are read).
pub fn read_entry(path: &Path, name: &str, max: u64) -> std::io::Result<Option<Vec<u8>>> {
    let file = std::fs::File::open(path)?;
    let mut archive = ZipArchive::new(std::io::BufReader::new(file)).map_err(zip_err)?;
    let entry = match archive.by_name(name) {
        Ok(e) => e,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(zip_err(e)),
    };
    let mut out = Vec::new();
    entry.take(max).read_to_end(&mut out)?;
    Ok(Some(out))
}

/// Every entry of a ZIP in memory: (name, bytes), directories left out
/// (tests and checks).
pub fn entries(bytes: &[u8]) -> std::io::Result<Vec<(String, Vec<u8>)>> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(zip_err)?;
    let mut out = Vec::new();
    for i in 0..archive.len() {
        let mut f = archive.by_index(i).map_err(zip_err)?;
        if f.is_dir() {
            continue;
        }
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        let name = f.name().map_err(zip_err)?.into_owned();
        out.push((name, buf));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tar_entries_become_zip_entries() {
        let mut b = tar::Builder::new(Vec::new());
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Directory);
        h.set_size(0);
        h.set_mode(0o755);
        b.append_data(&mut h, "cats/", std::io::empty()).unwrap();
        for (name, body) in [
            ("cats/set.toml", &b"name = \"Cats\"\n"[..]),
            ("cats/01-a.png", b"\x89PNG.."),
        ] {
            let mut h = tar::Header::new_gnu();
            h.set_size(body.len() as u64);
            h.set_mode(0o644);
            b.append_data(&mut h, name, body).unwrap();
        }
        let tar = b.into_inner().unwrap();
        let zip = tar_to_zip(&tar).unwrap();
        assert_eq!(
            entries(&zip).unwrap(),
            vec![
                ("cats/set.toml".to_string(), b"name = \"Cats\"\n".to_vec()),
                ("cats/01-a.png".to_string(), b"\x89PNG..".to_vec())
            ]
        );
        assert_eq!(zip, tar_to_zip(&tar).unwrap(), "deterministic");
        let gz = gzip(&tar).unwrap();
        let mut back = Vec::new();
        flate2::read::GzDecoder::new(&gz[..]).read_to_end(&mut back).unwrap();
        assert_eq!(back, tar);
    }

    #[test]
    fn reads_one_entry_from_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.zip");
        std::fs::write(
            &path,
            zip_bytes([("a.txt", &b"alpha"[..]), ("b.txt", b"beta")]).unwrap(),
        )
        .unwrap();
        assert_eq!(read_entry(&path, "b.txt", 100).unwrap().unwrap(), b"beta");
        assert_eq!(read_entry(&path, "c.txt", 100).unwrap(), None);
        assert_eq!(read_entry(&path, "a.txt", 2).unwrap().unwrap(), b"al");
    }
}
