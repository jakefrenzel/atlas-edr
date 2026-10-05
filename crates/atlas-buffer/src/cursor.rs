//! The ack cursor (sensor spec §8.5): a position in the log, persisted
//! atomically as `[u64 LE segment][u64 LE offset][u32 LE CRC32C of both]`.

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;

use crate::fsx;

const CURSOR_FILE: &str = "cursor";
const CURSOR_TMP: &str = "cursor.tmp";
const CURSOR_LEN: usize = 20;

/// A position in the log: the next record to read is at `offset` in segment `segment`.
/// `Cursor::default()` is before every record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cursor {
    pub segment: u64,
    pub offset: u64,
}

fn encode(c: Cursor) -> [u8; CURSOR_LEN] {
    let mut out = [0u8; CURSOR_LEN];
    out[..8].copy_from_slice(&c.segment.to_le_bytes());
    out[8..16].copy_from_slice(&c.offset.to_le_bytes());
    let crc = crc32c::crc32c(&out[..16]);
    out[16..].copy_from_slice(&crc.to_le_bytes());
    out
}

fn decode(bytes: &[u8]) -> Option<Cursor> {
    let bytes: &[u8; CURSOR_LEN] = bytes.try_into().ok()?;
    let crc = u32::from_le_bytes(bytes[16..].try_into().ok()?);
    if crc32c::crc32c(&bytes[..16]) != crc {
        return None;
    }
    Some(Cursor {
        segment: u64::from_le_bytes(bytes[..8].try_into().ok()?),
        offset: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
    })
}

/// What was found in the cursor file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Loaded {
    Missing,
    Valid(Cursor),
    /// Wrong size or bad CRC: delivery restarts from the oldest record (at-least-once).
    Corrupt,
}

pub(crate) fn load(dir: &Path) -> io::Result<Loaded> {
    let path = dir.join(CURSOR_FILE);
    let file = match fsx::open_read(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Loaded::Missing),
        Err(e) => return Err(e),
    };
    let mut bytes = Vec::with_capacity(CURSOR_LEN);
    // One byte more than a valid file, so an oversized file is detected without reading it all.
    file.take(CURSOR_LEN as u64 + 1).read_to_end(&mut bytes)?;
    Ok(decode(&bytes).map_or(Loaded::Corrupt, Loaded::Valid))
}

/// Writes the cursor to a temporary file, flushes it, then renames it over the old one.
pub(crate) fn store(dir: &Path, cursor: Cursor) -> io::Result<()> {
    let tmp = dir.join(CURSOR_TMP);
    {
        let mut file = fsx::open(&tmp, OpenOptions::new().write(true).create(true).truncate(true))?;
        file.write_all(&encode(cursor))?;
        file.sync_all()?;
    }
    fs::rename(&tmp, dir.join(CURSOR_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_then_load_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(load(tmp.path()).unwrap(), Loaded::Missing);
        for c in [Cursor { segment: 1, offset: 8 }, Cursor { segment: u64::MAX, offset: 1 << 40 }] {
            store(tmp.path(), c).unwrap();
            assert_eq!(load(tmp.path()).unwrap(), Loaded::Valid(c));
        }
        assert!(!tmp.path().join(CURSOR_TMP).exists());
    }

    #[test]
    fn damaged_files_are_corrupt() {
        let tmp = tempfile::tempdir().unwrap();
        let good = encode(Cursor { segment: 3, offset: 99 });
        let mut flipped = good;
        flipped[0] ^= 1;
        let mut long = good.to_vec();
        long.push(0);
        for bytes in [&good[..19], &flipped[..], &long[..], &[][..]] {
            fs::write(tmp.path().join(CURSOR_FILE), bytes).unwrap();
            assert_eq!(load(tmp.path()).unwrap(), Loaded::Corrupt, "{} bytes", bytes.len());
        }
    }

    #[test]
    fn cursors_order_by_segment_then_offset() {
        assert!(Cursor { segment: 1, offset: 900 } < Cursor { segment: 2, offset: 8 });
        assert!(Cursor { segment: 2, offset: 8 } < Cursor { segment: 2, offset: 9 });
    }
}
