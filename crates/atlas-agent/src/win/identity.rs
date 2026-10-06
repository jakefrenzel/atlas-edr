//! Device and boot identity, the clock anchor, and the other start-up facts
//! the pipeline's `Setup` needs (sensor spec §3.3, §5.5, §6.1).

use std::fmt;
use std::io::Write;
use std::path::Path;

use atlas_schema::{BootId, DeviceUid};
use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SYSTEM_INFORMATION_CLASS};
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows::core::w;

use super::telemetry;
use super::util::{aligned, as_bytes, filetime_to_unix_ns, qpc_frequency, qpc_now, u32_at, u64_at};
use crate::config::Ticks;
use crate::process::Identity;
use crate::time::Anchor;

/// What the agent learns about itself and the machine at start.
#[derive(Debug, Clone)]
pub struct Started {
    pub identity: Identity,
    pub ticks: Ticks,
    pub anchor: Anchor,
    /// `HKLM\SYSTEM\Select\Current` (§5.5).
    pub current_control_set: u32,
    /// The agent's own start key (§5.5 self-filter).
    pub self_key: u64,
    /// QPC at start.
    pub started: i64,
    /// `KUSER_SHARED_DATA.BootId` when it disagreed with the kernel's
    /// telemetry value, which is then used (§6.1). The agent logs it.
    pub boot_id_disagreement: Option<u32>,
}

#[derive(Debug)]
pub enum IdentityError {
    /// `device.json` could not be read or created, or is not valid.
    DeviceFile(String),
    /// The agent's own `PROCESS_TELEMETRY_ID_INFORMATION`.
    Telemetry,
    /// The System process's creation time.
    BootTime,
    /// `HKLM\SYSTEM\Select\Current`.
    ControlSet,
}

impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdentityError::DeviceFile(e) => write!(f, "device identity file: {e}"),
            IdentityError::Telemetry => f.write_str("the agent's own process telemetry could not be read"),
            IdentityError::BootTime => f.write_str("the System process's creation time could not be read"),
            IdentityError::ControlSet => f.write_str(r"HKLM\SYSTEM\Select\Current could not be read"),
        }
    }
}

impl std::error::Error for IdentityError {}

/// Gathers the start-up facts. `device_file` is `device.json` (§6.1); the
/// install location and its permissions are plan 1b-4's.
pub fn start(device_file: &Path) -> Result<Started, IdentityError> {
    let device = device_uid(device_file)?;
    let me = telemetry::current().ok_or(IdentityError::Telemetry)?;
    let kusd = kusd_boot_id();
    // The two are the same kernel value; on a disagreement trust telemetry (§6.1).
    let disagreement = (kusd != me.boot_id).then_some(kusd);
    let boot_time = boot_time().ok_or(IdentityError::BootTime)?;
    let identity = Identity { device, boot: boot_id(me.boot_id, boot_time), kernel_boot_id: me.boot_id as u16 };
    Ok(Started {
        identity,
        ticks: ticks(),
        anchor: anchor(),
        current_control_set: current_control_set().ok_or(IdentityError::ControlSet)?,
        self_key: me.start_key,
        started: qpc_now(),
        boot_id_disagreement: disagreement,
    })
}

pub fn ticks() -> Ticks {
    Ticks::new(qpc_frequency())
}

/// A (QPC, wall clock) pair (§3.3); re-taken every 60 s by the driver.
pub fn anchor() -> Anchor {
    let before = qpc_now();
    // SAFETY: no arguments; returns the current time.
    let ft = unsafe { GetSystemTimePreciseAsFileTime() };
    let after = qpc_now();
    let ft = (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime);
    Anchor { qpc: before + (after - before) / 2, unix_ns: filetime_to_unix_ns(ft) }
}

/// `device.boot_id` = `BLAKE3("atlas.boot.v1" ‖ BootId[u32 LE] ‖ boot_time[u64 LE])[0..16]`
/// (0a §4.4, §6.1).
pub fn boot_id(kernel_boot_id: u32, boot_time: u64) -> BootId {
    let mut h = blake3::Hasher::new();
    h.update(b"atlas.boot.v1");
    h.update(&kernel_boot_id.to_le_bytes());
    h.update(&boot_time.to_le_bytes());
    let mut out = [0u8; 16];
    out.copy_from_slice(&h.finalize().as_bytes()[..16]);
    BootId::from_bytes(out)
}

/// The persisted `device.uid`: read from `path`, or generated and written once
/// (§6.1). A file that exists but is not valid is an error, never replaced: a
/// new uid would split the device's history.
pub fn device_uid(path: &Path) -> Result<DeviceUid, IdentityError> {
    let err = |e: &dyn fmt::Display| IdentityError::DeviceFile(format!("{}: {e}", path.display()));
    if !path.exists() {
        let uid = DeviceUid::from_bytes(*uuid::Uuid::new_v4().as_bytes());
        // Write a temporary file, then link it into place: the link fails if
        // another process created the file first, and nobody reads a partial file.
        // A fresh name: a stale temporary from an earlier run, or another caller's, is never ours.
        let tmp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4().simple()));
        let written = (|| {
            let mut f = std::fs::File::create_new(&tmp)?;
            f.write_all(serde_json::json!({ "device_uid": uid.to_string() }).to_string().as_bytes())?;
            f.sync_all()
        })();
        let linked = written.and_then(|()| std::fs::hard_link(&tmp, path));
        let _ = std::fs::remove_file(&tmp);
        match linked {
            Ok(()) => return Ok(uid),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(err(&e)),
        }
    }
    let text = std::fs::read_to_string(path).map_err(|e| err(&e))?;
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| err(&e))?;
    let hex = v.get("device_uid").and_then(|h| h.as_str()).ok_or_else(|| err(&"no device_uid"))?;
    parse_uid(hex).ok_or_else(|| err(&"device_uid is not 32 lowercase hex digits"))
}

fn parse_uid(hex: &str) -> Option<DeviceUid> {
    if hex.len() != 32 || !hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(DeviceUid::from_bytes(out))
}

/// `KUSER_SHARED_DATA.BootId`: user-mode address `0x7FFE0000 + 0x2c4`, an offset
/// from public symbols on builds 26200 and 26300 (§6.1).
fn kusd_boot_id() -> u32 {
    const BOOT_ID: usize = 0x7FFE_0000 + 0x2c4;
    // SAFETY: KUSER_SHARED_DATA is mapped read-only at this fixed address in
    // every process; the field is a 4-byte-aligned u32 inside its first page.
    unsafe { (BOOT_ID as *const u32).read_volatile() }
}

/// The System process's creation time (FILETIME), §6.1. Read from
/// `SystemProcessInformation`, the same kernel field `GetProcessTimes` returns,
/// without needing a handle to PID 4 (an unelevated caller cannot open it).
fn boot_time() -> Option<u64> {
    const SYSTEM_PROCESS_INFORMATION: SYSTEM_INFORMATION_CLASS = SYSTEM_INFORMATION_CLASS(5);
    // SYSTEM_PROCESS_INFORMATION (x64): NextEntryOffset @0, CreateTime @32, UniqueProcessId @80.
    let mut buf = aligned(1 << 20);
    loop {
        let mut ret = 0u32;
        let len = (buf.len() * 8) as u32;
        // SAFETY: `buf` is writable for `len` bytes and 8-aligned.
        let st =
            unsafe { NtQuerySystemInformation(SYSTEM_PROCESS_INFORMATION, buf.as_mut_ptr().cast(), len, &mut ret) };
        if st.is_err() {
            if ret > len && ret < 1 << 28 {
                buf = aligned(ret as usize + (1 << 16));
                continue;
            }
            return None;
        }
        let b = as_bytes(&buf);
        let mut off = 0usize;
        loop {
            if u64_at(b, off + 80)? == 4 {
                return u64_at(b, off + 32).filter(|t| *t != 0);
            }
            match u32_at(b, off)? {
                0 => return None,
                next => off += next as usize,
            }
        }
    }
}

fn current_control_set() -> Option<u32> {
    let mut v = 0u32;
    let mut size = 4u32;
    // SAFETY: writes at most `size` bytes into `v`.
    let st = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(r"SYSTEM\Select"),
            w!("Current"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut v as *mut u32).cast()),
            Some(&mut size),
        )
    };
    st.is_ok().then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_id_formula() {
        // BLAKE3("atlas.boot.v1" ‖ 7u32 LE ‖ 9u64 LE), first 16 bytes.
        let mut h = blake3::Hasher::new();
        h.update(b"atlas.boot.v1\x07\x00\x00\x00\x09\x00\x00\x00\x00\x00\x00\x00");
        assert_eq!(boot_id(7, 9).as_bytes()[..], h.finalize().as_bytes()[..16]);
        assert_ne!(boot_id(7, 9), boot_id(8, 9));
        assert_ne!(boot_id(7, 9), boot_id(7, 10));
    }

    #[test]
    fn device_uid_is_created_once_and_kept() {
        let dir = std::env::temp_dir().join(format!("atlas-identity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("device.json");
        let a = device_uid(&path).unwrap();
        assert_eq!(device_uid(&path).unwrap(), a);
        let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(leftovers, vec![std::ffi::OsString::from("device.json")]);

        // A damaged file is an error, and stays as it was.
        std::fs::write(&path, r#"{"device_uid":"xyz"}"#).unwrap();
        assert!(matches!(device_uid(&path), Err(IdentityError::DeviceFile(_))));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), r#"{"device_uid":"xyz"}"#);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn uid_parsing() {
        assert!(parse_uid("000102030405060708090a0b0c0d0e0f").is_some());
        assert!(parse_uid("000102030405060708090A0B0C0D0E0F").is_none());
        assert!(parse_uid("0001").is_none());
    }

    #[test]
    fn start_reads_this_machine() {
        let dir = std::env::temp_dir().join(format!("atlas-start-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = start(&dir.join("device.json")).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(s.self_key >> 48, u64::from(s.identity.kernel_boot_id), "own start key carries the BootId");
        assert_eq!(s.boot_id_disagreement, None, "KUSER_SHARED_DATA.BootId offset");
        assert!(s.current_control_set >= 1);
        assert!(s.ticks.frequency > 0 && s.started >= s.anchor.qpc);
        let now_ns = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as i64;
        assert!((now_ns - s.anchor.unix_ns).abs() < 5_000_000_000, "anchor is the wall clock");
        let bt = boot_time().unwrap();
        assert!(filetime_to_unix_ns(bt) < now_ns, "booted in the past");
        assert_eq!(s.identity.boot, boot_id(u32::from(s.identity.kernel_boot_id), bt));
    }

    /// The System process's creation time from `SystemProcessInformation`
    /// equals `GetProcessTimes` on PID 4, which needs an elevated caller.
    #[test]
    #[ignore = "opening PID 4 needs an elevated run"]
    fn boot_time_matches_get_process_times() {
        use windows::Win32::Foundation::FILETIME;
        use windows::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        let h = super::super::util::Owned(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, 4) }.unwrap());
        let (mut c, mut e, mut k, mut u) =
            (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        // SAFETY: test-only; four writable FILETIMEs.
        unsafe { GetProcessTimes(h.raw(), &mut c, &mut e, &mut k, &mut u) }.unwrap();
        let created = (u64::from(c.dwHighDateTime) << 32) | u64::from(c.dwLowDateTime);
        assert_eq!(boot_time(), Some(created));
    }
}
