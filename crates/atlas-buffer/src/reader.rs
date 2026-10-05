//! Readers (sensor spec §8.5). Safe beside a live writer, in this process or another:
//! - a record that is not completely there yet in the newest segment means
//!   "not written yet": `next_record` returns `None` and the caller polls again;
//! - a bad record in a sealed segment (one with a newer segment after it) is
//!   corruption: the rest of that segment is skipped and counted;
//! - a segment deleted under the reader (retention, overflow, ack) is skipped.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::cursor::Cursor;
use crate::fsx::{self, SEGMENT_HEADER, SEGMENT_MAGIC, segment_path};
use crate::record::{self, Frame, RECORD_HEADER};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// Where this record starts.
    pub at: Cursor,
    /// Where the next record starts: pass it to `Writer::ack` once this record is delivered.
    pub next: Cursor,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReaderStats {
    /// Sealed segments whose remainder was skipped as corrupt (bad record or bad header).
    pub corrupt_segments: u64,
}

pub struct Reader {
    dir: PathBuf,
    pos: Cursor,
    file: Option<File>,
    stats: ReaderStats,
}

enum At {
    Record(Record),
    /// Nothing more at this position yet.
    End,
    /// A partial or invalid record at this position.
    Bad,
}

impl Reader {
    /// A reader positioned at `from` (`Cursor::default()` for the oldest record).
    /// No I/O happens until `next_record`.
    pub fn open(dir: &Path, from: Cursor) -> Self {
        Self { dir: dir.to_owned(), pos: from, file: None, stats: ReaderStats::default() }
    }

    /// The position of the next record to read.
    pub fn position(&self) -> Cursor {
        self.pos
    }

    pub fn stats(&self) -> ReaderStats {
        self.stats
    }

    /// The next record, or `None` if the reader has caught up with the writer.
    pub fn next_record(&mut self) -> io::Result<Option<Record>> {
        loop {
            if self.file.is_none() && !self.enter_segment()? {
                return Ok(None);
            }
            match self.read_at()? {
                At::Record(r) => return Ok(Some(r)),
                At::End | At::Bad if !self.newer_segment_exists()? => return Ok(None),
                At::End | At::Bad => {
                    // Sealed: its contents are final now, so read once more before moving on.
                    match self.read_at()? {
                        At::Record(r) => return Ok(Some(r)),
                        At::End => {}
                        At::Bad => self.stats.corrupt_segments += 1,
                    }
                    self.next_segment();
                }
            }
        }
    }

    /// Only called on a sealed segment, so a larger number exists and this cannot overflow.
    fn next_segment(&mut self) {
        self.file = None;
        self.pos = Cursor { segment: self.pos.segment.saturating_add(1), offset: 0 };
    }

    fn newer_segment_exists(&self) -> io::Result<bool> {
        Ok(fsx::list_segments(&self.dir)?.last().is_some_and(|&s| s > self.pos.segment))
    }

    /// Opens the first existing segment at or after the position. `false`: none
    /// is ready yet.
    fn enter_segment(&mut self) -> io::Result<bool> {
        loop {
            let seqs = fsx::list_segments(&self.dir)?;
            let Some(&seq) = seqs.iter().find(|&&s| s >= self.pos.segment) else { return Ok(false) };
            if seq > self.pos.segment {
                self.pos = Cursor { segment: seq, offset: 0 };
            }
            let sealed = seqs.last().is_some_and(|&last| last > seq);
            let mut file = match fsx::open_read(&segment_path(&self.dir, seq)) {
                Ok(f) => f,
                // A sealed segment deleted (or delete-pending) since the listing: skip it.
                // The newest segment is never deleted, so there the error is real.
                Err(e) if sealed && matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied) => {
                    self.next_segment();
                    continue;
                }
                Err(e) => return Err(e),
            };
            let mut header = [0u8; SEGMENT_MAGIC.len()];
            let n = read_up_to(&mut file, &mut header)?;
            if n == header.len() && header == SEGMENT_MAGIC {
                self.pos.offset = self.pos.offset.max(SEGMENT_HEADER);
                self.file = Some(file);
                return Ok(true);
            }
            if !sealed {
                return Ok(false); // being created
            }
            self.stats.corrupt_segments += 1;
            self.next_segment();
        }
    }

    fn read_at(&mut self) -> io::Result<At> {
        let file = self.file.as_mut().expect("entered");
        file.seek(SeekFrom::Start(self.pos.offset))?;
        let mut buf = vec![0u8; RECORD_HEADER];
        match read_up_to(file, &mut buf)? {
            0 => return Ok(At::End),
            n if n < RECORD_HEADER => return Ok(At::Bad),
            _ => {}
        }
        // The header alone tells a bad length (Invalid) from a missing payload (Incomplete),
        // so nothing is allocated for an invalid length.
        if record::parse(&buf, 0) == Frame::Invalid {
            return Ok(At::Bad);
        }
        let len = u32::from_le_bytes(buf[..4].try_into().expect("4 bytes")) as usize;
        buf.resize(RECORD_HEADER + len, 0);
        if read_up_to(file, &mut buf[RECORD_HEADER..])? < len {
            return Ok(At::Bad);
        }
        let Frame::Record { .. } = record::parse(&buf, 0) else { return Ok(At::Bad) };
        let at = self.pos;
        self.pos.offset += buf.len() as u64;
        buf.drain(..RECORD_HEADER);
        Ok(At::Record(Record { at, next: self.pos, payload: buf }))
    }
}

/// Reads until `buf` is full or the file ends; returns the bytes read.
fn read_up_to(file: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match file.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}
