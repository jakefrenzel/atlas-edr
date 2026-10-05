//! Writing, reading, rotation, acks and the overflow policies (sensor spec §8.1, §8.4, §8.5).

mod common;

use std::time::{Duration, Instant};

use atlas_buffer::{Config, Cursor, Gap, Overflow, Reader, SEGMENT_HEADER, Writer};
use common::*;
use proptest::prelude::*;

#[test]
fn records_come_back_in_order_across_rotations() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, 0..20);
    assert_eq!(read_times(tmp.path(), Cursor::default()), (0..20).collect::<Vec<_>>());
    // 6 records per 256-byte segment.
    assert_eq!(segment_count(tmp.path()), 4);
    assert_eq!(w.stats().records_written, 20);
    assert_eq!(w.disk_bytes(), 4 * SEGMENT_HEADER + 20 * FRAMED);
}

#[test]
fn nothing_reaches_disk_before_the_first_tick() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    w.append(&rec(1)).unwrap();
    assert_eq!(read_times(tmp.path(), Cursor::default()), Vec::<i64>::new());
    flush(&mut w);
    assert_eq!(read_times(tmp.path(), Cursor::default()), vec![1]);
}

#[test]
fn writes_wait_for_the_flush_interval() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    let t0 = Instant::now();
    w.append(&rec(1)).unwrap();
    w.tick(t0).unwrap();
    w.append(&rec(2)).unwrap();
    w.tick(t0 + Duration::from_millis(999)).unwrap();
    assert_eq!(read_times(tmp.path(), Cursor::default()), vec![1]);
    w.tick(t0 + Duration::from_secs(1)).unwrap();
    assert_eq!(read_times(tmp.path(), Cursor::default()), vec![1, 2]);
}

#[test]
fn a_crowded_backlog_is_written_before_the_interval() {
    let tmp = tempfile::tempdir().unwrap();
    let backlog_bytes = 300 * 1024;
    let mut w = open(Config { backlog_bytes, segment_bytes: 1 << 20, cap_bytes: 4 << 20, ..Config::new(tmp.path()) });
    let t0 = Instant::now();
    w.tick(t0).unwrap();
    w.append(&vec![1; backlog_bytes / 2]).unwrap();
    w.tick(t0 + Duration::from_millis(1)).unwrap();
    assert_eq!(w.stats().records_written, 1);
}

#[test]
fn close_writes_everything() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    for t in 0..3 {
        w.append(&rec(t)).unwrap();
    }
    w.close().unwrap();
    assert_eq!(read_times(tmp.path(), Cursor::default()), vec![0, 1, 2]);
}

#[test]
fn reopening_starts_a_new_segment() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, 0..2);
    drop(w);
    let (mut w, recovery) = Writer::open(cfg(tmp.path()), time_of).unwrap();
    assert_eq!(recovery, atlas_buffer::Recovery::default());
    write(&mut w, 2..4);
    assert_eq!(segment_count(tmp.path()), 2);
    assert_eq!(read_times(tmp.path(), Cursor::default()), vec![0, 1, 2, 3]);
}

#[test]
fn opening_and_idling_creates_no_segment() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    flush(&mut w);
    drop(w);
    let _w = open(cfg(tmp.path()));
    assert_eq!(segment_count(tmp.path()), 0);
}

#[test]
fn a_reader_resumes_from_a_record_cursor() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, 0..10);
    let mut reader = Reader::open(tmp.path(), Cursor::default());
    let mut cursor = Cursor::default();
    for _ in 0..7 {
        cursor = reader.next_record().unwrap().unwrap().next;
    }
    assert_eq!(cursor, reader.position());
    assert_eq!(read_times(tmp.path(), cursor), vec![7, 8, 9]);
}

#[test]
fn ack_persists_the_cursor_and_deletes_delivered_segments() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, 0..20); // segments 1..=4
    let mut reader = Reader::open(tmp.path(), Cursor::default());
    let cursor = (0..13).map(|_| reader.next_record().unwrap().unwrap().next).last().unwrap();
    assert_eq!(cursor.segment, 3);

    w.ack(cursor).unwrap();
    assert_eq!(segment_count(tmp.path()), 2, "segments 1 and 2 were wholly delivered");
    assert_eq!(w.acked(), Some(cursor));

    // An older cursor is ignored; one past the newest segment is refused.
    w.ack(Cursor { segment: 1, offset: 8 }).unwrap();
    assert_eq!(w.acked(), Some(cursor));
    assert!(w.ack(Cursor { segment: 99, offset: 8 }).is_err());

    drop(w);
    let w = open(cfg(tmp.path()));
    assert_eq!(w.acked(), Some(cursor));
    assert_eq!(read_times(tmp.path(), cursor), (13..20).collect::<Vec<_>>());
}

#[test]
fn a_damaged_cursor_restarts_delivery_from_the_oldest_record() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, 0..3);
    w.ack(Cursor { segment: 1, offset: SEGMENT_HEADER + FRAMED }).unwrap();
    drop(w);
    std::fs::write(tmp.path().join("cursor"), b"garbage").unwrap();
    let (w, recovery) = Writer::open(cfg(tmp.path()), time_of).unwrap();
    assert!(recovery.cursor_reset);
    assert_eq!(w.acked(), None);
}

#[test]
fn segments_left_behind_the_cursor_are_deleted_at_open() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, 0..20);
    w.ack(Cursor { segment: 3, offset: SEGMENT_HEADER }).unwrap();
    drop(w);
    // Simulate a crash between writing the cursor and deleting segment 2.
    std::fs::copy(tmp.path().join("00000000000000000003.seg"), tmp.path().join("00000000000000000002.seg")).unwrap();
    let _w = open(cfg(tmp.path()));
    assert!(!tmp.path().join("00000000000000000002.seg").exists());
}

#[test]
fn new_segments_never_reuse_a_delivered_number() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    write(&mut w, 0..20); // newest is segment 4
    w.ack(Cursor { segment: 4, offset: SEGMENT_HEADER + 2 * FRAMED }).unwrap();
    drop(w);
    // Every segment deleted out of band: the next one must sort after the cursor.
    for e in std::fs::read_dir(tmp.path()).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "seg") {
            std::fs::remove_file(p).unwrap();
        }
    }
    let mut w = open(cfg(tmp.path()));
    write(&mut w, [100]);
    assert!(tmp.path().join("00000000000000000005.seg").exists());
    assert_eq!(read_times(tmp.path(), w.acked().unwrap()), vec![100]);
}

// ---------------------------------------------------------------- overflow

/// Writes `n` records one tick at a time; returns the policy's gaps.
fn fill(w: &mut Writer, times: std::ops::Range<i64>) -> Vec<Gap> {
    let mut gaps = Vec::new();
    for t in times {
        write(w, [t]);
        gaps.extend(w.take_gaps());
    }
    gaps
}

#[test]
fn retention_deletes_the_oldest_segment_without_a_gap() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(cfg(tmp.path()));
    let gaps = fill(&mut w, 0..60);
    assert!(gaps.is_empty());
    assert!(w.disk_bytes() <= 1024);
    assert!(w.stats().retention_evictions > 0);
    // What is left is the newest, contiguous history.
    let times = read_times(tmp.path(), Cursor::default());
    assert_eq!(times, (60 - times.len() as i64..60).collect::<Vec<_>>());
}

#[test]
fn drop_oldest_reports_each_deleted_segment_as_a_gap() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(Config { overflow: Overflow::DropOldest, ..cfg(tmp.path()) });
    // The 1024-byte cap holds 4 segments of 6 records (248 bytes each): starting the
    // 5th (record 24) and the 6th (record 30) each deletes the oldest.
    let gaps = fill(&mut w, 0..36);
    assert_eq!(
        gaps,
        vec![
            Gap { records: 6, first_time: Some(0), last_time: Some(5) },
            Gap { records: 6, first_time: Some(6), last_time: Some(11) },
        ]
    );
    assert_eq!(w.stats().overflow_evictions, 2);
    assert_eq!(read_times(tmp.path(), Cursor::default()), (12..36).collect::<Vec<_>>());
}

#[test]
fn head_tail_pins_at_least_a_quarter_of_the_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    // 16 segments under the cap, so the head is several segments, not just the first.
    let mut w = open(Config { overflow: Overflow::HeadTail, cap_bytes: 16 * 256, ..cfg(tmp.path()) });
    let gaps = fill(&mut w, 0..300);
    let kept = read_times(tmp.path(), Cursor::default());
    // The head is the oldest whole segments holding ≥ 25% of the bytes on disk.
    let head = kept.iter().zip(0..).take_while(|(t, i)| **t == *i).count();
    let head_bytes = head as u64 * FRAMED + head.div_ceil(6) as u64 * SEGMENT_HEADER;
    assert!(head_bytes * 4 >= w.disk_bytes(), "head {head} records, {head_bytes} of {} bytes", w.disk_bytes());
    assert!(head >= 18, "more than one segment is pinned (head = {head})");
    assert_eq!(kept.last(), Some(&299));
    assert_eq!(kept.len() as u64 + gaps.iter().map(|g| g.records).sum::<u64>(), 300);
}

#[test]
fn head_tail_keeps_the_oldest_records_and_the_newest() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(Config { overflow: Overflow::HeadTail, ..cfg(tmp.path()) });
    let gaps = fill(&mut w, 0..60);
    let kept = read_times(tmp.path(), Cursor::default());
    // The head (the first segment: 0..6) is pinned through the whole flood...
    assert_eq!(&kept[..6], &[0, 1, 2, 3, 4, 5]);
    // ...the newest records are there...
    assert_eq!(kept.last(), Some(&59));
    // ...and the middle went, each deleted segment reported as a gap.
    let dropped: u64 = gaps.iter().map(|g| g.records).sum();
    assert_eq!(kept.len() as u64 + dropped, 60);
    assert_eq!(gaps[0], Gap { records: 6, first_time: Some(6), last_time: Some(11) });
}

#[test]
fn drop_newest_keeps_the_disk_and_reports_the_dropped_records() {
    let tmp = tempfile::tempdir().unwrap();
    let mut w = open(Config { overflow: Overflow::DropNewest, ..cfg(tmp.path()) });
    // 4 segments (24 records) fit; every later record is dropped, one gap per tick.
    let gaps = fill(&mut w, 0..30);
    assert_eq!(read_times(tmp.path(), Cursor::default()), (0..24).collect::<Vec<_>>());
    assert_eq!(gaps.len(), 6);
    assert_eq!(gaps[0], Gap { records: 1, first_time: Some(24), last_time: Some(24) });
    assert_eq!(w.stats().overflow_drops, 6);

    // Delivering (acking) frees space, and writing resumes.
    let end = {
        let mut r = Reader::open(tmp.path(), Cursor::default());
        std::iter::from_fn(|| r.next_record().unwrap()).last().unwrap().next
    };
    w.ack(end).unwrap();
    write(&mut w, [1000]);
    assert_eq!(read_times(tmp.path(), end), vec![1000]);
}

#[test]
fn config_is_validated() {
    let tmp = tempfile::tempdir().unwrap();
    for bad in [
        Config { cap_bytes: 1000, ..cfg(tmp.path()) },
        Config { segment_bytes: SEGMENT_HEADER, ..cfg(tmp.path()) },
        Config { backlog_bytes: 1024, ..cfg(tmp.path()) },
    ] {
        assert!(Writer::open(bad, time_of).is_err());
    }
}

fn policy() -> impl Strategy<Value = Overflow> {
    prop_oneof![
        Just(Overflow::Retention),
        Just(Overflow::HeadTail),
        Just(Overflow::DropOldest),
        Just(Overflow::DropNewest)
    ]
}

/// A record whose time is `t`, padded to `size` bytes (at least 8).
fn sized(t: i64, size: usize) -> Vec<u8> {
    let mut p = t.to_le_bytes().to_vec();
    p.resize(size.max(8), 0x33);
    p
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    /// Under any policy and record sizes: the disk stays under the cap, what is kept is
    /// in order, every lost record is accounted for in a gap (except under retention,
    /// which reports none), and head-plus-tail never deletes the pinned first segment.
    #[test]
    fn overflow_invariants(policy in policy(), sizes in prop::collection::vec(8usize..200, 1..120)) {
        let tmp = tempfile::tempdir().unwrap();
        let mut w = open(Config { overflow: policy, ..cfg(tmp.path()) });
        let mut gaps = Vec::new();
        let mut first_segment = Vec::new();
        for (t, &size) in sizes.iter().enumerate() {
            w.append(&sized(t as i64, size)).unwrap();
            flush(&mut w);
            gaps.extend(w.take_gaps());
            prop_assert!(w.disk_bytes() <= 1024);
            prop_assert_eq!(dir_bytes(tmp.path()), w.disk_bytes(), "the writer's accounting matches the disk");
            if first_segment.is_empty() && tmp.path().join("00000000000000000002.seg").exists() {
                let mut r = Reader::open(tmp.path(), Cursor::default());
                first_segment = std::iter::from_fn(|| r.next_record().unwrap())
                    .take_while(|rec| rec.at.segment == 1)
                    .map(|rec| time_of(&rec.payload).unwrap())
                    .collect();
            }
        }
        let kept = read_times(tmp.path(), Cursor::default());
        prop_assert!(kept.windows(2).all(|p| p[0] < p[1]));
        let lost: u64 = gaps.iter().map(|g| g.records).sum();
        if policy == Overflow::Retention {
            prop_assert!(gaps.is_empty());
        } else {
            prop_assert_eq!(kept.len() as u64 + lost, sizes.len() as u64);
        }
        if policy == Overflow::HeadTail {
            prop_assert!(first_segment.iter().all(|t| kept.contains(t)), "the pinned head was deleted");
        }
    }
}
