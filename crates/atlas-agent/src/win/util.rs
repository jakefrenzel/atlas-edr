//! Small helpers shared by the Windows services.

use windows::Win32::Foundation::{CloseHandle, HANDLE, UNICODE_STRING};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::core::PWSTR;

/// A NUL-terminated UTF-16 copy of `s`.
pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// A kernel handle closed on drop.
#[derive(Debug)]
pub(crate) struct Owned(pub HANDLE);

impl Owned {
    pub(crate) fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_invalid() && !self.0.0.is_null() {
            // SAFETY: the handle is owned by this value and closed only here.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

// SAFETY: a kernel handle value can be used and closed from any thread.
unsafe impl Send for Owned {}

/// The current QPC value.
pub(crate) fn qpc_now() -> i64 {
    let mut v = 0i64;
    // SAFETY: writes one i64; cannot fail on Windows XP and later.
    let _ = unsafe { QueryPerformanceCounter(&mut v) };
    v
}

/// QPC ticks per second.
pub(crate) fn qpc_frequency() -> i64 {
    let mut v = 0i64;
    // SAFETY: as above.
    let _ = unsafe { QueryPerformanceFrequency(&mut v) };
    v
}

/// A counted string over `units`, or `None` if it is longer than a
/// `UNICODE_STRING` can describe (32,767 units). `units` must outlive it.
pub(crate) fn counted(units: &mut [u16]) -> Option<UNICODE_STRING> {
    let bytes = u16::try_from(units.len() * 2).ok().filter(|b| *b <= 0xFFFE)?;
    Some(UNICODE_STRING { Length: bytes, MaximumLength: bytes, Buffer: PWSTR(units.as_mut_ptr()) })
}

/// The UTF-16 string at `off` in `b`, up to its NUL (or the end of `b`).
pub(crate) fn utf16z_at(b: &[u8], off: usize) -> Option<String> {
    let units: Vec<u16> =
        b.get(off..)?.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
    Some(String::from_utf16_lossy(&units))
}

pub(crate) fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(off..off + 4)?.try_into().ok()?))
}

pub(crate) fn u64_at(b: &[u8], off: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(off..off + 8)?.try_into().ok()?))
}

/// A buffer of `bytes` bytes (rounded up to 8), aligned for the structures the
/// NT query functions return.
pub(crate) fn aligned(bytes: usize) -> Vec<u64> {
    vec![0u64; bytes.div_ceil(8)]
}

/// The bytes of an [`aligned`] buffer.
pub(crate) fn as_bytes(buf: &[u64]) -> &[u8] {
    // SAFETY: any u64 slice is a valid byte slice of eight times its length.
    unsafe { std::slice::from_raw_parts(buf.as_ptr().cast(), buf.len() * 8) }
}

/// FILETIME (100 ns since 1601) → Unix nanoseconds.
pub(crate) fn filetime_to_unix_ns(ft: u64) -> i64 {
    const EPOCH_DIFF: i64 = 116_444_736_000_000_000;
    (ft as i64 - EPOCH_DIFF).saturating_mul(100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16z_stops_at_nul_and_end() {
        let b: Vec<u8> = "ab\0c".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(utf16z_at(&b, 0).as_deref(), Some("ab"));
        assert_eq!(utf16z_at(&b, 6).as_deref(), Some("c"));
        assert_eq!(utf16z_at(&b, 100), None);
    }

    #[test]
    fn counted_strings_have_a_limit() {
        let mut ok = vec![b'a' as u16; 32_767];
        assert_eq!(counted(&mut ok).map(|u| u.Length), Some(65_534));
        let mut long = vec![b'a' as u16; 32_768];
        assert!(counted(&mut long).is_none());
    }

    #[test]
    fn filetime_epoch() {
        assert_eq!(filetime_to_unix_ns(116_444_736_000_000_000), 0);
        assert_eq!(filetime_to_unix_ns(116_444_736_000_000_001), 100);
    }
}
