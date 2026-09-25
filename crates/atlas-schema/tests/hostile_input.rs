//! Stable-Rust fuzzing: arbitrary and corrupted bytes must never panic.
//! (The cargo-fuzz target in `fuzz/` does the same with coverage guidance.)

mod common;

use atlas_schema::{decode_event, encode_event};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(5000))]

    #[test]
    fn random_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        let _ = decode_event(&bytes);
    }

    #[test]
    fn corrupted_valid_events_never_panic(
        event in common::arb_event(),
        flips in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..8),
    ) {
        let mut bytes = encode_event(event);
        for (at, value) in flips {
            let i = at.index(bytes.len());
            bytes[i] = value;
        }
        let _ = decode_event(&bytes);
    }

    #[test]
    fn truncated_valid_events_never_panic(event in common::arb_event(), cut in any::<prop::sample::Index>()) {
        let bytes = encode_event(event);
        let _ = decode_event(&bytes[..cut.index(bytes.len())]);
    }
}
