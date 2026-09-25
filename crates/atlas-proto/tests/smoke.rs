//! The generated types exist, encode, and decode; the descriptor set is embedded.

use atlas_proto::v1;
use prost::Message;

#[test]
fn event_round_trips_through_bytes() {
    let event = v1::Event {
        event_id: vec![1; 16],
        time: 5,
        sensor: v1::Sensor::Etw as i32,
        device: Some(v1::Device { uid: vec![2; 16], boot_id: vec![3; 16] }),
        kind: Some(v1::event::Kind::Dns(v1::DnsActivity {
            hostname: "example.com".into(),
            query_type: 28,
            ..Default::default()
        })),
    };
    let bytes = event.encode_to_vec();
    assert_eq!(v1::Event::decode(bytes.as_slice()).unwrap(), event);
}

#[test]
fn descriptor_set_is_embedded() {
    let fds = atlas_proto::FILE_DESCRIPTOR_SET;
    assert!(fds.windows(b"atlas.events.v1".len()).any(|w| w == b"atlas.events.v1"));
}
