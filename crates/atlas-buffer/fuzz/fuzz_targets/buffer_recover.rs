#![no_main]

use std::path::PathBuf;
use std::sync::OnceLock;

use atlas_buffer::{Config, Cursor, Reader, SEGMENT_HEADER, SEGMENT_MAGIC, Writer, record};
use libfuzzer_sys::fuzz_target;

/// One scratch directory per fuzzing process, emptied before every input.
fn scratch() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("atlas-buffer-fuzz-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    })
}

// Arbitrary bytes as the newest segment file: recovery must never panic; it either
// keeps the file as foreign (and only if it does not start with the segment header),
// or repairs it to exactly its valid prefix, after which a reader sees exactly
// those records.
fuzz_target!(|data: &[u8]| {
    let dir = scratch();
    for entry in std::fs::read_dir(dir).expect("list") {
        std::fs::remove_file(entry.expect("entry").path()).expect("clean");
    }
    let path = dir.join("00000000000000000001.seg");
    std::fs::write(&path, data).expect("write");

    let cfg = Config { segment_bytes: 1 << 20, cap_bytes: 4 << 20, ..Config::new(dir) };
    let (writer, recovery) = Writer::open(cfg, |_| None).expect("recovery handles any bytes");
    drop(writer);

    let header = SEGMENT_HEADER as usize;
    let is_segment = data.len() >= header && data[..header] == SEGMENT_MAGIC;
    if recovery.foreign_segment == Some(1) {
        assert!(!is_segment, "a real segment was called foreign");
        assert_eq!(std::fs::read(&path).expect("read"), data, "a foreign segment is left untouched");
        return;
    }
    assert_eq!(recovery.foreign_segment, None);
    // A repaired torn header is an empty segment: no records.
    let scan = if is_segment { record::scan(data, header) } else { record::scan(&[], 0) };
    let expected_len = if is_segment { scan.valid_end } else { header };
    assert_eq!(std::fs::metadata(&path).expect("stat").len() as usize, expected_len);
    if is_segment {
        assert_eq!(recovery.truncated_bytes as usize, data.len() - scan.valid_end);
    }

    let mut reader = Reader::open(dir, Cursor::default());
    for (start, end) in scan.records {
        let got = reader.next_record().expect("read").expect("a recovered record");
        assert_eq!(got.payload, &data[start..end]);
    }
    assert!(reader.next_record().expect("read").is_none());
    assert_eq!(reader.stats().corrupt_segments, 0);
});
