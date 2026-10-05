//! Readers beside a live writer (sensor spec §8.5): `dump --follow` and, later, the transport.

mod common;

use std::fs::OpenOptions;
use std::io::Write;

use atlas_buffer::{Cursor, Reader, SEGMENT_HEADER};
use common::*;

fn seg(dir: &std::path::Path, seq: u64) -> std::path::PathBuf {
    dir.join(format!("{seq:020}.seg"))
}

/// Creating symlinks needs a privilege (Developer Mode or elevation). Without it the test
/// skips locally, but fails in CI (`CI` is set on GitHub runners), where a skip would hide
/// that the reparse-point checks never ran.
fn skip_or_fail(e: std::io::Error) {
    if std::env::var_os("CI").is_some() {
        panic!("cannot create a symlink in CI: {e}");
    }
    eprintln!("skipped: cannot create a symlink here ({e})");
}

fn times(reader: &mut Reader) -> Vec<i64> {
    std::iter::from_fn(|| reader.next_record().unwrap()).map(|r| time_of(&r.payload).unwrap()).collect()
}

#[test]
fn a_follower_sees_new_records_and_new_segments() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    let mut reader = Reader::open(tmp.path(), Cursor::default());
    assert_eq!(times(&mut reader), Vec::<i64>::new(), "nothing written yet");
    write(&mut w, 0..4);
    assert_eq!(times(&mut reader), vec![0, 1, 2, 3]);
    write(&mut w, 4..15); // rotates twice
    assert_eq!(times(&mut reader), (4..15).collect::<Vec<_>>());
    assert_eq!(reader.position().segment, 3);
}

#[test]
fn an_incomplete_tail_in_the_newest_segment_is_waited_for() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, 0..2);
    let mut reader = Reader::open(tmp.path(), Cursor::default());
    assert_eq!(times(&mut reader), vec![0, 1]);

    // The next record lands in two pieces, as a concurrent read may see it.
    let mut framed = Vec::new();
    let payload = rec(2);
    framed.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    framed.extend_from_slice(&crc32c::crc32c(&payload).to_le_bytes());
    framed.extend_from_slice(&payload);
    let mut file = OpenOptions::new().append(true).open(seg(tmp.path(), 1)).unwrap();
    file.write_all(&framed[..13]).unwrap();
    assert_eq!(times(&mut reader), Vec::<i64>::new(), "not written yet, not corrupt");
    file.write_all(&framed[13..]).unwrap();
    assert_eq!(times(&mut reader), vec![2]);
    assert_eq!(reader.stats().corrupt_segments, 0);
}

#[test]
fn a_bad_record_in_a_sealed_segment_skips_the_rest_of_that_segment() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, 0..20); // segments 1..=4, 6 records each (4 in the last)
    let path = seg(tmp.path(), 2);
    let mut data = std::fs::read(&path).unwrap();
    data[SEGMENT_HEADER as usize + 2 * FRAMED as usize + 20] ^= 0xff; // third record of segment 2
    std::fs::write(&path, data).unwrap();

    let mut reader = Reader::open(tmp.path(), Cursor::default());
    let expected: Vec<i64> = (0..6).chain(6..8).chain(12..20).collect();
    assert_eq!(times(&mut reader), expected);
    assert_eq!(reader.stats().corrupt_segments, 1);
}

#[test]
fn a_bad_record_in_the_newest_segment_is_not_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, 0..3);
    let path = seg(tmp.path(), 1);
    let mut data = std::fs::read(&path).unwrap();
    data[SEGMENT_HEADER as usize + FRAMED as usize + 20] ^= 0xff; // second record
    std::fs::write(&path, data).unwrap();

    let mut reader = Reader::open(tmp.path(), Cursor::default());
    assert_eq!(times(&mut reader), vec![0], "waits: the writer may still be writing it");
    assert_eq!(reader.stats().corrupt_segments, 0);
}

#[test]
fn segments_deleted_under_a_reader_are_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, 0..20);
    let mut reader = Reader::open(tmp.path(), Cursor::default());
    assert_eq!(time_of(&reader.next_record().unwrap().unwrap().payload), Some(0));

    // Segments 1 and 2 are deleted while the reader holds segment 1 open. On Windows this
    // needs FILE_SHARE_DELETE on the reader's handle.
    w.ack(Cursor { segment: 3, offset: SEGMENT_HEADER }).unwrap();
    assert!(!seg(tmp.path(), 1).exists() && !seg(tmp.path(), 2).exists());

    // The open segment is read to its end, then the deleted one is skipped.
    let expected: Vec<i64> = (1..6).chain(12..20).collect();
    assert_eq!(times(&mut reader), expected);
}

#[test]
fn a_cursor_into_a_deleted_segment_starts_at_the_next_one() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, 0..20);
    w.ack(Cursor { segment: 3, offset: SEGMENT_HEADER }).unwrap();
    assert_eq!(read_times(tmp.path(), Cursor { segment: 2, offset: 48 }), (12..20).collect::<Vec<_>>());
}

#[cfg(windows)]
#[test]
fn a_segment_that_is_a_symlink_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, [1]);
    drop(w);
    let target = tmp.path().join("elsewhere.bin");
    std::fs::copy(seg(tmp.path(), 1), &target).unwrap();
    if let Err(e) = std::os::windows::fs::symlink_file(&target, seg(tmp.path(), 2)) {
        skip_or_fail(e);
        return;
    }
    let err = atlas_buffer::Writer::open(cfg(tmp.path()), time_of).err().expect("newest segment is a symlink");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

    let mut reader = Reader::open(tmp.path(), Cursor { segment: 2, offset: 0 });
    assert_eq!(reader.next_record().unwrap_err().kind(), std::io::ErrorKind::InvalidData);

    // A sealed segment that is a symlink is refused too.
    std::fs::copy(seg(tmp.path(), 1), seg(tmp.path(), 3)).unwrap();
    let err = atlas_buffer::Writer::open(cfg(tmp.path()), time_of).err().expect("sealed segment is a symlink");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

/// A tool that opens a segment without delete sharing (an editor, `Get-Content`)
/// must not stall the writer or double-count the eviction.
#[cfg(windows)]
#[test]
fn a_segment_that_cannot_be_deleted_is_skipped_and_counted_once() {
    use std::os::windows::fs::OpenOptionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(atlas_buffer::Config { overflow: atlas_buffer::Overflow::DropOldest, ..cfg(tmp.path()) });
    write(&mut w, 0..24); // 4 full segments: the cap is reached at the next rotation
    let held = OpenOptions::new().read(true).share_mode(0x1).open(seg(tmp.path(), 1)).unwrap();

    for t in 24..31 {
        write(&mut w, [t]); // `write` panics if a tick fails
    }
    assert!(!w.is_failing());
    assert_eq!(w.stats().delete_failures, 2, "segment 1 was tried at the two rotations");
    // Segments 2 and 3 went instead; segment 1 is still there.
    assert_eq!(w.take_gaps().iter().map(|g| g.first_time).collect::<Vec<_>>(), vec![Some(6), Some(12)]);
    assert_eq!(w.stats().overflow_evictions, 2);

    drop(held);
    write(&mut w, 31..37);
    assert_eq!(w.take_gaps().len(), 1, "segment 1 goes at the next rotation");
    assert!(!seg(tmp.path(), 1).exists());
}

#[test]
fn a_buffer_directory_that_is_a_symlink_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let link = tmp.path().join("buffer");
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_dir(&real, &link);
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(&real, &link);
    if let Err(e) = made {
        skip_or_fail(e);
        return;
    }
    let err = atlas_buffer::Writer::open(cfg(&link), time_of).err().expect("directory is a symlink");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}
