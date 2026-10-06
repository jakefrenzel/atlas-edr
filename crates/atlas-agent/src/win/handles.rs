//! The system handle table and naming handles from it (sensor spec §7.4).
//!
//! Handles are duplicated into the agent with **no access rights** (plan
//! 1b-3b, D2), so the agent never holds a handle that can read or write
//! another process's object. Keys are named with
//! `NtQueryObject(ObjectNameInformation)`: `NtQueryKey(KeyNameInformation)` is
//! denied on a no-access handle. Files are named with
//! `GetFinalPathNameByHandleW` on a helper thread with a timeout, because a
//! query on a synchronous file object can block behind another thread's I/O.

use std::os::windows::io::AsRawHandle;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Wdk::Foundation::{NtQueryObject, OBJECT_INFORMATION_CLASS};
use windows::Wdk::Storage::FileSystem::{FileFsDeviceInformation, NtQueryVolumeInformationFile};
use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SYSTEM_INFORMATION_CLASS};
use windows::Win32::Foundation::{DUPLICATE_HANDLE_OPTIONS, DuplicateHandle, HANDLE, UNICODE_STRING};
use windows::Win32::Storage::FileSystem::{GetFinalPathNameByHandleW, VOLUME_NAME_NT};
use windows::Win32::System::IO::{CancelSynchronousIo, IO_STATUS_BLOCK};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, OpenProcess, PROCESS_DUP_HANDLE, SetThreadPriority,
    THREAD_PRIORITY_BELOW_NORMAL,
};

use super::util::{Owned, aligned, as_bytes};

/// One row of the handle table (`SYSTEM_HANDLE_TABLE_ENTRY_INFO_EX`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Entry {
    /// Kernel object address: Kernel-Registry's `KeyObject`, Kernel-File's `FileObject`.
    pub object: u64,
    pub pid: u32,
    pub handle: u64,
    pub type_index: u16,
}

/// `SYSTEM_HANDLE_TABLE_ENTRY_INFO_EX` (x64, 40 bytes): Object @0, UniqueProcessId @8,
/// HandleValue @16, GrantedAccess @24, CreatorBackTraceIndex @28, ObjectTypeIndex @30.
const ENTRY: usize = 40;

/// Reads the whole table. Object addresses are zero without `SeDebugPrivilege`.
pub(crate) fn table() -> Option<Vec<Entry>> {
    const SYSTEM_EXTENDED_HANDLE_INFORMATION: SYSTEM_INFORMATION_CLASS = SYSTEM_INFORMATION_CLASS(64);
    const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;
    let mut buf = aligned(8 << 20);
    for _ in 0..8 {
        let mut ret = 0u32;
        let len = (buf.len() * 8) as u32;
        // SAFETY: `buf` is writable for `len` bytes and 8-aligned.
        let st = unsafe {
            NtQuerySystemInformation(SYSTEM_EXTENDED_HANDLE_INFORMATION, buf.as_mut_ptr().cast(), len, &mut ret)
        };
        if st.0 == STATUS_INFO_LENGTH_MISMATCH {
            // The table grows between calls: leave headroom.
            buf = aligned((ret as usize).max(len as usize) * 3 / 2);
            continue;
        }
        if st.is_err() {
            return None;
        }
        return Some(parse_table(as_bytes(&buf)));
    }
    None
}

/// Header: NumberOfHandles (usize), Reserved (usize); then the entries.
pub(crate) fn parse_table(b: &[u8]) -> Vec<Entry> {
    let field = |off: usize| b.get(off..off + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap()));
    let count = field(0).unwrap_or(0) as usize;
    let count = count.min(b.len().saturating_sub(16) / ENTRY);
    (0..count)
        .filter_map(|i| {
            let e = 16 + i * ENTRY;
            Some(Entry {
                object: field(e)?,
                pid: u32::try_from(field(e + 8)?).ok()?,
                handle: field(e + 16)?,
                type_index: u16::from_le_bytes(b.get(e + 30..e + 32)?.try_into().ok()?),
            })
        })
        .collect()
}

/// A process opened for duplicating its handles.
pub(crate) fn open_process(pid: u32) -> Option<Owned> {
    // SAFETY: the handle is owned by `Owned`.
    unsafe { OpenProcess(PROCESS_DUP_HANDLE, false, pid) }.ok().map(Owned)
}

/// Duplicates `handle` from `process` into the agent with no access rights.
pub(crate) fn duplicate(process: &Owned, handle: u64) -> Option<Owned> {
    let mut dup = HANDLE::default();
    // SAFETY: duplicates into our own process; the copy is owned by `Owned`.
    unsafe {
        DuplicateHandle(
            process.raw(),
            HANDLE(handle as *mut _),
            GetCurrentProcess(),
            &mut dup,
            0,
            false,
            DUPLICATE_HANDLE_OPTIONS(0),
        )
    }
    .ok()?;
    Some(Owned(dup))
}

/// The object's name (`ObjectNameInformation`): for a key, `\REGISTRY\…`.
/// Never used on file handles, where it can block.
pub(crate) fn object_name(h: &Owned) -> Option<String> {
    let mut buf = aligned(4096);
    for _ in 0..2 {
        let mut ret = 0u32;
        let len = (buf.len() * 8) as u32;
        // SAFETY: `buf` is writable and aligned for an OBJECT_NAME_INFORMATION.
        let st = unsafe {
            NtQueryObject(
                Some(h.raw()),
                OBJECT_INFORMATION_CLASS(1),
                Some(buf.as_mut_ptr().cast()),
                len,
                Some(&mut ret),
            )
        };
        if st.is_ok() {
            // SAFETY: the buffer starts with a UNICODE_STRING whose Buffer points into it.
            let us = unsafe { &*(buf.as_ptr() as *const UNICODE_STRING) };
            if us.Buffer.is_null() || us.Length == 0 {
                return None;
            }
            // SAFETY: Length bytes at Buffer lie inside `buf`.
            let units = unsafe { std::slice::from_raw_parts(us.Buffer.0, us.Length as usize / 2) };
            return Some(String::from_utf16_lossy(units));
        }
        if ret > len && ret <= 1 << 16 {
            buf = aligned(ret as usize);
            continue;
        }
        return None;
    }
    None
}

/// `FILE_FS_DEVICE_INFORMATION`.
#[repr(C)]
#[derive(Default)]
struct DeviceInfo {
    device_type: u32,
    characteristics: u32,
}

const FILE_DEVICE_DISK: u32 = 7;
const FILE_REMOTE_DEVICE: u32 = 0x10;

/// The NT name of a disk file; `None` for anything else (pipes, sockets,
/// devices, network redirectors) or a failed query. May block.
fn disk_file_name(h: &Owned) -> Option<String> {
    let mut info = DeviceInfo::default();
    let mut iosb = IO_STATUS_BLOCK::default();
    // SAFETY: `info` is writable for its size.
    let st = unsafe {
        NtQueryVolumeInformationFile(
            h.raw(),
            &mut iosb,
            (&mut info as *mut DeviceInfo).cast(),
            size_of::<DeviceInfo>() as u32,
            FileFsDeviceInformation,
        )
    };
    if st.is_err() || info.device_type != FILE_DEVICE_DISK || info.characteristics & FILE_REMOTE_DEVICE != 0 {
        return None;
    }
    let mut buf = vec![0u16; 1024];
    loop {
        // SAFETY: `buf` is writable; VOLUME_NAME_NT gives `\Device\HarddiskVolumeN\…`, long names.
        let n = unsafe { GetFinalPathNameByHandleW(h.raw(), &mut buf, VOLUME_NAME_NT) } as usize;
        if n == 0 {
            return None;
        }
        if n < buf.len() {
            return Some(String::from_utf16_lossy(&buf[..n]));
        }
        if n > 32_768 {
            return None;
        }
        buf = vec![0u16; n + 1];
    }
}

/// How a file name query ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FileName {
    /// A disk file's NT name, with the duplicate handed back.
    Named(String, Owned),
    /// Not a disk file, or the query failed.
    Unnamable,
    /// No answer within the timeout: the helper keeps the handle and closes it
    /// when its query returns.
    TimedOut,
}

impl PartialEq for Owned {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for Owned {}

/// One helper thread that names file handles.
struct Helper {
    tx: Sender<Owned>,
    rx: Receiver<(Option<String>, Owned)>,
    thread: JoinHandle<()>,
}

impl Helper {
    fn spawn() -> Helper {
        let (tx, jobs) = channel::<Owned>();
        let (done, rx) = channel();
        let thread = std::thread::Builder::new()
            .name("atlas-seeder-namer".into())
            .spawn(move || {
                // SAFETY: lowers this thread's priority, as the seeder's.
                unsafe {
                    let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
                }
                for h in jobs {
                    let name = disk_file_name(&h);
                    // A failed send means the seeder gave up on this query; `h` closes here.
                    if done.send((name, h)).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn a seeder helper");
        Helper { tx, rx, thread }
    }
}

/// File naming with a timeout per query (§7.4). A query that times out is
/// cancelled with `CancelSynchronousIo`; a helper that still does not return
/// is set aside as stuck, and a new one takes over. At most `max_stuck`
/// helpers may be stuck: past that, [`FileNamer::paused`] is true.
pub(crate) struct FileNamer {
    helper: Helper,
    stuck: Vec<Helper>,
    timeout: Duration,
    max_stuck: usize,
}

impl FileNamer {
    pub(crate) fn new(timeout: Duration, max_stuck: usize) -> Self {
        FileNamer { helper: Helper::spawn(), stuck: Vec::new(), timeout, max_stuck }
    }

    /// Helpers stuck now (a Sensor Health gauge). Recovered ones are dropped.
    pub(crate) fn stuck(&mut self) -> usize {
        // A stuck helper that answered has returned: its result is stale, the
        // handle closes with it, and the thread exits once its sender drops.
        self.stuck.retain(|h| matches!(h.rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)));
        self.stuck.len()
    }

    /// Too many helpers are stuck: file seeding pauses (§7.4).
    pub(crate) fn paused(&mut self) -> bool {
        self.stuck() >= self.max_stuck
    }

    pub(crate) fn name(&mut self, h: Owned) -> FileName {
        if self.helper.tx.send(h).is_err() {
            self.helper = Helper::spawn();
            return FileName::Unnamable;
        }
        match self.helper.rx.recv_timeout(self.timeout) {
            Ok((Some(n), h)) => return FileName::Named(n, h),
            Ok((None, _)) => return FileName::Unnamable,
            Err(RecvTimeoutError::Disconnected) => {
                self.helper = Helper::spawn();
                return FileName::Unnamable;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        // SAFETY: the helper thread is alive (its channel is open); cancels its
        // pending synchronous I/O, if any.
        unsafe {
            let _ = CancelSynchronousIo(HANDLE(self.helper.thread.as_raw_handle()));
        }
        match self.helper.rx.recv_timeout(Duration::from_millis(50)) {
            Ok(_) => {}
            Err(_) => {
                let replacement = Helper::spawn();
                self.stuck.push(std::mem::replace(&mut self.helper, replacement));
            }
        }
        FileName::TimedOut
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::io::{FromRawHandle, IntoRawHandle};

    /// Duplicates one of our own handles, as the seeder does for others'.
    fn dup_own(h: HANDLE) -> Owned {
        // SAFETY: test-only; a pseudo-handle for our own process.
        let me = Owned(unsafe { windows::Win32::System::Threading::GetCurrentProcess() });
        let d = duplicate(&me, h.0 as u64).expect("duplicate");
        std::mem::forget(me); // pseudo-handle: never closed
        d
    }

    #[test]
    fn parses_entries() {
        let mut b = vec![0u8; 16 + 2 * ENTRY];
        b[0..8].copy_from_slice(&2u64.to_le_bytes());
        let e = 16 + ENTRY;
        b[e..e + 8].copy_from_slice(&0xffff_a000_1234_5678u64.to_le_bytes());
        b[e + 8..e + 16].copy_from_slice(&4321u64.to_le_bytes());
        b[e + 16..e + 24].copy_from_slice(&0x1a4u64.to_le_bytes());
        b[e + 30..e + 32].copy_from_slice(&44u16.to_le_bytes());
        let t = parse_table(&b);
        assert_eq!(t.len(), 2);
        assert_eq!(t[1], Entry { object: 0xffff_a000_1234_5678, pid: 4321, handle: 0x1a4, type_index: 44 });
        // A count larger than the buffer is cut to what is there.
        b[0..8].copy_from_slice(&1000u64.to_le_bytes());
        assert_eq!(parse_table(&b).len(), 2);
    }

    #[test]
    fn our_handles_are_in_the_table() {
        let f = std::fs::File::open(std::env::current_exe().unwrap()).unwrap();
        let t = table().expect("handle table");
        let me = std::process::id();
        assert!(t.iter().any(|e| e.pid == me && e.handle == f.as_raw_handle() as u64));
    }

    #[test]
    fn keys_are_named_through_a_no_access_duplicate() {
        let k = super::super::value::tests::TestKey::new("handles");
        let d = dup_own(HANDLE(k.hkey.0));
        assert_eq!(object_name(&d), Some(k.path()));
    }

    #[test]
    fn disk_files_are_named_and_others_are_not() {
        let mut namer = FileNamer::new(Duration::from_millis(200), 2);
        let exe = std::env::current_exe().unwrap();
        let f = std::fs::File::open(&exe).unwrap();
        let FileName::Named(n, _) = namer.name(dup_own(HANDLE(f.as_raw_handle()))) else { panic!("not named") };
        assert!(
            n.starts_with(r"\Device\")
                && n.to_lowercase().ends_with(&exe.file_name().unwrap().to_string_lossy().to_lowercase()),
            "{n}"
        );
        let (r, _w) = std::io::pipe().unwrap();
        let r = r.into_raw_handle();
        assert_eq!(namer.name(dup_own(HANDLE(r))), FileName::Unnamable, "an anonymous pipe is not a disk file");
        // SAFETY: test-only; reclaim the pipe end so it closes.
        drop(unsafe { std::fs::File::from_raw_handle(r) });
    }

    /// A query on a synchronous file object waits while another thread's
    /// operation on it is pending: here a byte-range lock that cannot be
    /// granted yet. The namer gives up, sets the helper aside as stuck, pauses
    /// at the limit, and recovers when the operation completes. (A pipe with a
    /// read pending does not block: its device type is answered at once, and
    /// pipes are never named.)
    #[test]
    fn blocked_queries_time_out_and_recover() {
        use windows::Win32::Storage::FileSystem::{LOCKFILE_EXCLUSIVE_LOCK, LockFileEx, UnlockFile};
        use windows::Win32::System::IO::OVERLAPPED;
        let path = std::env::temp_dir().join(format!("atlas-namer-{}.bin", std::process::id()));
        std::fs::write(&path, b"0123").unwrap();
        let holder = std::fs::File::open(&path).unwrap();
        let waiter = std::fs::File::open(&path).unwrap();
        let lock = |f: &std::fs::File| {
            let mut ov = OVERLAPPED::default();
            // SAFETY: test-only; a synchronous handle, so the call waits until granted.
            unsafe { LockFileEx(HANDLE(f.as_raw_handle()), LOCKFILE_EXCLUSIVE_LOCK, None, 1, 0, &mut ov) }.unwrap();
        };
        lock(&holder);
        let waiter_raw = HANDLE(waiter.as_raw_handle());
        let mut namer = FileNamer::new(Duration::from_millis(100), 1);
        let d = dup_own(waiter_raw);
        let t = std::thread::spawn(move || {
            lock(&waiter);
            waiter
        });
        std::thread::sleep(Duration::from_millis(100)); // the lock request is pending
        assert_eq!(namer.name(d), FileName::TimedOut);
        assert_eq!(namer.stuck(), 1);
        assert!(namer.paused());
        // SAFETY: test-only; releasing the first lock grants the second.
        unsafe { UnlockFile(HANDLE(holder.as_raw_handle()), 0, 0, 1, 0) }.unwrap();
        drop(t.join().unwrap());
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(namer.stuck(), 0, "the helper returned");
        assert!(!namer.paused());
        drop(holder);
        std::fs::remove_file(&path).unwrap();
    }
}
