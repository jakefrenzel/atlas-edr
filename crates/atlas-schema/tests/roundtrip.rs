//! Domain → wire → domain must be lossless for every valid event (spec 8.1).

mod common;

use atlas_schema::{decode_event, encode_event};
use proptest::prelude::*;

#[test]
fn every_sample_round_trips() {
    for (name, event) in common::samples() {
        let decoded = decode_event(&encode_event(event.clone())).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(decoded, event, "{name}");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn arbitrary_valid_events_round_trip(event in common::arb_event()) {
        let decoded = decode_event(&encode_event(event.clone())).expect("valid event must decode");
        prop_assert_eq!(decoded, event);
    }
}
