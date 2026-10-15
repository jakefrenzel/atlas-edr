//! `atlas-agent dump` (sensor spec §11.3; plan 1b-4a, D3): the buffer as JSON
//! lines, one event per line, in protobuf's standard JSON mapping (the same
//! rendering as the replay snapshot). Read-only: it never acks.
//!
//! Every record is validated first (0a's `decode_event`); an invalid one is
//! skipped and counted, never printed (§8.2). The reader follows the §8.5
//! rules, so `dump --follow` can run next to the agent.

use std::io::{self, Write};
use std::path::Path;
use std::str::FromStr;
use std::sync::OnceLock;
use std::time::Duration;

use atlas_buffer::{Cursor, Reader};
use atlas_schema::{EventKind, decode_event};
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor};

/// The classes `--class` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassName {
    Process,
    Module,
    Network,
    File,
    RegistryKey,
    RegistryValue,
    Dns,
    EventLog,
    SensorHealth,
}

impl ClassName {
    pub const ALL: [(&'static str, ClassName); 9] = [
        ("process", ClassName::Process),
        ("module", ClassName::Module),
        ("network", ClassName::Network),
        ("file", ClassName::File),
        ("registry_key", ClassName::RegistryKey),
        ("registry_value", ClassName::RegistryValue),
        ("dns", ClassName::Dns),
        ("event_log", ClassName::EventLog),
        ("sensor_health", ClassName::SensorHealth),
    ];

    pub fn of(kind: &EventKind) -> ClassName {
        match kind {
            EventKind::Process(_) => ClassName::Process,
            EventKind::Module(_) => ClassName::Module,
            EventKind::Network(_) => ClassName::Network,
            EventKind::File(_) => ClassName::File,
            EventKind::RegistryKey(_) => ClassName::RegistryKey,
            EventKind::RegistryValue(_) => ClassName::RegistryValue,
            EventKind::Dns(_) => ClassName::Dns,
            EventKind::EventLog(_) => ClassName::EventLog,
            EventKind::SensorHealth(_) => ClassName::SensorHealth,
        }
    }
}

impl FromStr for ClassName {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        ClassName::ALL.iter().find(|(n, _)| *n == s).map(|(_, c)| *c).ok_or_else(|| {
            let names: Vec<_> = ClassName::ALL.iter().map(|(n, _)| *n).collect();
            format!("unknown class {s:?}; one of {}", names.join(", "))
        })
    }
}

/// `--from <segment:offset>`, as `Record::next` cursors print.
pub fn parse_cursor(s: &str) -> Result<Cursor, String> {
    let (seg, off) = s.split_once(':').ok_or_else(|| format!("{s:?} is not <segment:offset>"))?;
    let n = |v: &str| v.parse::<u64>().map_err(|_| format!("{s:?} is not <segment:offset>"));
    Ok(Cursor { segment: n(seg)?, offset: n(off)? })
}

fn event_descriptor() -> &'static MessageDescriptor {
    static DESC: OnceLock<MessageDescriptor> = OnceLock::new();
    DESC.get_or_init(|| {
        let pool = DescriptorPool::decode(atlas_proto::FILE_DESCRIPTOR_SET).expect("the built-in descriptor set");
        pool.get_message_by_name("atlas.events.v1.Event").expect("atlas.events.v1.Event")
    })
}

/// One encoded event as a line of protobuf JSON. The caller has validated it.
pub fn render(bytes: &[u8]) -> Result<String, String> {
    let msg = DynamicMessage::decode(event_descriptor().clone(), bytes).map_err(|e| e.to_string())?;
    serde_json::to_string(&msg).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Default)]
pub struct DumpOptions {
    pub from: Cursor,
    pub class: Option<ClassName>,
    pub follow: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DumpStats {
    pub printed: u64,
    /// Records that failed validation (`quality.buffer_invalid_records`).
    pub invalid: u64,
    /// Sealed segments whose remainder was skipped as corrupt.
    pub corrupt_segments: u64,
    /// Where a later `--from` continues.
    pub next: Cursor,
}

/// How long `--follow` waits when it has caught up.
const FOLLOW_POLL: Duration = Duration::from_millis(250);

/// Prints the buffer in `dir` to `out`. With `follow`, waits for new records
/// until `stop()` is true.
pub fn dump(
    dir: &Path,
    opts: &DumpOptions,
    out: &mut impl Write,
    mut stop: impl FnMut() -> bool,
) -> io::Result<DumpStats> {
    let mut reader = Reader::open(dir, opts.from);
    let mut stats = DumpStats { next: opts.from, ..DumpStats::default() };
    loop {
        match reader.next_record()? {
            Some(rec) => {
                stats.next = rec.next;
                let Ok(event) = decode_event(&rec.payload) else {
                    stats.invalid += 1;
                    continue;
                };
                if opts.class.is_some_and(|c| c != ClassName::of(&event.kind)) {
                    continue;
                }
                match render(&rec.payload) {
                    Ok(line) => {
                        writeln!(out, "{line}")?;
                        stats.printed += 1;
                    }
                    Err(_) => stats.invalid += 1,
                }
            }
            None if opts.follow && !stop() => {
                out.flush()?;
                std::thread::sleep(FOLLOW_POLL);
            }
            None => break,
        }
    }
    stats.corrupt_segments = reader.stats().corrupt_segments;
    out.flush()?;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::{Snapshot, event, report};
    use atlas_buffer::Writer;
    use atlas_schema::{BootId, Device, DeviceUid, Event, EventId, encode_event};
    use std::time::Instant;

    fn health(time: i64) -> Event {
        let (r, _) = report(&Snapshot::at_start(true, 0), &Snapshot::at_start(true, 0));
        let device = Device { uid: DeviceUid::from_bytes([1; 16]), boot_id: BootId::from_bytes([2; 16]) };
        event(EventId::new_v7(), time, device, 0, r)
    }

    fn buffer(name: &str, records: &[Vec<u8>]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("atlas-dump-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut w, _) = Writer::open(atlas_buffer::Config::new(&dir), |_| None).unwrap();
        for r in records {
            w.append(r).unwrap();
        }
        w.tick(Instant::now()).unwrap();
        w.close().unwrap();
        dir
    }

    #[test]
    fn prints_protobuf_json_lines_and_skips_invalid_records() {
        let dir = buffer("lines", &[encode_event(health(1)), b"not an event".to_vec(), encode_event(health(2))]);
        let mut out = Vec::new();
        let stats = dump(&dir, &DumpOptions::default(), &mut out, || true).unwrap();
        assert_eq!((stats.printed, stats.invalid), (2, 1));
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<serde_json::Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["time"], "1", "int64 as a string, protobuf's mapping");
        assert_eq!(lines[1]["sensor"], "SENSOR_ETW");
        assert!(lines[0]["sensorHealth"]["report"].is_object());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_rendering_is_the_replay_snapshots() {
        let e = health(7);
        let pool = DescriptorPool::decode(atlas_proto::FILE_DESCRIPTOR_SET).unwrap();
        let desc = pool.get_message_by_name("atlas.events.v1.Event").unwrap();
        let msg = DynamicMessage::decode(desc, encode_event(e.clone()).as_slice()).unwrap();
        assert_eq!(render(&encode_event(e)).unwrap(), serde_json::to_string(&msg).unwrap());
    }

    #[test]
    fn class_and_from_select_records() {
        let dir = buffer("select", &[encode_event(health(1)), encode_event(health(2))]);
        let mut out = Vec::new();
        let opts = DumpOptions { class: Some(ClassName::File), ..DumpOptions::default() };
        let all = dump(&dir, &opts, &mut out, || true).unwrap();
        assert_eq!(all.printed, 0);

        // Continue after the first record: from where it ends.
        let mut r = Reader::open(&dir, Cursor::default());
        let first = r.next_record().unwrap().unwrap();
        let opts = DumpOptions { from: first.next, class: Some(ClassName::SensorHealth), ..DumpOptions::default() };
        let mut out = Vec::new();
        let rest = dump(&dir, &opts, &mut out, || true).unwrap();
        assert_eq!(rest.printed, 1);
        assert!(String::from_utf8(out).unwrap().contains("\"time\":\"2\""));
        assert_eq!(rest.next, all.next);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn follow_waits_for_new_records_until_stopped() {
        let dir = buffer("follow", &[encode_event(health(1))]);
        let (mut w, _) = Writer::open(atlas_buffer::Config::new(&dir), |_| None).unwrap();
        let polls = std::cell::Cell::new(0);
        let mut out = Vec::new();
        let opts = DumpOptions { follow: true, ..DumpOptions::default() };
        let stats = dump(&dir, &opts, &mut out, || {
            polls.set(polls.get() + 1);
            if polls.get() == 1 {
                w.append(&encode_event(health(2))).unwrap();
                w.tick(Instant::now()).unwrap();
            }
            polls.get() > 2
        })
        .unwrap();
        assert_eq!(stats.printed, 2);
        w.close().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn options_parse() {
        assert_eq!(parse_cursor("3:128"), Ok(Cursor { segment: 3, offset: 128 }));
        assert!(parse_cursor("3").is_err() && parse_cursor("a:1").is_err() && parse_cursor("1:-2").is_err());
        assert_eq!("registry_value".parse(), Ok(ClassName::RegistryValue));
        let e = "registry".parse::<ClassName>().unwrap_err();
        assert!(e.contains("registry_key"), "{e}");
    }
}
