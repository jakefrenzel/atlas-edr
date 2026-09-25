//! Baseline throughput for encode, decode+validate, and raw protobuf decode
//! over one sample of every class/activity. Not a gate (spec 8.1).

#[path = "../tests/common/mod.rs"]
mod common;

use std::hint::black_box;

use atlas_schema::{decode_event, encode_event, wire};
use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use prost::Message;

fn codec(c: &mut Criterion) {
    let events: Vec<_> = common::samples().into_iter().map(|(_, e)| e).collect();
    let encoded: Vec<Vec<u8>> = events.iter().cloned().map(encode_event).collect();

    let mut group = c.benchmark_group("codec");
    group.throughput(Throughput::Elements(events.len() as u64));

    group.bench_function("encode", |b| {
        b.iter_batched(
            || events.clone(),
            |events| {
                for e in events {
                    black_box(encode_event(e));
                }
            },
            BatchSize::SmallInput,
        )
    });

    group.bench_function("decode_and_validate", |b| {
        b.iter(|| {
            for bytes in &encoded {
                black_box(decode_event(black_box(bytes)).expect("valid"));
            }
        })
    });

    group.bench_function("protobuf_decode_only", |b| {
        b.iter(|| {
            for bytes in &encoded {
                black_box(wire::Event::decode(black_box(bytes.as_slice())).expect("valid"));
            }
        })
    });

    group.finish();
}

criterion_group!(benches, codec);
criterion_main!(benches);
