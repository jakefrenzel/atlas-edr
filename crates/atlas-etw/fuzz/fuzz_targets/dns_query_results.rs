#![no_main]

use atlas_etw::parse::{DnsAnswers, parse_query_results};
use libfuzzer_sys::fuzz_target;

// DNS-Client 3008's QueryResults text (sensor spec §5.4): arbitrary UTF-16 never
// panics and never yields more than 0a's 64 answers.
fuzz_target!(|data: &[u8]| {
    let units: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let a = parse_query_results(&units);
    assert!(a.answers.len() <= DnsAnswers::MAX);
    assert!(!a.truncated || a.answers.len() == DnsAnswers::MAX);
});
