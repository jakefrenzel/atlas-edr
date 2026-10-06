#![no_main]

use atlas_etw::layout::LAYOUTS;
use atlas_etw::parse::{EventMeta, PointerSize, parse, parse_as};
use libfuzzer_sys::fuzz_target;

// Every parser, both pointer sizes, known and newer versions (sensor spec §12.1).
// The first two bytes pick the event, pointer size and version; the rest is the
// payload. Parsing must never panic, and must be deterministic.
fuzz_target!(|data: &[u8]| {
    let [pick, flags, payload @ ..] = data else { return };
    let l = &LAYOUTS[usize::from(*pick) % LAYOUTS.len()];
    let pointer_size = if flags & 1 == 0 { PointerSize::P64 } else { PointerSize::P32 };
    let meta = EventMeta { provider: l.provider, id: l.id, version: l.version.saturating_add(flags >> 6), pointer_size };
    let first = parse(&meta, payload);
    assert_eq!(first, parse(&meta, payload));
    // A newer version parsed with the known layout (after the version check) must not panic either.
    let _ = parse_as(&meta, l.version, payload, flags & 2 != 0);
});
