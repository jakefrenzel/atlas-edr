//! Crash recovery (sensor spec §8.2): a segment torn at any byte, or corrupted,
//! recovers to exactly its valid prefix, and writing continues after it.

mod common;

use atlas_buffer::record::{self, MAX_PAYLOAD, RECORD_HEADER};
use atlas_buffer::{Cursor, Reader, SEGMENT_HEADER, SEGMENT_MAGIC, Writer};
use common::*;
use proptest::prelude::*;

const FIRST: &str = "00000000000000000001.seg";

/// A segment holding records of different sizes, and each record's end offset.
fn sample_segment() -> (Vec<u8>, Vec<usize>) {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    let payloads: [&[u8]; 4] = [&rec(0), b"x", &[7; 100], &rec(3)];
    for p in payloads {
        w.append(p).unwrap();
    }
    w.close().unwrap();
    let data = std::fs::read(tmp.path().join(FIRST)).unwrap();
    let mut ends = Vec::new();
    let mut at = SEGMENT_HEADER as usize;
    for p in payloads {
        at += RECORD_HEADER + p.len();
        ends.push(at);
    }
    assert_eq!(*ends.last().unwrap(), data.len());
    (data, ends)
}

#[test]
fn a_segment_torn_at_every_byte_recovers_its_whole_records() {
    let (data, ends) = sample_segment();
    for cut in 0..=data.len() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(FIRST), &data[..cut]).unwrap();

        let (mut w, recovery) = Writer::open(cfg(tmp.path()), time_of).unwrap();
        let whole = ends.iter().filter(|&&e| e <= cut).count();
        let valid_end = if cut < SEGMENT_HEADER as usize { 0 } else { ends[..whole].last().map_or(8, |&e| e) };
        assert_eq!(recovery.truncated_bytes, (cut - valid_end) as u64, "cut at {cut}");
        assert_eq!(recovery.foreign_segment, None, "cut at {cut}");

        // Writing continues right after the last whole record.
        write(&mut w, [99]);
        let mut reader = Reader::open(tmp.path(), Cursor::default());
        let got: Vec<_> = std::iter::from_fn(|| reader.next_record().unwrap()).map(|r| r.payload).collect();
        assert_eq!(got.len(), whole + 1, "cut at {cut}");
        assert_eq!(got.last().unwrap(), &rec(99), "cut at {cut}");
        assert_eq!(reader.stats().corrupt_segments, 0);
    }
}

#[test]
fn a_corrupt_record_in_the_newest_segment_is_cut_with_everything_after_it() {
    let (mut data, ends) = sample_segment();
    data[ends[1] + RECORD_HEADER + 50] ^= 0x01; // inside the third record's payload
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(FIRST), &data).unwrap();
    let (_, recovery) = Writer::open(cfg(tmp.path()), time_of).unwrap();
    assert_eq!(recovery.truncated_bytes, (data.len() - ends[1]) as u64);
    assert_eq!(std::fs::metadata(tmp.path().join(FIRST)).unwrap().len(), ends[1] as u64);
}

#[test]
fn a_foreign_newest_segment_is_kept_and_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, 0..2);
    drop(w);
    std::fs::write(tmp.path().join("00000000000000000002.seg"), b"not an atlas segment").unwrap();

    let (mut w, recovery) = Writer::open(cfg(tmp.path()), time_of).unwrap();
    assert_eq!(recovery.foreign_segment, Some(2));
    write(&mut w, [2]);
    assert!(tmp.path().join("00000000000000000003.seg").exists());

    let mut reader = Reader::open(tmp.path(), Cursor::default());
    let got: Vec<_> =
        std::iter::from_fn(|| reader.next_record().unwrap()).map(|r| time_of(&r.payload).unwrap()).collect();
    assert_eq!(got, vec![0, 1, 2]);
    assert_eq!(reader.stats().corrupt_segments, 1);
}

#[test]
fn a_header_torn_at_creation_is_rewritten() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(FIRST), &SEGMENT_MAGIC[..3]).unwrap();
    let (mut w, recovery) = Writer::open(cfg(tmp.path()), time_of).unwrap();
    assert_eq!((recovery.truncated_bytes, recovery.foreign_segment), (3, None));
    write(&mut w, [5]);
    assert_eq!(read_times(tmp.path(), Cursor::default()), vec![5]);
}

#[test]
fn a_zero_filled_newest_segment_is_a_torn_header_not_foreign() {
    // NTFS can keep a new file's size after a power cut while its bytes read as zeros.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(FIRST), [0u8; 64]).unwrap();
    let (mut w, recovery) = Writer::open(cfg(tmp.path()), time_of).unwrap();
    assert_eq!((recovery.truncated_bytes, recovery.foreign_segment), (64, None));
    write(&mut w, [5]);
    let mut reader = Reader::open(tmp.path(), Cursor::default());
    assert_eq!(reader.next_record().unwrap().map(|r| time_of(&r.payload)), Some(Some(5)));
    assert_eq!(reader.stats().corrupt_segments, 0);
}

/// A reader (the transport) can ack records that were written but not yet flushed.
/// If a power cut then loses them, new records must not land behind the ack.
#[test]
fn an_ack_ahead_of_a_power_cut_does_not_hide_new_records() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, 0..4);
    let mut reader = Reader::open(tmp.path(), Cursor::default());
    let acked = (0..4).map(|_| reader.next_record().unwrap().unwrap().next).last().unwrap();
    w.ack(acked).unwrap();
    drop(w);
    // The power cut kept only the first two records.
    let file = std::fs::OpenOptions::new().write(true).open(tmp.path().join(FIRST)).unwrap();
    file.set_len(SEGMENT_HEADER + 2 * FRAMED).unwrap();
    drop(file);

    let mut w = open(cfg(tmp.path()));
    assert_eq!(w.acked(), Some(acked));
    write(&mut w, 100..110);
    assert_eq!(read_times(tmp.path(), acked), (100..110).collect::<Vec<_>>());
}

#[test]
fn a_cursor_at_the_last_segment_number_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let mut bytes = u64::MAX.to_le_bytes().to_vec();
    bytes.extend_from_slice(&8u64.to_le_bytes());
    let crc = crc32c::crc32c(&bytes);
    bytes.extend_from_slice(&crc.to_le_bytes());
    std::fs::write(tmp.path().join("cursor"), bytes).unwrap();
    let err = Writer::open(cfg(tmp.path()), time_of).err().expect("no segment number after u64::MAX");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

fn framed(payloads: &[Vec<u8>]) -> (Vec<u8>, Vec<usize>) {
    let mut data = Vec::new();
    let mut ends = Vec::new();
    for p in payloads {
        data.extend_from_slice(&(p.len() as u32).to_le_bytes());
        data.extend_from_slice(&crc32c::crc32c(p).to_le_bytes());
        data.extend_from_slice(p);
        ends.push(data.len());
    }
    (data, ends)
}

proptest! {
    /// The pure scan behind recovery: any prefix yields exactly the whole records in it.
    #[test]
    fn scan_of_any_prefix_is_the_whole_records(
        payloads in prop::collection::vec(prop::collection::vec(any::<u8>(), 1..300), 0..12),
        cut in any::<prop::sample::Index>(),
    ) {
        let (data, ends) = framed(&payloads);
        let cut = cut.index(data.len() + 1);
        let scan = record::scan(&data[..cut], 0);
        let whole = ends.iter().filter(|&&e| e <= cut).count();
        prop_assert_eq!(scan.records.len(), whole);
        prop_assert_eq!(scan.valid_end, if whole == 0 { 0 } else { ends[whole - 1] });
    }

    /// A changed byte inside record k leaves exactly records 0..k.
    #[test]
    fn a_changed_byte_stops_the_scan_at_its_record(
        payloads in prop::collection::vec(prop::collection::vec(any::<u8>(), 1..300), 1..12),
        at in any::<prop::sample::Index>(),
        delta in 1u8..=255,
    ) {
        let (mut data, ends) = framed(&payloads);
        let i = at.index(data.len());
        data[i] = data[i].wrapping_add(delta);
        let k = ends.iter().filter(|&&e| e <= i).count();
        prop_assert_eq!(record::scan(&data, 0).records.len(), k);
    }

    #[test]
    fn scan_never_panics_on_arbitrary_bytes(data in prop::collection::vec(any::<u8>(), 0..2048)) {
        let scan = record::scan(&data, 0);
        prop_assert!(scan.valid_end <= data.len());
        for (start, end) in scan.records {
            prop_assert!(end - start <= MAX_PAYLOAD);
        }
    }
}
