//! Newer event versions (sensor spec §4.3): the first time a (provider, event,
//! version) newer than our newest layout is seen, ask TDH once for that
//! version's layout **as installed on this machine** and accept it if ours is a
//! prefix of it. The verdict is cached.
//!
//! The installed description is the manifest, or the MOF class for Session B's
//! classic events. It is never read from the event itself: a user-mode provider
//! (DNS-Client) can be forged (§4.4), and a forged event could carry its own
//! schema and so decide a verdict for every later genuine event.

use super::{EventRecord, tdh};
use crate::Provider;
use crate::layout::{self, Field};
use crate::parse::{ParseError, RawEvent, parse, parse_as};
use std::collections::HashMap;

/// What a newer version's installed layout says about ours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Not a prefix, or no installed description: dropped as `unknown_version`.
    Rejected,
    /// Ours is a strict prefix: parsed with our layout; a name that ends our
    /// layout stops at its first NUL, because fields may follow it.
    Prefix,
    /// The same fields: parsed exactly as our version.
    Same,
}

/// The verdict for our layout against a newer version's installed layout.
pub fn verdict(ours: &[Field], installed: Option<&[tdh::TdhField]>) -> Verdict {
    match installed {
        Some(theirs) if layout::is_prefix(ours, theirs) => {
            if theirs.len() == ours.len() {
                Verdict::Same
            } else {
                Verdict::Prefix
            }
        }
        _ => Verdict::Rejected,
    }
}

/// The installed layout of a version, or `None` if Windows has no description of it.
fn installed_layout(provider: Provider, id: u16, version: u8) -> Option<Vec<tdh::TdhField>> {
    match provider {
        Provider::ClassicProcess => tdh::classic_layout(u8::try_from(id).ok()?, version).ok(),
        _ => tdh::manifest_layout(provider, id, version).ok().flatten(),
    }
}

/// Parses live events, deciding once per newer version whether to accept it.
#[derive(Default)]
pub struct VersionGate {
    verdicts: HashMap<(Provider, u16, u8), Verdict>,
}

impl VersionGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parses `rec`. A newer version that failed the prefix check, or whose TDH
    /// layout could not be read, stays `Err(NewerVersion)`: the caller counts it
    /// as `unknown_version` and drops it.
    pub fn parse(&mut self, rec: &EventRecord) -> Result<RawEvent, ParseError> {
        let meta = rec.meta().ok_or(ParseError::UnknownEvent)?;
        match parse(&meta, rec.payload()) {
            Err(ParseError::NewerVersion { version, newest }) => {
                let v = *self.verdicts.entry((meta.provider, meta.id, version)).or_insert_with(|| {
                    let ours = layout::find(meta.provider, meta.id, newest).map(|l| l.fields).unwrap_or_default();
                    verdict(ours, installed_layout(meta.provider, meta.id, version).as_deref())
                });
                match v {
                    Verdict::Same => parse_as(&meta, newest, rec.payload(), true),
                    Verdict::Prefix => parse_as(&meta, newest, rec.payload(), false),
                    Verdict::Rejected => Err(ParseError::NewerVersion { version, newest }),
                }
            }
            other => other,
        }
    }

    /// Every newer version seen so far and its verdict, for logs and Sensor Health.
    pub fn verdicts(&self) -> impl Iterator<Item = ((Provider, u16, u8), Verdict)> + '_ {
        self.verdicts.iter().map(|(k, v)| (*k, *v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Diagnostics::Etw::{EVENT_HEADER_FLAG_64_BIT_HEADER, EVENT_RECORD};
    use windows::core::GUID;

    /// A record as ETW would deliver it, built offline: TDH needs only the header
    /// to find an event's description, so no session or elevation is involved.
    fn record(provider: Provider, id: u16, version: u8, payload: &mut [u8]) -> EVENT_RECORD {
        // SAFETY: an all-zero EVENT_RECORD is valid; the fields used are set below.
        let mut rec: EVENT_RECORD = unsafe { std::mem::zeroed() };
        rec.EventHeader.ProviderId = GUID::from_u128(provider.guid());
        rec.EventHeader.EventDescriptor.Id = id;
        rec.EventHeader.EventDescriptor.Version = version;
        rec.EventHeader.Flags = EVENT_HEADER_FLAG_64_BIT_HEADER as u16;
        rec.UserData = payload.as_mut_ptr().cast();
        rec.UserDataLength = payload.len() as u16;
        rec
    }

    /// Kernel-Registry CloseKey v0: KeyObject, Status, KeyName "".
    fn close_key_payload() -> Vec<u8> {
        let mut b = 0xffff_d001u64.to_le_bytes().to_vec();
        b.extend(0u32.to_le_bytes());
        b.extend([0, 0]);
        b
    }

    #[test]
    fn a_known_version_parses_without_a_verdict() {
        let mut payload = close_key_payload();
        let rec = record(Provider::KernelRegistry, 13, 0, &mut payload);
        let mut gate = VersionGate::new();
        assert!(
            matches!(gate.parse(&EventRecord::from_raw(&rec)), Ok(RawEvent::RegCloseKey(k)) if k.key_object == 0xffff_d001)
        );
        assert_eq!(gate.verdicts().count(), 0);
    }

    #[test]
    fn a_newer_version_unknown_to_tdh_is_rejected_and_remembered() {
        let mut payload = close_key_payload();
        // No manifest has a CloseKey version 9, so TDH cannot describe it.
        let rec = record(Provider::KernelRegistry, 13, 9, &mut payload);
        let mut gate = VersionGate::new();
        for _ in 0..2 {
            assert_eq!(
                gate.parse(&EventRecord::from_raw(&rec)),
                Err(ParseError::NewerVersion { version: 9, newest: 0 })
            );
        }
        assert_eq!(gate.verdicts().collect::<Vec<_>>(), vec![((Provider::KernelRegistry, 13, 9), Verdict::Rejected)]);
    }

    #[test]
    fn the_verdict_uses_the_installed_layout() {
        // A real older/newer pair from the installed manifest: ProcessStart v3 is a
        // strict prefix of v4 (v4 appends SecurityMitigations).
        let v3 = layout::find(Provider::KernelProcess, 1, 3).unwrap().fields;
        let v4 = layout::find(Provider::KernelProcess, 1, 4).unwrap().fields;
        let installed_v4 = installed_layout(Provider::KernelProcess, 1, 4).expect("v4 is in the manifest");
        assert_eq!(verdict(v3, Some(&installed_v4)), Verdict::Prefix);
        assert_eq!(verdict(v4, Some(&installed_v4)), Verdict::Same);
        // v4 is not a prefix of v3, and no description means rejection.
        let installed_v3 = installed_layout(Provider::KernelProcess, 1, 3).unwrap();
        assert_eq!(verdict(v4, Some(&installed_v3)), Verdict::Rejected);
        assert_eq!(verdict(v4, None), Verdict::Rejected);
        assert_eq!(installed_layout(Provider::KernelProcess, 1, 99), None);
        // The classic class is looked up through its MOF description.
        let classic = layout::find(Provider::ClassicProcess, 1, 4).unwrap().fields;
        assert_eq!(verdict(classic, installed_layout(Provider::ClassicProcess, 1, 4).as_deref()), Verdict::Same);
    }

    #[test]
    fn other_providers_are_unknown_events() {
        let mut payload = close_key_payload();
        let mut rec = record(Provider::KernelRegistry, 13, 0, &mut payload);
        rec.EventHeader.ProviderId = GUID::from_u128(0x1234);
        assert_eq!(VersionGate::new().parse(&EventRecord::from_raw(&rec)), Err(ParseError::UnknownEvent));
    }
}
