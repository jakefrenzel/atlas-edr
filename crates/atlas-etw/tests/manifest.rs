//! Our layout tables against the installed provider manifests (sensor spec
//! §4.3). Needs Windows, not elevation: TDH reads the manifests directly.
#![cfg(windows)]

use atlas_etw::Provider;
use atlas_etw::layout::LAYOUTS;
use atlas_etw::session::tdh::{classic_layout, manifest_layout};

#[test]
fn every_manifest_layout_matches_the_installed_manifest() {
    let mut failures = Vec::new();
    for l in LAYOUTS.iter().filter(|l| l.provider != Provider::ClassicProcess) {
        let ours: Vec<(String, u16)> = l.fields.iter().map(|(n, t)| (n.to_string(), *t as u16)).collect();
        match manifest_layout(l.provider, l.id, l.version) {
            Ok(Some(theirs)) if theirs == ours => {}
            Ok(Some(theirs)) => failures
                .push(format!("{:?} {} v{}:\n  ours   {ours:?}\n  theirs {theirs:?}", l.provider, l.id, l.version)),
            // A build whose manifest lacks this version (older Windows) logs another
            // version, which the replay and live tests cover.
            Ok(None) => eprintln!("{:?} {} v{}: not in this build's manifest", l.provider, l.id, l.version),
            Err(e) => failures.push(format!("{:?} {} v{}: {e}", l.provider, l.id, l.version)),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn the_classic_process_layout_matches_its_mof_class() {
    for l in LAYOUTS.iter().filter(|l| l.provider == Provider::ClassicProcess) {
        let ours: Vec<(String, u16)> = l.fields.iter().map(|(n, t)| (n.to_string(), *t as u16)).collect();
        let theirs = classic_layout(l.id as u8, l.version).unwrap_or_else(|e| panic!("opcode {}: {e}", l.id));
        assert_eq!(theirs, ours, "opcode {} v{}", l.id, l.version);
    }
}
