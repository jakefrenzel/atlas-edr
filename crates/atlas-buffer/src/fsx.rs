//! File-system helpers: segment naming, and opening files the way the agent's
//! data directory requires (sensor spec §8.5, §11.1).
//!
//! - Every handle shares read, write and delete, so a reader (`dump --follow`)
//!   never blocks the writer, and retention can delete a segment a reader holds.
//! - Nothing inside the buffer directory may be a reparse point (symlink or
//!   junction): on Windows files are opened with `FILE_FLAG_OPEN_REPARSE_POINT`
//!   and the handle's attributes are checked, so the check and the use are the
//!   same handle.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// Every segment starts with this 8-byte header: a magic number and a format version.
pub const SEGMENT_MAGIC: [u8; 8] = *b"ATLSEG01";
pub const SEGMENT_HEADER: u64 = SEGMENT_MAGIC.len() as u64;

const SEGMENT_EXT: &str = "seg";

/// `00000000000000000042.seg`: zero-padded so names sort like numbers.
pub fn segment_path(dir: &Path, seq: u64) -> PathBuf {
    dir.join(format!("{seq:020}.{SEGMENT_EXT}"))
}

fn parse_segment_name(name: &str) -> Option<u64> {
    let digits = name.strip_suffix(".seg")?;
    if digits.len() != 20 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Sequence numbers of the segments in `dir`, ascending. Other names are ignored.
pub fn list_segments(dir: &Path) -> io::Result<Vec<u64>> {
    let mut seqs = Vec::new();
    for entry in fs::read_dir(dir)? {
        if let Some(seq) = entry?.file_name().to_str().and_then(parse_segment_name) {
            seqs.push(seq);
        }
    }
    seqs.sort_unstable();
    Ok(seqs)
}

fn invalid(what: &str, path: &Path) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("{what}: {}", path.display()))
}

#[cfg(windows)]
mod imp {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};

    use super::*;

    const FILE_SHARE_READ: u32 = 0x1;
    const FILE_SHARE_WRITE: u32 = 0x2;
    const FILE_SHARE_DELETE: u32 = 0x4;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

    pub fn open(path: &Path, opts: &mut OpenOptions) -> io::Result<File> {
        let file = opts
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let meta = file.metadata()?;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 || !meta.is_file() {
            return Err(invalid("not a regular file", path));
        }
        Ok(file)
    }

    pub fn is_reparse_point(meta: &fs::Metadata) -> bool {
        meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    /// Non-Windows builds are for CI only; the check here is best effort.
    pub fn open(path: &Path, opts: &mut OpenOptions) -> io::Result<File> {
        match fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() => return Err(invalid("not a regular file", path)),
            _ => {}
        }
        let file = opts.open(path)?;
        if !file.metadata()?.is_file() {
            return Err(invalid("not a regular file", path));
        }
        Ok(file)
    }

    pub fn is_reparse_point(meta: &fs::Metadata) -> bool {
        meta.file_type().is_symlink()
    }
}

/// Opens a file inside the buffer directory (see the module docs).
pub fn open(path: &Path, opts: &mut OpenOptions) -> io::Result<File> {
    imp::open(path, opts)
}

pub fn open_read(path: &Path) -> io::Result<File> {
    open(path, OpenOptions::new().read(true))
}

/// Read-write, created if missing, never truncated.
pub fn open_rw(path: &Path) -> io::Result<File> {
    open(path, OpenOptions::new().read(true).write(true).create(true).truncate(false))
}

/// The size of a segment, without following a symlink or junction: one is refused.
pub fn segment_len(path: &Path) -> io::Result<u64> {
    let meta = fs::symlink_metadata(path)?;
    if imp::is_reparse_point(&meta) || !meta.is_file() {
        return Err(invalid("not a regular file", path));
    }
    Ok(meta.len())
}

/// Creates the buffer directory if needed (its parent must exist) and checks
/// that it is a real directory, not a symlink or junction.
pub fn prepare_dir(dir: &Path) -> io::Result<()> {
    match fs::create_dir(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let meta = fs::symlink_metadata(dir)?;
    if imp::is_reparse_point(&meta) || !meta.is_dir() {
        return Err(invalid("buffer directory is not a plain directory", dir));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_names_round_trip_and_sort_numerically() {
        let dir = Path::new("buf");
        let name = segment_path(dir, 42).file_name().unwrap().to_str().unwrap().to_owned();
        assert_eq!(name, "00000000000000000042.seg");
        assert_eq!(parse_segment_name(&name), Some(42));
        assert!(segment_path(dir, 9).file_name() < segment_path(dir, 10).file_name());
    }

    #[test]
    fn other_names_are_not_segments() {
        for name in ["cursor", "cursor.tmp", "42.seg", "0000000000000000004x.seg", "00000000000000000042.seg.tmp"] {
            assert_eq!(parse_segment_name(name), None, "{name}");
        }
    }

    #[test]
    fn list_ignores_other_files() {
        let tmp = tempfile::tempdir().unwrap();
        for seq in [3, 1, 2] {
            File::create(segment_path(tmp.path(), seq)).unwrap();
        }
        File::create(tmp.path().join("cursor")).unwrap();
        assert_eq!(list_segments(tmp.path()).unwrap(), vec![1, 2, 3]);
    }
}
