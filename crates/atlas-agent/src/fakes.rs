//! Stand-ins for Windows, for tests and the full-pipeline replay: lookups from
//! tables, and event ids that depend only on the event time and order.

use std::collections::HashMap;
use std::sync::Mutex;

use atlas_schema::EventId;

use crate::driver::Lanes;
use crate::pipeline::IdGen;
use crate::services::{LiveProcess, Lookups, Reply, Request};

/// [`Lookups`] answered from tables.
#[derive(Debug, Clone, Default)]
pub struct FakeLookups {
    /// (NT device prefix, drive) pairs, such as (`\Device\HarddiskVolume3`, `C:`).
    pub devices: Vec<(String, String)>,
    pub live: HashMap<u32, LiveProcess>,
    pub accounts: HashMap<String, String>,
}

impl FakeLookups {
    pub fn with_device(mut self, nt: &str, drive: &str) -> Self {
        self.devices.push((nt.to_string(), drive.to_string()));
        self
    }
}

impl Lookups for FakeLookups {
    /// Prefix match on a component boundary, ignoring case (§5.5):
    /// `HarddiskVolume1` never matches `HarddiskVolume10\…`.
    fn dos_path(&mut self, nt: &str) -> Option<String> {
        self.devices.iter().find_map(|(dev, drive)| {
            let head = nt.get(..dev.len())?;
            let rest = &nt[dev.len()..];
            (head.eq_ignore_ascii_case(dev) && (rest.is_empty() || rest.starts_with('\\')))
                .then(|| format!("{drive}{rest}"))
        })
    }

    fn live_process(&mut self, pid: u32) -> Option<LiveProcess> {
        self.live.get(&pid).cloned()
    }

    fn account_name(&mut self, sid: &str) -> Option<String> {
        self.accounts.get(sid).cloned()
    }
}

/// How [`FakeLanes`] answers a request, if at all.
type Answer = Box<dyn Fn(&Request) -> Option<Reply> + Send>;

/// Services that answer every request at once, through `answer`: the reply
/// is collected on the next pass, as a real lane's would be at the earliest.
pub struct FakeLanes {
    answer: Answer,
    replies: Mutex<Vec<Reply>>,
    /// Every request submitted, in order.
    pub submitted: Mutex<Vec<Request>>,
}

impl FakeLanes {
    pub fn new(answer: impl Fn(&Request) -> Option<Reply> + Send + 'static) -> Self {
        FakeLanes { answer: Box::new(answer), replies: Mutex::default(), submitted: Mutex::default() }
    }
}

impl Lanes for FakeLanes {
    fn submit(&self, r: Request) {
        if let Some(reply) = (self.answer)(&r) {
            self.replies.lock().expect("replies").push(reply);
        }
        self.submitted.lock().expect("submitted").push(r);
    }

    fn replies(&self) -> Vec<Reply> {
        std::mem::take(&mut *self.replies.lock().expect("replies"))
    }
}

/// UUIDv7 ids from the event's time (milliseconds) and a counter, so a replay
/// gives the same ids every run.
pub fn sequential_ids() -> IdGen {
    let mut n: u64 = 0;
    Box::new(move |unix_ns: i64| {
        n += 1;
        let millis = u64::try_from(unix_ns.max(0) / 1_000_000).unwrap_or(0);
        let mut rand = [0u8; 10];
        rand[2..].copy_from_slice(&n.to_be_bytes());
        let uuid = uuid::Builder::from_unix_timestamp_millis(millis, &rand).into_uuid();
        EventId::from_uuid(uuid).expect("a v7 uuid")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_prefixes_match_on_component_boundaries() {
        let mut l = FakeLookups::default().with_device(r"\Device\HarddiskVolume1", "C:");
        assert_eq!(l.dos_path(r"\Device\HarddiskVolume1\x"), Some(r"C:\x".into()));
        assert_eq!(l.dos_path(r"\device\harddiskvolume1"), Some("C:".into()));
        assert_eq!(l.dos_path(r"\Device\HarddiskVolume10\x"), None);
    }

    #[test]
    fn ids_are_v7_and_repeatable() {
        let (mut a, mut b) = (sequential_ids(), sequential_ids());
        let x = a(1_790_000_000_123_456_789);
        assert_eq!(x, b(1_790_000_000_123_456_789));
        assert_ne!(x, a(1_790_000_000_123_456_789));
    }
}
