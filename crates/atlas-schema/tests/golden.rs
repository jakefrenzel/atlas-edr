//! Golden fixtures: one protobuf-JSON file per class/activity in
//! `tests/fixtures/`. They document what events look like on the wire and
//! pin the encoding. Regenerate with `ATLAS_UPDATE_FIXTURES=1 cargo test -p atlas-schema --test golden`
//! and review the diff before committing.

mod common;

use std::path::PathBuf;

use atlas_schema::{decode_event, encode_event};
use prost::Message;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor};

fn event_descriptor() -> MessageDescriptor {
    DescriptorPool::decode(atlas_proto::FILE_DESCRIPTOR_SET)
        .expect("descriptor set decodes")
        .get_message_by_name("atlas.events.v1.Event")
        .expect("Event message exists")
}

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(format!("{name}.json"))
}

fn to_json(bytes: &[u8]) -> String {
    let msg = DynamicMessage::decode(event_descriptor(), bytes).expect("decodes");
    let mut json = serde_json::to_string_pretty(&msg).expect("serializes");
    json.push('\n');
    json
}

fn from_json(json: &str) -> Vec<u8> {
    let mut de = serde_json::Deserializer::from_str(json);
    let msg = DynamicMessage::deserialize(event_descriptor(), &mut de).expect("fixture is valid protobuf-JSON");
    de.end().expect("no trailing data");
    msg.encode_to_vec()
}

#[test]
fn fixtures_match_samples() {
    let update = std::env::var_os("ATLAS_UPDATE_FIXTURES").is_some();
    for (name, event) in common::samples() {
        let path = fixture_path(name);
        if update {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, to_json(&encode_event(event.clone()))).unwrap();
        }
        let json = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("missing {}; run with ATLAS_UPDATE_FIXTURES=1", path.display()));
        let decoded = decode_event(&from_json(&json)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(decoded, event, "{name}: fixture and sample disagree");
    }
}
