//! Tier 2 (sensor spec §12.2): replays recorded events through the parsers and
//! compares every field with TDH's decoding from the machine that recorded them.
//!
//! - `tests/fixtures/*.jsonl` are committed. They are recorded on a GitHub
//!   Windows runner by `tests/live.rs` and hold only the scenario's own events.
//! - `ATLAS_ETW_EXTRA_FIXTURES` (directories, separated by `;`) adds local
//!   recordings, such as the host's, that are never committed (§4.3).

mod common;

use atlas_etw::layout;
use atlas_etw::parse::{ParseError, parse};
use common::{Fixture, compare, short_name};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn jsonl_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> =
        std::fs::read_dir(dir).map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect()).unwrap_or_default();
    files.retain(|p| p.extension().is_some_and(|x| x == "jsonl"));
    files.sort();
    files
}

struct Outcome {
    parsed: usize,
    /// (provider, id, version) of every event that parsed and agreed with TDH.
    kinds: BTreeSet<(&'static str, u16, u8)>,
    failures: Vec<String>,
}

/// `strict`: every line of one of our providers must be a readable fixture (the
/// committed recordings). Local recordings from the spike probe hold lines
/// without a raw payload, which are skipped.
fn replay(files: &[PathBuf], strict: bool) -> Outcome {
    let mut out = Outcome { parsed: 0, kinds: BTreeSet::new(), failures: Vec::new() };
    for file in files {
        let bytes = std::fs::read(file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        let text = String::from_utf8_lossy(&bytes);
        for (n, line) in text.lines().enumerate() {
            let at = format!("{}:{}", file.display(), n + 1);
            let Some(f) = Fixture::from_json(line) else {
                if strict && !line.trim().is_empty() {
                    out.failures.push(format!("{at}: not a readable fixture line"));
                }
                continue;
            };
            match parse(&f.meta, &f.payload) {
                Ok(ev) => {
                    let errs = compare(&ev, &f.tdh);
                    if errs.is_empty() {
                        out.parsed += 1;
                        out.kinds.insert((short_name(f.meta.provider), f.meta.id, f.meta.version));
                    } else {
                        out.failures.push(format!("{at}: {}", errs.join("; ")));
                    }
                }
                // Recordings may hold events outside our set (the spike probe recorded everything).
                Err(ParseError::UnknownEvent) => {}
                Err(e) => out.failures.push(format!("{at}: {:?} {e}", f.meta)),
            }
        }
    }
    out
}

#[test]
fn filetime_formats_like_tdh() {
    // 2026-10-02T18:37:27.610206700Z, from a spike S8 ProcessStart.
    let unix = 1_790_966_247u64;
    let ft = (unix + 11_644_473_600) * 10_000_000 + 6_102_067;
    assert_eq!(common::filetime_iso(ft), "2026-10-02T18:37:27.610206700Z");
    assert_eq!(common::filetime_iso(0), "1601-01-01T00:00:00.000000000Z");
}

#[test]
fn committed_fixtures_agree_with_tdh() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let files = jsonl_files(&dir);
    let out = replay(&files, true);
    assert!(out.failures.is_empty(), "{} mismatches:\n{}", out.failures.len(), out.failures.join("\n"));
    // Until the first recording is committed there is nothing to check; after
    // that, a recording that yields no events is a broken recording.
    if !files.is_empty() {
        assert!(out.parsed > 0, "{} fixture file(s) but no events", files.len());
        // The scenario produces every event we parse, at some version.
        let expected: BTreeSet<_> = layout::LAYOUTS.iter().map(|l| (short_name(l.provider), l.id)).collect();
        let seen: BTreeSet<_> = out.kinds.iter().map(|(p, id, _)| (*p, *id)).collect();
        let missing: Vec<_> = expected.difference(&seen).collect();
        assert!(missing.is_empty(), "the fixtures lack {missing:?}");
    }
}

#[test]
fn extra_local_fixtures_agree_with_tdh() {
    let Ok(dirs) = std::env::var("ATLAS_ETW_EXTRA_FIXTURES") else { return };
    let files: Vec<PathBuf> =
        dirs.split(';').filter(|d| !d.is_empty()).flat_map(|d| jsonl_files(Path::new(d))).collect();
    let out = replay(&files, false);
    eprintln!("replayed {} events from {} files; kinds: {:?}", out.parsed, files.len(), out.kinds);
    assert!(out.failures.is_empty(), "{} mismatches:\n{}", out.failures.len(), out.failures.join("\n"));
}
