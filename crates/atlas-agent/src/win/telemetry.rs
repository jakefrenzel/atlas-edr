//! `PROCESS_TELEMETRY_ID_INFORMATION` (`NtQueryInformationProcess` class 64;
//! sensor spec §5.2, §6.1): a live process's start key, BootId, image path and
//! command line. The layout is undocumented (phnt `ntpsapi.h`); every field is
//! read bounds-checked, and the spikes (S1, S2, S8) confirmed it on the host.

use windows::Wdk::System::Threading::{NtQueryInformationProcess, PROCESSINFOCLASS};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

use super::util::{Owned, aligned, as_bytes, u32_at, u64_at, utf16z_at};

const PROCESS_TELEMETRY_ID_INFORMATION: PROCESSINFOCLASS = PROCESSINFOCLASS(64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Telemetry {
    pub pid: u32,
    pub start_key: u64,
    pub boot_id: u32,
    /// NT path (`\Device\HarddiskVolume3\…`).
    pub image_path: Option<String>,
    pub command_line: Option<String>,
}

/// Field offsets (x64).
const PID: usize = 4;
const START_KEY: usize = 8;
const BOOT_ID: usize = 60;
const IMAGE_PATH_OFFSET: usize = 76;
const COMMAND_LINE_OFFSET: usize = 88;
/// The fixed part ends after `CommandLineOffset`.
const FIXED: usize = 92;

/// Parses the buffer the kernel filled. `None` if it is too short.
pub(crate) fn parse(b: &[u8]) -> Option<Telemetry> {
    if b.len() < FIXED {
        return None;
    }
    let string = |field: usize| match u32_at(b, field)? as usize {
        0 => None,
        off => utf16z_at(b, off),
    };
    Some(Telemetry {
        pid: u32_at(b, PID)?,
        start_key: u64_at(b, START_KEY)?,
        boot_id: u32_at(b, BOOT_ID)?,
        image_path: string(IMAGE_PATH_OFFSET),
        command_line: string(COMMAND_LINE_OFFSET),
    })
}

/// The process with this PID, if it is running and can be opened.
pub(crate) fn query(pid: u32) -> Option<Telemetry> {
    // SAFETY: the handle is closed by `Owned`.
    let h = Owned(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?);
    let t = query_handle(h.raw())?;
    // A PID can be reused at any moment; the kernel answered for this handle's process.
    (t.pid == pid).then_some(t)
}

/// The agent's own process.
pub(crate) fn current() -> Option<Telemetry> {
    // SAFETY: the pseudo-handle needs no closing.
    query_handle(unsafe { GetCurrentProcess() })
}

fn query_handle(h: HANDLE) -> Option<Telemetry> {
    let mut buf = aligned(4096);
    loop {
        let mut ret = 0u32;
        let len = (buf.len() * 8) as u32;
        // SAFETY: `buf` is writable for `len` bytes and 8-aligned.
        let st = unsafe {
            NtQueryInformationProcess(h, PROCESS_TELEMETRY_ID_INFORMATION, buf.as_mut_ptr().cast(), len, &mut ret)
        };
        if st.is_ok() {
            return parse(as_bytes(&buf).get(..ret as usize)?);
        }
        if ret > len && ret <= 1 << 20 {
            buf = aligned(ret as usize);
            continue;
        }
        return None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(image: Option<&str>, cmd: Option<&str>) -> Vec<u8> {
        let mut b = vec![0u8; 96];
        b[0..4].copy_from_slice(&96u32.to_le_bytes());
        b[4..8].copy_from_slice(&1234u32.to_le_bytes());
        b[8..16].copy_from_slice(&0x007c_0000_0000_33eeu64.to_le_bytes());
        b[60..64].copy_from_slice(&124u32.to_le_bytes());
        for (field, s) in [(IMAGE_PATH_OFFSET, image), (COMMAND_LINE_OFFSET, cmd)] {
            if let Some(s) = s {
                let off = b.len() as u32;
                b[field..field + 4].copy_from_slice(&off.to_le_bytes());
                b.extend(s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
            }
        }
        b
    }

    #[test]
    fn parses_fields_and_strings() {
        let t = parse(&buffer(Some(r"\Device\HarddiskVolume3\x.exe"), Some("x.exe -a"))).unwrap();
        assert_eq!(t.pid, 1234);
        assert_eq!(t.start_key, 0x007c_0000_0000_33ee);
        assert_eq!(t.boot_id, 124);
        assert_eq!(t.image_path.as_deref(), Some(r"\Device\HarddiskVolume3\x.exe"));
        assert_eq!(t.command_line.as_deref(), Some("x.exe -a"));
    }

    #[test]
    fn missing_strings_and_short_buffers() {
        let t = parse(&buffer(None, None)).unwrap();
        assert_eq!((t.image_path, t.command_line), (None, None));
        assert_eq!(parse(&buffer(None, None)[..FIXED - 1]), None);
    }

    #[test]
    fn reads_our_own_process() {
        let me = current().expect("own telemetry");
        assert_eq!(me.pid, std::process::id());
        assert_eq!(me.start_key >> 48, u64::from(me.boot_id) & 0xFFFF, "start key = BootId << 48 | sequence");
        let exe = me.image_path.expect("image path");
        assert!(exe.starts_with(r"\Device\"), "NT path: {exe}");
        assert_eq!(query(std::process::id()).map(|t| t.start_key), Some(me.start_key));
    }
}
