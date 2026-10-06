//! Registry value reads after a SetValueKey (sensor spec §7.5).
//!
//! The key is opened by its raw NT name with `OBJ_OPENLINK`, so a key swapped
//! for a symbolic link is not followed, and with `REG_OPTION_BACKUP_RESTORE`
//! when `SeBackupPrivilege` is enabled, which defeats a deny-SYSTEM DACL. The
//! value name is counted, so embedded NULs are kept.

use windows::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows::Wdk::System::Registry::{KeyValuePartialInformation, NtOpenKeyEx, NtQueryValueKey};
use windows::Win32::Foundation::{HANDLE, OBJ_CASE_INSENSITIVE, OBJ_OPENLINK};
use windows::Win32::System::Registry::{KEY_QUERY_VALUE, REG_OPTION_BACKUP_RESTORE};

use super::util::{Owned, aligned, as_bytes, counted, u32_at};
use crate::services::{ValueData, ValueRead};

/// At most this much data is kept (§7.5).
pub(crate) const DATA_MAX: usize = 4096;

const STATUS_BUFFER_OVERFLOW: i32 = 0x8000_0005_u32 as i32;

/// Reads the value; `None` if the key or value cannot be read.
///
/// One query, with room for the header and 4 KiB of data. A larger value
/// returns `STATUS_BUFFER_OVERFLOW` with the header and the first bytes of
/// data filled in: that is all the event keeps, so the value is never read
/// whole, however large it is (R-M7).
pub(crate) fn read(r: &ValueRead, backup: bool) -> Option<ValueData> {
    let key = open_key(&r.key_path, backup)?;
    let mut name = r.value_name.clone();
    let name = counted(&mut name)?;
    // KEY_VALUE_PARTIAL_INFORMATION: TitleIndex @0, Type @4, DataLength @8, Data @12.
    let mut buf = aligned(12 + DATA_MAX);
    let mut ret = 0u32;
    let len = (buf.len() * 8) as u32;
    // SAFETY: `buf` is writable for `len` bytes; `name` points into a live Vec.
    let st = unsafe {
        NtQueryValueKey(key.raw(), &name, KeyValuePartialInformation, Some(buf.as_mut_ptr().cast()), len, &mut ret)
    };
    if st.is_err() && st.0 != STATUS_BUFFER_OVERFLOW {
        return None;
    }
    let b = as_bytes(&buf);
    let size = u32_at(b, 8)?;
    let kept = (size as usize).min(DATA_MAX);
    Some(ValueData { value_type: u32_at(b, 4)?, size, data: b.get(12..12 + kept)?.to_vec() })
}

fn open_key(nt_path: &str, backup: bool) -> Option<Owned> {
    let mut units: Vec<u16> = nt_path.encode_utf16().collect();
    let name = counted(&mut units)?;
    let oa = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        ObjectName: &name,
        Attributes: OBJ_CASE_INSENSITIVE | OBJ_OPENLINK,
        ..Default::default()
    };
    let options = if backup { REG_OPTION_BACKUP_RESTORE.0 } else { 0 };
    let mut h = HANDLE::default();
    // SAFETY: `oa` and the name it points to outlive the call; the handle is owned below.
    unsafe { NtOpenKeyEx(&mut h, KEY_QUERY_VALUE.0, &oa, options) }.ok().ok()?;
    Some(Owned(h))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use windows::Wdk::Foundation::{NtQueryObject, OBJECT_INFORMATION_CLASS};
    use windows::Wdk::System::Registry::NtSetValueKey;
    use windows::Win32::Foundation::UNICODE_STRING;
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_ALL_ACCESS, REG_BINARY, REG_DWORD, REG_LINK, REG_OPTION_CREATE_LINK,
        REG_OPTION_VOLATILE, REG_SZ, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW,
    };
    use windows::core::{HSTRING, PCWSTR};

    use super::super::util::wide;

    /// A volatile test key under HKCU, deleted on drop.
    pub(crate) struct TestKey {
        pub hkey: HKEY,
        sub: String,
    }

    impl TestKey {
        pub(crate) fn new(tag: &str) -> Self {
            let sub = format!(r"Software\AtlasTest-{tag}-{}", std::process::id());
            TestKey { hkey: Self::create(&sub, REG_OPTION_VOLATILE.0), sub }
        }

        fn create(sub: &str, options: u32) -> HKEY {
            let mut h = HKEY::default();
            // SAFETY: test-only; creates a key under HKCU.
            unsafe {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    &HSTRING::from(sub),
                    None,
                    None,
                    windows::Win32::System::Registry::REG_OPEN_CREATE_OPTIONS(options),
                    KEY_ALL_ACCESS,
                    None,
                    &mut h,
                    None,
                )
            }
            .ok()
            .expect("create test key");
            h
        }

        pub(crate) fn child(&self, name: &str, options: u32) -> HKEY {
            Self::create(&format!(r"{}\{name}", self.sub), options)
        }

        /// The key's NT name (`\REGISTRY\USER\<SID>\Software\…`).
        pub(crate) fn nt_name(h: HKEY) -> String {
            let mut buf = aligned(2048);
            let mut ret = 0u32;
            // SAFETY: ObjectNameInformation into an aligned, writable buffer.
            let st = unsafe {
                NtQueryObject(
                    Some(HANDLE(h.0)),
                    OBJECT_INFORMATION_CLASS(1),
                    Some(buf.as_mut_ptr().cast()),
                    (buf.len() * 8) as u32,
                    Some(&mut ret),
                )
            };
            assert!(st.is_ok());
            // SAFETY: the buffer starts with a UNICODE_STRING pointing into itself.
            let us = unsafe { &*(buf.as_ptr() as *const UNICODE_STRING) };
            String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(us.Buffer.0, us.Length as usize / 2) })
        }

        pub(crate) fn path(&self) -> String {
            Self::nt_name(self.hkey)
        }

        pub(crate) fn set(&self, h: HKEY, name: &[u16], ty: u32, data: &[u8]) {
            let mut n = name.to_vec();
            let us = counted(&mut n).unwrap();
            // SAFETY: test-only; counted name and data are live.
            let st =
                unsafe { NtSetValueKey(HANDLE(h.0), &us, None, ty, Some(data.as_ptr().cast()), data.len() as u32) };
            assert!(st.is_ok(), "NtSetValueKey {st:?}");
        }
    }

    impl Drop for TestKey {
        fn drop(&mut self) {
            // SAFETY: deletes the test tree.
            unsafe {
                let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(self.sub.as_str()));
            }
        }
    }

    pub(crate) fn units(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn read_of(path: String, name: &[u16]) -> ValueRead {
        ValueRead { key_path: path, value_name: name.to_vec() }
    }

    #[test]
    fn reads_types_sizes_and_truncates() {
        let k = TestKey::new("value");
        k.set(k.hkey, &units("d"), REG_DWORD.0, &7u32.to_le_bytes());
        let big = vec![0xAB; 10_000];
        k.set(k.hkey, &units("big"), REG_BINARY.0, &big);
        let d = read(&read_of(k.path(), &units("d")), false).unwrap();
        assert_eq!(d, ValueData { value_type: REG_DWORD.0, size: 4, data: 7u32.to_le_bytes().to_vec() });
        let b = read(&read_of(k.path(), &units("big")), false).unwrap();
        assert_eq!((b.value_type, b.size, b.data.len()), (REG_BINARY.0, 10_000, DATA_MAX));
        assert!(b.data.iter().all(|&x| x == 0xAB));
        assert_eq!(read(&read_of(k.path(), &units("absent")), false), None);
        assert_eq!(read(&read_of(format!(r"{}\nokey", k.path()), &units("d")), false), None);
        // Case-insensitive like the registry; the empty name is the default value.
        k.set(k.hkey, &[], REG_SZ.0, &[b'x', 0, 0, 0]);
        assert_eq!(read(&read_of(k.path().to_uppercase(), &[]), false).map(|v| v.size), Some(4));
    }

    /// R-M7: a value of any size gives its type, full size and first 4 KiB.
    #[test]
    fn huge_values_are_read_without_reading_them_whole() {
        let k = TestKey::new("huge");
        let mut big = vec![0u8; 17 << 20];
        big[..9].copy_from_slice(b"evil.exe\0");
        k.set(k.hkey, &units("run"), REG_BINARY.0, &big);
        let v = read(&read_of(k.path(), &units("run")), false).expect("read");
        assert_eq!((v.value_type, v.size, v.data.len()), (REG_BINARY.0, 17 << 20, DATA_MAX));
        assert_eq!(&v.data[..9], b"evil.exe\0");
    }

    #[test]
    fn embedded_nuls_in_value_names_are_kept() {
        let k = TestKey::new("nul");
        let name = [b'a' as u16, 0, b'b' as u16];
        k.set(k.hkey, &name, REG_DWORD.0, &1u32.to_le_bytes());
        k.set(k.hkey, &units("a"), REG_DWORD.0, &2u32.to_le_bytes());
        assert_eq!(read(&read_of(k.path(), &name), false).map(|v| v.data), Some(1u32.to_le_bytes().to_vec()));
        assert_eq!(read(&read_of(k.path(), &units("a")), false).map(|v| v.data), Some(2u32.to_le_bytes().to_vec()));
    }

    #[test]
    fn symbolic_link_keys_are_not_followed() {
        let k = TestKey::new("link");
        let target = k.child("target", REG_OPTION_VOLATILE.0);
        k.set(target, &units("v"), REG_DWORD.0, &5u32.to_le_bytes());
        let link = k.child("link", REG_OPTION_VOLATILE.0 | REG_OPTION_CREATE_LINK.0);
        let to: Vec<u8> = TestKey::nt_name(target).encode_utf16().flat_map(u16::to_le_bytes).collect();
        let slv = wide("SymbolicLinkValue");
        // SAFETY: test-only; REG_LINK data is the counted target name (no NUL).
        unsafe { RegSetValueExW(link, PCWSTR(slv.as_ptr()), None, REG_LINK, Some(&to)) }.ok().unwrap();
        // Control: a reader that follows links sees the target's value.
        let mut v = 0u32;
        let mut size = 4u32;
        let sub = HSTRING::from(format!(r"{}\link", k.sub));
        // SAFETY: test-only; writes at most 4 bytes.
        unsafe {
            windows::Win32::System::Registry::RegGetValueW(
                HKEY_CURRENT_USER,
                &sub,
                &HSTRING::from("v"),
                windows::Win32::System::Registry::RRF_RT_REG_DWORD,
                None,
                Some((&mut v as *mut u32).cast()),
                Some(&mut size),
            )
        }
        .ok()
        .expect("the link is followed by a normal open");
        assert_eq!(v, 5);
        let through = format!(r"{}\link", k.path());
        assert_eq!(read(&read_of(through, &units("v")), false), None, "the link itself has no value v");
        assert!(read(&read_of(TestKey::nt_name(target), &units("v")), false).is_some());
    }

    #[test]
    fn unreadable_hives_fail() {
        assert_eq!(
            read(&read_of(r"\REGISTRY\A\{00000000-0000-0000-0000-000000000000}".into(), &units("x")), false),
            None
        );
    }

    /// `HKLM\SAM\SAM` allows only SYSTEM: an elevated admin reads it only
    /// with `REG_OPTION_BACKUP_RESTORE` and `SeBackupPrivilege`.
    #[test]
    #[ignore = "needs SeBackupPrivilege (an elevated run)"]
    fn backup_semantics_pass_a_system_only_dacl() {
        assert!(super::super::privilege::enable(super::super::privilege::BACKUP));
        let r = read_of(r"\REGISTRY\MACHINE\SAM\SAM\Domains\Account".into(), &units("F"));
        assert_eq!(read(&r, false), None, "denied without backup semantics");
        let v = read(&r, true).expect("read with backup semantics");
        assert_eq!(v.value_type, REG_BINARY.0);
        assert!(v.size > 0);
    }
}
