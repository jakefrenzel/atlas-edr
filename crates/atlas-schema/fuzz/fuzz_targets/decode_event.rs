#![no_main]

use libfuzzer_sys::fuzz_target;

// Untrusted bytes → decode + validate must never panic, hang, or blow up memory.
fuzz_target!(|data: &[u8]| {
    let _ = atlas_schema::decode_event(data);
});
