//! [`Lookups`] on Windows: the device map, live processes and account names
//! (sensor spec §5.3, §5.5). They run on the pipeline thread, so each is cheap:
//! the device map and account names are cached, and an account lookup never
//! waits more than [`ACCOUNT_TIMEOUT`] (plan 1b-3b, D5).

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::ConvertStringSidToSidW;
use windows::Win32::Security::{LookupAccountSidW, PSID, SID_NAME_USE};
use windows::Win32::Storage::FileSystem::QueryDosDeviceW;
use windows::core::{PCWSTR, PWSTR};

use super::telemetry;
use super::util::wide;
use crate::services::{LiveProcess, Lookups};

/// Full rebuild interval of the device map (§5.5).
pub const DEVICE_REFRESH: Duration = Duration::from_secs(60);
/// A lookup miss rebuilds the map at most this often (§5.5).
pub const DEVICE_MISS_REFRESH: Duration = Duration::from_secs(5);
/// How long the pipeline thread waits for an account name (plan 1b-3b, D5).
pub const ACCOUNT_TIMEOUT: Duration = Duration::from_millis(100);
/// A failed account lookup is retried after this long (a domain controller
/// may have been unreachable).
pub const ACCOUNT_FAILURE_TTL: Duration = Duration::from_secs(600);
/// Account cache bound; it is cleared when full (SIDs seen are few).
const ACCOUNT_CAP: usize = 4096;

/// NT device prefix → drive, rebuilt on a timer and on a miss.
pub struct DeviceMap<F> {
    load: F,
    /// (device, drive), longest device first.
    entries: Vec<(String, String)>,
    loaded: Instant,
    missed: Option<Instant>,
}

impl<F: FnMut() -> Vec<(String, String)>> DeviceMap<F> {
    pub fn new(mut load: F, now: Instant) -> Self {
        let entries = sorted(load());
        DeviceMap { load, entries, loaded: now, missed: None }
    }

    /// The drive form of `nt`, or `None` to keep the NT path.
    pub fn dos_path(&mut self, nt: &str, now: Instant) -> Option<String> {
        if now.duration_since(self.loaded) >= DEVICE_REFRESH {
            self.reload(now);
        }
        if let Some(p) = self.find(nt) {
            return Some(p);
        }
        if self.missed.is_none_or(|m| now.duration_since(m) >= DEVICE_MISS_REFRESH) {
            self.missed = Some(now);
            self.reload(now);
            return self.find(nt);
        }
        None
    }

    fn reload(&mut self, now: Instant) {
        self.entries = sorted((self.load)());
        self.loaded = now;
    }

    /// Prefix match on a component boundary, ignoring case (§5.5).
    fn find(&self, nt: &str) -> Option<String> {
        self.entries.iter().find_map(|(dev, drive)| {
            let head = nt.get(..dev.len())?;
            let rest = &nt[dev.len()..];
            (head.eq_ignore_ascii_case(dev) && (rest.is_empty() || rest.starts_with('\\')))
                .then(|| format!("{drive}{rest}"))
        })
    }
}

fn sorted(mut v: Vec<(String, String)>) -> Vec<(String, String)> {
    v.sort_by_key(|(dev, _)| std::cmp::Reverse(dev.len()));
    v
}

/// `QueryDosDeviceW` for every drive letter. Only `\Device\…` targets are kept:
/// a `subst` drive's target is itself a drive path.
pub fn query_drives() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut buf = vec![0u16; 1024];
    for letter in b'A'..=b'Z' {
        let drive = format!("{}:", letter as char);
        let name = wide(&drive);
        // SAFETY: `name` is NUL-terminated; `buf` is writable.
        let n = unsafe { QueryDosDeviceW(PCWSTR(name.as_ptr()), Some(&mut buf)) } as usize;
        let first = buf[..n.min(buf.len())].split(|&u| u == 0).next().unwrap_or(&[]);
        let target = String::from_utf16_lossy(first);
        if n > 0 && target.starts_with(r"\Device\") {
            out.push((target, drive));
        }
    }
    out
}

/// `DOMAIN\name` per SID string, resolved on a helper thread.
pub struct Accounts {
    /// Names and failures, with when they were learned.
    cache: HashMap<String, (Option<String>, Instant)>,
    asked: HashSet<String>,
    tx: Sender<String>,
    rx: Receiver<(String, Option<String>)>,
    timeout: Duration,
    failure_ttl: Duration,
    /// A wait timed out and no answer has come since: the helper is stuck on
    /// a slow lookup, so new SIDs are queued without waiting (R-M4).
    stalled: bool,
}

impl Accounts {
    /// `resolve` runs on the helper thread. A failure is retried after `failure_ttl`.
    pub fn new(
        resolve: impl Fn(&str) -> Option<String> + Send + 'static,
        timeout: Duration,
        failure_ttl: Duration,
    ) -> Self {
        let (tx, jobs) = channel::<String>();
        let (done, rx) = channel();
        std::thread::Builder::new()
            .name("atlas-accounts".into())
            .spawn(move || {
                for sid in jobs {
                    let name = resolve(&sid);
                    if done.send((sid, name)).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn the account lookup thread");
        Accounts { cache: HashMap::new(), asked: HashSet::new(), tx, rx, timeout, failure_ttl, stalled: false }
    }

    /// The cached name, or a lookup waited for up to the timeout. A slow
    /// lookup finishes in the background and serves later calls. While one is
    /// overdue, new lookups are not waited for: the pipeline thread waits at
    /// most one timeout until the helper answers again.
    pub fn name(&mut self, sid: &str) -> Option<String> {
        while let Ok((s, n)) = self.rx.try_recv() {
            self.store(s, n);
        }
        match self.cache.get(sid) {
            Some((n @ Some(_), _)) => return n.clone(),
            Some((None, at)) if at.elapsed() < self.failure_ttl => return None,
            Some((None, _)) => {
                self.cache.remove(sid);
            }
            None => {}
        }
        // Already asked and still running: don't wait a second time.
        if !self.asked.insert(sid.to_string()) || self.tx.send(sid.to_string()).is_err() || self.stalled {
            return None;
        }
        let until = Instant::now() + self.timeout;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left) {
                Ok((s, n)) => {
                    let mine = s == sid;
                    self.store(s, n.clone());
                    if mine {
                        return n;
                    }
                }
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                    self.stalled = true;
                    return None;
                }
            }
        }
    }

    fn store(&mut self, sid: String, name: Option<String>) {
        self.stalled = false;
        if self.cache.len() >= ACCOUNT_CAP {
            self.cache.clear();
        }
        self.asked.remove(&sid);
        self.cache.insert(sid, (name, Instant::now()));
    }
}

/// `LookupAccountSidW` on the local machine: `DOMAIN\name`, or `name` when
/// the domain is empty (e.g. `Everyone`).
pub fn lookup_account(sid: &str) -> Option<String> {
    let s = wide(sid);
    let mut psid = PSID::default();
    // SAFETY: `s` is NUL-terminated; the SID is freed with LocalFree below.
    unsafe { ConvertStringSidToSidW(PCWSTR(s.as_ptr()), &mut psid) }.ok()?;
    let mut name = vec![0u16; 256];
    let mut domain = vec![0u16; 256];
    let (mut n, mut d) = (name.len() as u32, domain.len() as u32);
    let mut use_ = SID_NAME_USE::default();
    // SAFETY: the buffers are writable for the lengths given.
    let r = unsafe {
        LookupAccountSidW(
            PCWSTR::null(),
            psid,
            Some(PWSTR(name.as_mut_ptr())),
            &mut n,
            Some(PWSTR(domain.as_mut_ptr())),
            &mut d,
            &mut use_,
        )
    };
    // SAFETY: allocated by ConvertStringSidToSidW.
    unsafe {
        let _ = LocalFree(Some(HLOCAL(psid.0)));
    }
    r.ok()?;
    let name = String::from_utf16_lossy(&name[..n as usize]);
    let domain = String::from_utf16_lossy(&domain[..d as usize]);
    Some(if domain.is_empty() { name } else { format!(r"{domain}\{name}") })
}

/// Loads the (device, drive) pairs.
type Drives = fn() -> Vec<(String, String)>;

/// The Windows [`Lookups`].
pub struct WinLookups {
    devices: DeviceMap<Drives>,
    accounts: Accounts,
}

impl WinLookups {
    pub fn new() -> Self {
        WinLookups {
            devices: DeviceMap::new(query_drives as Drives, Instant::now()),
            accounts: Accounts::new(lookup_account, ACCOUNT_TIMEOUT, ACCOUNT_FAILURE_TTL),
        }
    }
}

impl Default for WinLookups {
    fn default() -> Self {
        Self::new()
    }
}

impl Lookups for WinLookups {
    fn dos_path(&mut self, nt_path: &str) -> Option<String> {
        self.devices.dos_path(nt_path, Instant::now())
    }

    /// Never cached: PIDs are reused (1b-3a Interfaces).
    fn live_process(&mut self, pid: u32) -> Option<LiveProcess> {
        let t = telemetry::query(pid)?;
        Some(LiveProcess { start_key: t.start_key, image_path: t.image_path?, command_line: t.command_line })
    }

    fn account_name(&mut self, sid: &str) -> Option<String> {
        self.accounts.name(sid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};

    fn counting(entries: Vec<(&str, &str)>) -> (Rc<Cell<u32>>, impl FnMut() -> Vec<(String, String)>) {
        let loads = Rc::new(Cell::new(0));
        let l = loads.clone();
        let e: Vec<(String, String)> = entries.into_iter().map(|(a, b)| (a.into(), b.into())).collect();
        (loads, move || {
            l.set(l.get() + 1);
            e.clone()
        })
    }

    #[test]
    fn device_map_matches_on_component_boundaries() {
        let t = Instant::now();
        let (_, load) = counting(vec![(r"\Device\HarddiskVolume1", "C:"), (r"\Device\HarddiskVolume10", "D:")]);
        let mut m = DeviceMap::new(load, t);
        assert_eq!(m.dos_path(r"\Device\HarddiskVolume1\x", t).as_deref(), Some(r"C:\x"));
        assert_eq!(m.dos_path(r"\device\harddiskvolume10\x", t).as_deref(), Some(r"D:\x"));
        assert_eq!(m.dos_path(r"\Device\HarddiskVolume1", t).as_deref(), Some("C:"));
        assert_eq!(m.dos_path(r"\Device\HarddiskVolume12\x", t), None);
    }

    #[test]
    fn device_map_refreshes_on_a_timer_and_on_misses() {
        let t = Instant::now();
        let (loads, load) = counting(vec![(r"\Device\HarddiskVolume1", "C:")]);
        let mut m = DeviceMap::new(load, t);
        assert_eq!(loads.get(), 1);
        m.dos_path(r"\Device\HarddiskVolume1\x", t + Duration::from_secs(59));
        assert_eq!(loads.get(), 1, "a hit before 60 s does not reload");
        m.dos_path(r"\Device\HarddiskVolume1\x", t + Duration::from_secs(60));
        assert_eq!(loads.get(), 2, "reloaded after 60 s");
        let t = t + Duration::from_secs(60);
        m.dos_path(r"\Device\Mup\x", t);
        assert_eq!(loads.get(), 3, "first miss reloads");
        m.dos_path(r"\Device\Mup\x", t + Duration::from_secs(4));
        assert_eq!(loads.get(), 3, "misses reload at most every 5 s");
        m.dos_path(r"\Device\Mup\x", t + Duration::from_secs(5));
        assert_eq!(loads.get(), 4);
    }

    #[test]
    fn device_map_picks_up_a_new_drive_on_a_miss() {
        let t = Instant::now();
        let drives = Rc::new(Cell::new(1));
        let d = drives.clone();
        let load = move || {
            let mut v = vec![(r"\Device\HarddiskVolume1".to_string(), "C:".to_string())];
            if d.get() > 1 {
                v.push((r"\Device\HarddiskVolume7".to_string(), "E:".to_string()));
            }
            v
        };
        let mut m = DeviceMap::new(load, t);
        drives.set(2);
        assert_eq!(m.dos_path(r"\Device\HarddiskVolume7\f", t).as_deref(), Some(r"E:\f"));
    }

    #[test]
    fn this_machine_has_a_system_drive() {
        let windir = std::env::var("SystemRoot").unwrap();
        let drive = &windir[..2];
        assert!(query_drives().iter().any(|(dev, d)| d.eq_ignore_ascii_case(drive) && dev.starts_with(r"\Device\")));
    }

    #[test]
    fn accounts_are_cached_and_never_wait_long() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let c = calls.clone();
        let mut a = Accounts::new(
            move |sid| {
                c.lock().unwrap().push(sid.to_string());
                if sid == "slow" {
                    std::thread::sleep(Duration::from_millis(300));
                }
                (sid != "none").then(|| format!("D\\{sid}"))
            },
            Duration::from_millis(100),
            Duration::from_secs(600),
        );
        assert_eq!(a.name("S-1").as_deref(), Some(r"D\S-1"));
        assert_eq!(a.name("S-1").as_deref(), Some(r"D\S-1"));
        assert_eq!(a.name("none"), None);
        assert_eq!(a.name("none"), None, "failures are cached too");
        let t = Instant::now();
        assert_eq!(a.name("slow"), None, "a slow lookup times out");
        assert!(t.elapsed() < Duration::from_millis(250));
        let t = Instant::now();
        assert_eq!(a.name("slow"), None, "still running: not asked twice");
        assert!(t.elapsed() < Duration::from_millis(50), "and not waited for twice");
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(a.name("slow").as_deref(), Some(r"D\slow"), "finished in the background");
        assert_eq!(*calls.lock().unwrap(), ["S-1", "none", "slow"]);
    }

    /// R-M4: behind one stuck lookup, new SIDs are not waited for one by one.
    #[test]
    fn one_stuck_lookup_does_not_make_every_new_sid_wait() {
        let mut a = Accounts::new(
            |sid| {
                if sid == "stuck" {
                    std::thread::sleep(Duration::from_millis(500));
                }
                Some(format!("D\\{sid}"))
            },
            Duration::from_millis(100),
            Duration::from_secs(600),
        );
        assert_eq!(a.name("stuck"), None);
        let t = Instant::now();
        for i in 0..10 {
            assert_eq!(a.name(&format!("S-{i}")), None);
        }
        assert!(t.elapsed() < Duration::from_millis(50), "{:?} for ten new SIDs", t.elapsed());
        std::thread::sleep(Duration::from_millis(600));
        assert_eq!(a.name("S-9").as_deref(), Some(r"D\S-9"), "answered in the background");
        assert_eq!(a.name("S-10").as_deref(), Some(r"D\S-10"), "waited for again once the helper answers");
    }

    /// R-m4: a failure is retried after its TTL.
    #[test]
    fn failures_are_retried_after_their_ttl() {
        let calls = Arc::new(Mutex::new(0));
        let c = calls.clone();
        let mut a = Accounts::new(
            move |_| {
                *c.lock().unwrap() += 1;
                None
            },
            Duration::from_millis(100),
            Duration::from_millis(50),
        );
        assert_eq!(a.name("S-1"), None);
        assert_eq!(a.name("S-1"), None);
        assert_eq!(*calls.lock().unwrap(), 1, "cached");
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(a.name("S-1"), None);
        assert_eq!(*calls.lock().unwrap(), 2, "asked again");
    }

    #[test]
    fn well_known_accounts() {
        assert_eq!(lookup_account("S-1-5-18").as_deref(), Some(r"NT AUTHORITY\SYSTEM"));
        assert_eq!(lookup_account("S-1-1-0").as_deref(), Some("Everyone"));
        assert_eq!(lookup_account("not a sid"), None);
    }

    #[test]
    fn live_process_describes_our_own() {
        let mut l = WinLookups::new();
        let me = l.live_process(std::process::id()).expect("own process");
        assert!(me.image_path.ends_with(".exe"), "{}", me.image_path);
        assert!(me.command_line.is_some());
        assert!(l.dos_path(&me.image_path).is_some_and(|p| p.as_bytes()[1] == b':'));
    }
}
