use std::{env, fs, path::PathBuf};

const PROTO_ROOT: &str = "proto";
const PROTO_FILES: &[&str] = &[
    "atlas/events/v1/objects.proto",
    "atlas/events/v1/process.proto",
    "atlas/events/v1/module.proto",
    "atlas/events/v1/network.proto",
    "atlas/events/v1/file.proto",
    "atlas/events/v1/registry.proto",
    "atlas/events/v1/dns.proto",
    "atlas/events/v1/event.proto",
];

fn main() {
    println!("cargo:rerun-if-changed={PROTO_ROOT}");

    // protox is a pure-Rust protobuf compiler: no protoc binary needed.
    let fds = protox::compile(PROTO_FILES, [PROTO_ROOT]).expect("failed to compile .proto files");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
    // Keep the descriptor set so tests can do protobuf-JSON via prost-reflect.
    fs::write(out_dir.join("atlas_events_v1.fds.bin"), prost::Message::encode_to_vec(&fds))
        .expect("failed to write descriptor set");

    prost_build::Config::new().compile_fds(fds).expect("failed to generate Rust code");
}
