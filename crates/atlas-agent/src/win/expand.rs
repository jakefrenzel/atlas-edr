//! 8.3 short-name expansion (sensor spec §7.2; plan 1b-3b, D4).
//!
//! `GetLongPathNameW` rejects `\\?\GLOBALROOT\Device\…` paths, so each short
//! component is looked up in its parent directory, opened by NT path, with
//! `NtQueryDirectoryFile` and the short name as the filter. That works for
//! shadow copies too. Only components that look short are looked up
//! (~54 µs each on the host).
//!
//! Results are cached by (parent directory, short name). Short names are
//! reused after a delete or rename, so the pipeline's `InvalidateHash` for a
//! path also drops the entries for that path and everything under it, and an
//! entry expires after [`TTL`] in case a change was never seen.

use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;
use std::time::{Duration, Instant};

use windows::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows::Wdk::Storage::FileSystem::{FileBothDirectoryInformation, NtOpenFile, NtQueryDirectoryFile};
use windows::Win32::Foundation::{HANDLE, OBJ_CASE_INSENSITIVE};
use windows::Win32::System::IO::IO_STATUS_BLOCK;

use super::util::{Owned, aligned, as_bytes, counted, u32_at};
use crate::paths::is_short_name as is_short_component;

pub(crate) const TTL: Duration = Duration::from_secs(60);
const CAP: usize = 16_384;

/// Directory lookups, behind a trait so the cache can be tested without disks.
pub(crate) trait Dir {
    /// The long name of `component` in the directory `dir` (an NT path).
    fn long_name(&mut self, dir: &str, component: &str) -> Option<String>;
}

/// (parent, lowercased) + (short name, uppercased).
type Key = (String, String);

pub(crate) struct Expander<D> {
    dir: D,
    cache: BTreeMap<Key, (String, Instant)>,
    /// Lowercased full long path of a cached directory entry → its key.
    by_long: HashMap<String, Key>,
}

impl<D: Dir> Expander<D> {
    pub(crate) fn new(dir: D) -> Self {
        Expander { dir, cache: BTreeMap::new(), by_long: HashMap::new() }
    }

    /// The long form of `nt_path`, or `None` if a short component cannot be
    /// expanded. Only `\Device\HarddiskVolume…` paths (volumes and shadow
    /// copies) are expanded: a network redirector could stall the expander lane.
    pub(crate) fn expand(&mut self, nt_path: &str, now: Instant) -> Option<String> {
        let (device, rest) = split_device(nt_path)?;
        if !device.to_ascii_lowercase().starts_with(r"\device\harddiskvolume") {
            return None;
        }
        let mut out = device.to_string();
        for comp in rest.split('\\').filter(|c| !c.is_empty()) {
            let long = if is_short_component(comp) { self.component(&out, comp, now)? } else { comp.to_string() };
            out.push('\\');
            out.push_str(&long);
        }
        if rest.ends_with('\\') {
            out.push('\\');
        }
        Some(out)
    }

    fn component(&mut self, parent: &str, short: &str, now: Instant) -> Option<String> {
        let key = (parent.to_lowercase(), short.to_uppercase());
        if let Some((long, at)) = self.cache.get(&key) {
            if now.duration_since(*at) < TTL {
                return Some(long.clone());
            }
            self.remove(&key);
        }
        let long = self.dir.long_name(parent, short)?;
        if self.cache.len() >= CAP {
            self.cache.clear();
            self.by_long.clear();
        }
        self.by_long.insert(format!(r"{}\{}", key.0, long.to_lowercase()), key.clone());
        self.cache.insert(key, (long.clone(), now));
        Some(long)
    }

    fn remove(&mut self, key: &Key) {
        if let Some((long, _)) = self.cache.remove(key) {
            self.by_long.remove(&format!(r"{}\{}", key.0, long.to_lowercase()));
        }
    }

    /// Forgets everything (a lost invalidation, R-M3).
    pub(crate) fn clear(&mut self) {
        self.cache.clear();
        self.by_long.clear();
    }

    /// Something at `nt_path` changed (deleted, renamed away, written): forget
    /// what the cache knows about it and below it. Short components in
    /// `nt_path` are resolved from the cache only; one the cache cannot
    /// resolve could stand for any directory, so the whole cache is cleared.
    pub(crate) fn invalidate(&mut self, nt_path: &str) {
        let Some((device, rest)) = split_device(nt_path) else { return };
        let mut long = device.to_lowercase();
        for comp in rest.split('\\').filter(|c| !c.is_empty()) {
            if !is_short_component(comp) {
                long = format!(r"{long}\{}", comp.to_lowercase());
                continue;
            }
            match self.cache.get(&(long.clone(), comp.to_uppercase())) {
                Some((l, _)) => long = format!(r"{long}\{}", l.to_lowercase()),
                None => {
                    self.clear();
                    return;
                }
            }
        }
        if let Some(key) = self.by_long.get(&long).cloned() {
            self.remove(&key);
        }
        // Entries whose parent is the path, or lies under it.
        let below = format!(r"{long}\");
        let doomed: Vec<Key> = self
            .cache
            .range((Bound::Included((long.clone(), String::new())), Bound::Unbounded))
            .map(|(k, _)| k)
            .take_while(|k| k.0 == long || k.0.starts_with(&long))
            .filter(|k| k.0 == long || k.0.starts_with(&below))
            .cloned()
            .collect();
        for k in doomed {
            self.remove(&k);
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.cache.len()
    }
}

/// `\Device\X` and the rest (which starts with `\`, or is empty).
fn split_device(nt: &str) -> Option<(&str, &str)> {
    const DEVICE: &str = r"\Device\";
    if !nt.get(..DEVICE.len())?.eq_ignore_ascii_case(DEVICE) {
        return None;
    }
    let end = nt[DEVICE.len()..].find('\\').map_or(nt.len(), |i| i + DEVICE.len());
    Some((&nt[..end], &nt[end..]))
}

/// The real directory lookup.
pub(crate) struct NtDir;

const FILE_LIST_DIRECTORY: u32 = 0x1;
const SYNCHRONIZE: u32 = 0x10_0000;
const FILE_SHARE_ALL: u32 = 0x7;
const FILE_DIRECTORY_FILE: u32 = 0x1;
const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x20;
const FILE_OPEN_FOR_BACKUP_INTENT: u32 = 0x4000;

impl Dir for NtDir {
    fn long_name(&mut self, dir: &str, component: &str) -> Option<String> {
        // A volume's root directory needs its trailing backslash.
        let dir = if split_device(dir)?.1.is_empty() { format!(r"{dir}\") } else { dir.to_string() };
        let mut d: Vec<u16> = dir.encode_utf16().collect();
        let name = counted(&mut d)?;
        let oa = OBJECT_ATTRIBUTES {
            Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
            ObjectName: &name,
            Attributes: OBJ_CASE_INSENSITIVE,
            ..Default::default()
        };
        let mut h = HANDLE::default();
        let mut iosb = IO_STATUS_BLOCK::default();
        // SAFETY: `oa` and its name outlive the call; the handle is owned below.
        unsafe {
            NtOpenFile(
                &mut h,
                FILE_LIST_DIRECTORY | SYNCHRONIZE,
                &oa,
                &mut iosb,
                FILE_SHARE_ALL,
                FILE_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_FOR_BACKUP_INTENT,
            )
        }
        .ok()
        .ok()?;
        let h = Owned(h);
        let mut c: Vec<u16> = component.encode_utf16().collect();
        let filter = counted(&mut c)?;
        let mut buf = aligned(4096);
        // SAFETY: `buf` is writable for its length; one entry, from the start.
        unsafe {
            NtQueryDirectoryFile(
                h.raw(),
                None,
                None,
                None,
                &mut iosb,
                buf.as_mut_ptr().cast(),
                (buf.len() * 8) as u32,
                FileBothDirectoryInformation,
                true,
                Some(&filter),
                true,
            )
        }
        .ok()
        .ok()?;
        // FILE_BOTH_DIR_INFORMATION: FileNameLength @60, ShortNameLength (i8) @68,
        // ShortName [u16; 12] @70, FileName @94.
        let b = as_bytes(&buf);
        let name_len = u32_at(b, 60)? as usize;
        let short_len = (*b.get(68)? as usize).min(24);
        let utf16 = |bytes: &[u8]| {
            String::from_utf16_lossy(
                &bytes.chunks_exact(2).map(|x| u16::from_le_bytes([x[0], x[1]])).collect::<Vec<_>>(),
            )
        };
        let long = utf16(b.get(94..94 + name_len)?);
        let short = utf16(b.get(70..70 + short_len)?);
        // The filter matches either name; accept only an exact match of one.
        (short.eq_ignore_ascii_case(component) || long.eq_ignore_ascii_case(component)).then_some(long)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// A fake directory tree: (parent lowercased, short uppercased) → long; counts lookups.
    type Tree = (HashMap<(String, String), String>, u32);

    #[derive(Clone, Default)]
    struct FakeDir(Rc<RefCell<Tree>>);

    impl FakeDir {
        fn put(&self, parent: &str, short: &str, long: &str) {
            self.0.borrow_mut().0.insert((parent.to_lowercase(), short.to_uppercase()), long.into());
        }
        fn lookups(&self) -> u32 {
            self.0.borrow().1
        }
    }

    impl Dir for FakeDir {
        fn long_name(&mut self, dir: &str, c: &str) -> Option<String> {
            let mut s = self.0.borrow_mut();
            s.1 += 1;
            s.0.get(&(dir.to_lowercase(), c.to_uppercase())).cloned()
        }
    }

    const V: &str = r"\Device\HarddiskVolume3";

    fn tree() -> FakeDir {
        let d = FakeDir::default();
        d.put(V, "SECRET~1", "SecretStuffAAA");
        d.put(&format!(r"{V}\SecretStuffAAA"), "LONGFI~1.TXT", "long file.txt");
        d
    }

    #[test]
    fn expands_short_components_only_and_caches() {
        let d = tree();
        let mut e = Expander::new(d.clone());
        let t = Instant::now();
        let p = format!(r"{V}\SECRET~1\LONGFI~1.TXT");
        assert_eq!(e.expand(&p, t), Some(format!(r"{V}\SecretStuffAAA\long file.txt")));
        assert_eq!(d.lookups(), 2);
        let lower = e.expand(&p.to_lowercase(), t).unwrap();
        assert_eq!(lower, format!(r"{}\SecretStuffAAA\long file.txt", V.to_lowercase()), "the device as given");
        assert_eq!(d.lookups(), 2, "cached");
        assert_eq!(e.expand(&format!(r"{V}\Plain\name.txt"), t), Some(format!(r"{V}\Plain\name.txt")));
        assert_eq!(d.lookups(), 2, "long components are never looked up");
        assert_eq!(e.expand(&format!(r"{V}\NOPE~1\x"), t), None);
        assert_eq!(e.expand(&format!(r"{V}\"), t), Some(format!(r"{V}\")));
    }

    #[test]
    fn device_prefix_in_any_case() {
        assert_eq!(split_device(r"\DEVICE\HarddiskVolume3\x"), Some((r"\DEVICE\HarddiskVolume3", r"\x")));
        assert_eq!(split_device(r"\device\HarddiskVolume3"), Some((r"\device\HarddiskVolume3", "")));
        assert_eq!(split_device(r"\Devices\x"), None);
        assert_eq!(split_device(r"C:\x"), None);
    }

    #[test]
    fn only_local_volumes() {
        let d = FakeDir::default();
        let mut e = Expander::new(d.clone());
        assert_eq!(e.expand(r"\Device\Mup\server\SHARE~1\x", Instant::now()), None);
        assert_eq!(d.lookups(), 0);
        let shadow = r"\Device\HarddiskVolumeShadowCopy1";
        d.put(shadow, "USERDA~1", "User Data");
        assert_eq!(e.expand(&format!(r"{shadow}\USERDA~1"), Instant::now()), Some(format!(r"{shadow}\User Data")));
    }

    #[test]
    fn entries_expire() {
        let d = tree();
        let mut e = Expander::new(d.clone());
        let t = Instant::now();
        e.expand(&format!(r"{V}\SECRET~1"), t);
        e.expand(&format!(r"{V}\SECRET~1"), t + TTL - Duration::from_millis(1));
        assert_eq!(d.lookups(), 1);
        e.expand(&format!(r"{V}\SECRET~1"), t + TTL);
        assert_eq!(d.lookups(), 2);
    }

    /// The reuse case measured on the host (D4): delete a directory, and the
    /// next new one gets its short name.
    #[test]
    fn invalidation_by_long_path_covers_the_entry_and_below() {
        let d = tree();
        let mut e = Expander::new(d.clone());
        let t = Instant::now();
        e.expand(&format!(r"{V}\SECRET~1\LONGFI~1.TXT"), t);
        assert_eq!(e.len(), 2);
        e.invalidate(&format!(r"{V}\SecretStuffAAA"));
        assert_eq!(e.len(), 0, "the directory and the entry inside it");
        d.put(V, "SECRET~1", "SecretStuffBBB");
        assert_eq!(e.expand(&format!(r"{V}\SECRET~1"), t), Some(format!(r"{V}\SecretStuffBBB")));
    }

    #[test]
    fn invalidation_by_short_path() {
        let d = tree();
        let mut e = Expander::new(d.clone());
        let t = Instant::now();
        e.expand(&format!(r"{V}\SECRET~1\LONGFI~1.TXT"), t);
        e.invalidate(&format!(r"{V}\secret~1\longfi~1.txt"));
        assert_eq!(e.len(), 1, "only the file's entry");
        e.invalidate(&format!(r"{V}\SECRET~1"));
        assert_eq!(e.len(), 0);
    }

    /// R-m1: a directory invalidated through its short name takes its subtree with it.
    #[test]
    fn invalidation_by_short_path_covers_the_subtree() {
        let d = tree();
        d.put(V, "SIBLIN~1", "Sibling Directory");
        let mut e = Expander::new(d.clone());
        let t = Instant::now();
        e.expand(&format!(r"{V}\SECRET~1\LONGFI~1.TXT"), t);
        e.expand(&format!(r"{V}\SIBLIN~1"), t);
        assert_eq!(e.len(), 3);
        e.invalidate(&format!(r"{V}\SECRET~1"));
        assert_eq!(e.len(), 1, "the directory and the file below it; the sibling stays");
    }

    /// A short component the cache cannot resolve could be any directory.
    #[test]
    fn an_unresolvable_short_component_clears_the_cache() {
        let d = tree();
        let mut e = Expander::new(d.clone());
        let t = Instant::now();
        e.expand(&format!(r"{V}\SECRET~1\LONGFI~1.TXT"), t);
        e.invalidate(&format!(r"{V}\SecretStuffAAA\x.txt"));
        assert_eq!(e.len(), 2, "a long path resolves without the cache: nothing else is touched");
        e.invalidate(&format!(r"{V}\UNSEEN~1\x"));
        assert_eq!(e.len(), 0);
    }

    #[test]
    fn invalidation_leaves_siblings_and_lookalikes() {
        let d = tree();
        d.put(V, "SECRET~2", "SecretStuffAAA-2");
        d.put(&format!(r"{V}\SecretStuffAAA-2"), "OTHERF~1", "other file");
        let mut e = Expander::new(d.clone());
        let t = Instant::now();
        e.expand(&format!(r"{V}\SECRET~1\LONGFI~1.TXT"), t);
        e.expand(&format!(r"{V}\SECRET~2\OTHERF~1"), t);
        assert_eq!(e.len(), 4);
        e.invalidate(&format!(r"{V}\SecretStuffAAA"));
        assert_eq!(e.len(), 2, "SecretStuffAAA-2 and its child stay");
        e.invalidate(&format!(r"{V}\unrelated.txt"));
        assert_eq!(e.len(), 2);
    }

    #[test]
    fn real_directories() {
        let dir = std::env::temp_dir().join(format!("atlas-expand-{}", std::process::id()));
        let deep = dir.join("Long Directory Alpha").join("a-long-file-name.txt");
        std::fs::create_dir_all(deep.parent().unwrap()).unwrap();
        std::fs::write(&deep, b"x").unwrap();
        let devices = super::super::lookups::query_drives();
        let long = deep.to_string_lossy().to_string();
        let (dev, drive) = devices.iter().find(|(_, d)| long[..2].eq_ignore_ascii_case(d)).unwrap();
        let short = short_path(&long);
        // On the runner TEMP is already `RUNNER~1`: check the new directory's own short name.
        assert!(
            short.split('\\').any(|c| c.to_ascii_uppercase().starts_with("LONGDI~")),
            "8.3 names are made on this volume: {short}"
        );
        let mut e = Expander::new(NtDir);
        let got = e.expand(&format!("{dev}{}", &short[drive.len()..]), Instant::now());
        // The temp directory itself may be spelled short in TEMP; compare the long forms.
        assert_eq!(got.map(|g| g.to_lowercase()), Some(format!("{dev}{}", &long_path(&long)[2..]).to_lowercase()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn short_path(p: &str) -> String {
        use windows::Win32::Storage::FileSystem::GetShortPathNameW;
        let w = super::super::util::wide(p);
        let mut buf = vec![0u16; 1024];
        // SAFETY: test-only; NUL-terminated input, writable output.
        let n = unsafe { GetShortPathNameW(windows::core::PCWSTR(w.as_ptr()), Some(&mut buf)) } as usize;
        String::from_utf16_lossy(&buf[..n])
    }

    fn long_path(p: &str) -> String {
        use windows::Win32::Storage::FileSystem::GetLongPathNameW;
        let w = super::super::util::wide(p);
        let mut buf = vec![0u16; 1024];
        // SAFETY: as above.
        let n = unsafe { GetLongPathNameW(windows::core::PCWSTR(w.as_ptr()), Some(&mut buf)) } as usize;
        String::from_utf16_lossy(&buf[..n])
    }
}
