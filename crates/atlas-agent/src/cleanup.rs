//! The outcome of a file Cleanup (sensor spec §5.1; plan 1b-3c, decision D3).
//!
//! A file system reports what a Cleanup removed in the IRP's `Information`,
//! which Kernel-File logs as the `ExtraInformation` of the Cleanup's
//! OperationEnd (24): the `FILE_CLEANUP_*` values of `ntifs.h`. Verified on the
//! host (build 26200) for NTFS, FAT32 and exFAT; an SMB share (`\Device\Mup`)
//! reports `UNKNOWN`. A delete is reported when it happens, from the process
//! doing it, and an undelete (a disposition set, then cleared) reports
//! `FILE_REMAINS`.

use atlas_etw::parse::RawEvent;

/// The file system did not say (SMB shares).
pub const UNKNOWN: u64 = 0;
pub const FILE_REMAINS: u64 = 0x2;
pub const FILE_DELETED: u64 = 0x4;
/// One hard link went; the file remains.
pub const LINK_DELETED: u64 = 0x8;
pub const STREAM_DELETED: u64 = 0x10;
/// Set with one of the above for a POSIX-style delete.
pub const POSIX_STYLE_DELETE: u64 = 0x20;

/// The Cleanup removed a name: the file, a hard link or a stream. Only the
/// values a file system reports count (one of the three, with or without the
/// POSIX flag), so another operation's `Information` (a byte count, a
/// query's length) that happens to share a bit is not taken for one.
pub fn removed(info: u64) -> bool {
    matches!(info & !POSIX_STYLE_DELETE, FILE_DELETED | LINK_DELETED | STREAM_DELETED)
}

/// The Irp of a Kernel-File event that starts an operation (every event with
/// an Irp but Cleanup and OperationEnd). Irps are per thread and reused, so
/// such an event ends whatever the Irp did before.
pub fn op_irp(e: &RawEvent) -> Option<u64> {
    match e {
        RawEvent::FileCreate(c) | RawEvent::FileCreateNew(c) => Some(c.irp),
        RawEvent::FileClose(h) => Some(h.irp),
        RawEvent::FileWrite(w) => Some(w.irp),
        RawEvent::FileSetInfo(i) | RawEvent::FileSetDelete(i) => Some(i.irp),
        RawEvent::FileDeletePath(p) | RawEvent::FileRenamePath(p) => Some(p.irp),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_counts_as_removed() {
        for info in [FILE_DELETED, LINK_DELETED, STREAM_DELETED, FILE_DELETED | POSIX_STYLE_DELETE] {
            assert!(removed(info), "{info:#x}");
        }
        // 0x18, 0x38: a query's returned length, seen on these Irps (review R-M4).
        for info in [UNKNOWN, FILE_REMAINS, POSIX_STYLE_DELETE, 0x18, 0x38, 0x14, 0x44, 100] {
            assert!(!removed(info), "{info:#x}");
        }
    }
}
