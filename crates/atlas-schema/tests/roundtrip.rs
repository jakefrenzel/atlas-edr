//! Domain → wire → domain must be lossless for every valid event (spec 8.1).

mod common;

use atlas_schema::{decode_event, encode_event};

#[test]
fn every_sample_round_trips() {
    for (name, event) in common::samples() {
        let decoded = decode_event(&encode_event(event.clone())).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(decoded, event, "{name}");
    }
}
