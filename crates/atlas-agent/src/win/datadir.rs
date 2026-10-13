//! The data directory (sensor spec §11.1; plan 1b-4a, D2).
//!
//! `C:\ProgramData` lets any user create folders, and grants `BUILTIN\Users`
//! write access to subfolders that inherit its ACL. The agent deletes and
//! renames files in its directory as an administrator or SYSTEM, so a folder
//! a user created or can write into would let them aim those operations
//! elsewhere. Therefore the directory, and `buffer\` and `logs\` inside it:
//! - are created with owner Administrators and a **protected** DACL granting
//!   full control to SYSTEM and Administrators only ([`SDDL`]);
//! - when they exist, are verified through one handle opened without following
//!   a reparse point: a real directory, owned by Administrators or SYSTEM, a
//!   protected DACL (not a null one), and no access granted to anyone else.
//!
//! A user who could rename a folder above the directory could put one of
//! their own in its place after these checks (and with it an `agent.toml`
//! that turns the sensor off). So before it, every folder above it, from the
//! volume root down, is checked the same way (without following a reparse
//! point): a real directory, owned by SYSTEM, Administrators or
//! TrustedInstaller, with a DACL that grants no one else a right to rename,
//! delete or re-permission it, or to rename what it contains ([`SWAP_RIGHTS`];
//! entries that only pass to children are skipped). `C:\` and
//! `C:\ProgramData` pass; a user's profile or `%TEMP%` does not. The path must
//! be absolute, without `.` or `..`. On any failure the agent refuses to start.

use std::fmt;
use std::path::{Component, Path, PathBuf};

use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{ConvertSidToStringSidW, GetSecurityInfo, SE_FILE_OBJECT};
use windows::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION, AclSizeInformation, DACL_SECURITY_INFORMATION, GetAce,
    GetAclInformation, GetSecurityDescriptorControl, GetSecurityDescriptorDacl, GetSecurityDescriptorOwner,
    OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED, SECURITY_ATTRIBUTES,
};
use windows::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateDirectoryW, CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, GetFileInformationByHandle, OPEN_EXISTING, READ_CONTROL,
};
use windows::core::{BOOL, PCWSTR, PWSTR};

use super::util::{Owned, SecurityDescriptor, wide};

/// Owner Administrators; a protected, auto-inherited DACL; full control to
/// SYSTEM and Administrators, inherited by files and folders inside.
pub const SDDL: &str = "O:BAD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";
/// The subdirectories the agent writes into.
pub const SUBDIRS: [&str; 2] = ["buffer", "logs"];
/// The rights on a folder above the directory that would let their holder
/// swap what the agent finds there: rename or delete it (`DELETE`), change
/// its permissions or owner (`WRITE_DAC`, `WRITE_OWNER`), rename or delete
/// what it contains (`FILE_DELETE_CHILD`), or all of these (`GENERIC_ALL`).
pub const SWAP_RIGHTS: u32 = 0x0001_0000 | 0x0004_0000 | 0x0008_0000 | 0x0000_0040 | 0x1000_0000;

const SYSTEM: &str = "S-1-5-18";
const ADMINISTRATORS: &str = "S-1-5-32-544";
/// Owns the volume root and the system's folders.
const TRUSTED_INSTALLER: &str = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
const ACCESS_DENIED_ACE_TYPE: u8 = 1;
const INHERIT_ONLY_ACE: u8 = 0x08;

/// Why the directory was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataDirError {
    pub path: PathBuf,
    pub reason: String,
}

impl fmt::Display for DataDirError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "data directory {}: {}", self.path.display(), self.reason)
    }
}

impl std::error::Error for DataDirError {}

fn refuse(path: &Path, reason: impl Into<String>) -> DataDirError {
    DataDirError { path: path.to_owned(), reason: reason.into() }
}

/// Checks the folders above the directory, then creates the directory and its
/// subdirectories as needed, and verifies each. Its parent must exist.
pub fn prepare(dir: &Path) -> Result<(), DataDirError> {
    check_ancestors(dir)?;
    create_or_verify(dir)?;
    for sub in SUBDIRS {
        create_or_verify(&dir.join(sub))?;
    }
    Ok(())
}

/// Every folder above `dir`, from the volume root down (see the module docs).
fn check_ancestors(dir: &Path) -> Result<(), DataDirError> {
    if !dir.is_absolute() || dir.components().any(|c| matches!(c, Component::CurDir | Component::ParentDir)) {
        return Err(refuse(dir, "must be an absolute path without . or .."));
    }
    let mut above: Vec<&Path> = dir.ancestors().skip(1).collect();
    above.reverse();
    for a in above {
        descriptor(a)
            .and_then(|sd| check_ancestor_descriptor(&sd))
            .map_err(|e| refuse(dir, format!("{}, a folder above it, {e}", a.display())))?;
    }
    Ok(())
}

fn create_or_verify(dir: &Path) -> Result<(), DataDirError> {
    let sd = SecurityDescriptor::from_sddl(SDDL).map_err(|e| refuse(dir, e))?;
    let attrs = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0.0,
        bInheritHandle: false.into(),
    };
    let path = wide(&dir.to_string_lossy());
    // SAFETY: `path` is NUL-terminated; `attrs` and the descriptor it points to
    // outlive the call.
    match unsafe { CreateDirectoryW(PCWSTR(path.as_ptr()), Some(&attrs)) } {
        Ok(()) => {}
        Err(e) if e.code() == ERROR_ALREADY_EXISTS.to_hresult() => {}
        Err(e) => return Err(refuse(dir, format!("cannot be created: {e}"))),
    }
    verify(dir)
}

/// Checks one directory (see the module docs).
pub fn verify(dir: &Path) -> Result<(), DataDirError> {
    descriptor(dir).and_then(|sd| check_descriptor(&sd)).map_err(|e| refuse(dir, e))
}

/// A directory's owner and DACL, read through one handle opened without
/// following a reparse point; a reparse point or a file is refused.
fn descriptor(dir: &Path) -> Result<SecurityDescriptor, String> {
    let path = wide(&dir.to_string_lossy());
    // SAFETY: `path` is NUL-terminated. The handle reads attributes and the
    // security descriptor only, and is closed by `Owned`.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(path.as_ptr()),
            (READ_CONTROL | FILE_READ_ATTRIBUTES).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            None,
        )
    }
    .map_err(|e| format!("cannot be opened: {e}"))?;
    let handle = Owned(handle);

    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: a valid handle and an out-structure.
    unsafe { GetFileInformationByHandle(handle.raw(), &mut info) }
        .map_err(|e| format!("attributes cannot be read: {e}"))?;
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Err("is a reparse point (a junction or symbolic link)".into());
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 == 0 {
        return Err("is not a directory".into());
    }

    let mut sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: a valid handle opened with READ_CONTROL; the descriptor is freed
    // by `SecurityDescriptor`'s drop.
    let err = unsafe {
        GetSecurityInfo(
            handle.raw(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            None,
            None,
            None,
            None,
            Some(&mut sd),
        )
    };
    if err.is_err() {
        return Err(format!("security cannot be read: {err:?}"));
    }
    Ok(SecurityDescriptor(sd))
}

/// The owner, the protection and the entries of a directory's descriptor.
fn check_descriptor(sd: &SecurityDescriptor) -> Result<(), String> {
    let owner = owner(sd)?;
    if owner != ADMINISTRATORS && owner != SYSTEM {
        return Err(format!("is owned by {owner}, not Administrators or SYSTEM"));
    }
    let (mut control, mut revision) = (0u16, 0u32);
    // SAFETY: `sd` is a valid descriptor.
    unsafe { GetSecurityDescriptorControl(sd.0, &mut control, &mut revision) }.map_err(unreadable)?;
    if control & SE_DACL_PROTECTED.0 == 0 {
        return Err("its DACL is not protected (it inherits from its parent)".into());
    }
    // Only allow entries for SYSTEM and Administrators; deny entries only take
    // access away and are accepted; any other kind is refused.
    for e in entries(sd)? {
        match e.kind {
            ACCESS_DENIED_ACE_TYPE => {}
            ACCESS_ALLOWED_ACE_TYPE => {
                let who = e.sid.unwrap_or_default();
                if who != SYSTEM && who != ADMINISTRATORS {
                    return Err(format!("grants access to {who}"));
                }
            }
            other => return Err(unaccepted(other)),
        }
    }
    Ok(())
}

/// The owner and the entries of a folder above the directory: no one but
/// SYSTEM, Administrators and TrustedInstaller may swap it.
fn check_ancestor_descriptor(sd: &SecurityDescriptor) -> Result<(), String> {
    let trusted = |sid: &str| [SYSTEM, ADMINISTRATORS, TRUSTED_INSTALLER].contains(&sid);
    let owner = owner(sd)?;
    if !trusted(&owner) {
        return Err(format!("is owned by {owner}, not SYSTEM, Administrators or TrustedInstaller"));
    }
    for e in entries(sd)? {
        match e.kind {
            ACCESS_DENIED_ACE_TYPE => {}
            // Rights the folder itself does not get are not checked.
            ACCESS_ALLOWED_ACE_TYPE if e.flags & INHERIT_ONLY_ACE != 0 || e.mask & SWAP_RIGHTS == 0 => {}
            ACCESS_ALLOWED_ACE_TYPE => {
                let who = e.sid.unwrap_or_default();
                if !trusted(&who) {
                    return Err(format!("lets {who} rename or re-permission it or what it contains"));
                }
            }
            other => return Err(unaccepted(other)),
        }
    }
    Ok(())
}

fn unreadable(e: windows::core::Error) -> String {
    format!("security cannot be read: {e}")
}

fn unaccepted(kind: u8) -> String {
    format!("has an entry of type {kind}, which the agent does not accept")
}

fn owner(sd: &SecurityDescriptor) -> Result<String, String> {
    let mut owner = PSID::default();
    let mut defaulted = BOOL::default();
    // SAFETY: `sd` is a valid descriptor; `owner` points into it.
    unsafe { GetSecurityDescriptorOwner(sd.0, &mut owner, &mut defaulted) }.map_err(unreadable)?;
    if owner.0.is_null() {
        return Err("has no owner".into());
    }
    sid_string(owner)
}

/// One DACL entry: its type, flags and mask, and for an allow entry its SID.
struct Entry {
    kind: u8,
    flags: u8,
    mask: u32,
    sid: Option<String>,
}

/// The entries of the descriptor's DACL; a missing or null DACL is refused.
fn entries(sd: &SecurityDescriptor) -> Result<Vec<Entry>, String> {
    let (mut present, mut dacl, mut defaulted) = (BOOL::default(), std::ptr::null_mut::<ACL>(), BOOL::default());
    // SAFETY: `sd` is a valid descriptor; `dacl` points into it.
    unsafe { GetSecurityDescriptorDacl(sd.0, &mut present, &mut dacl, &mut defaulted) }.map_err(unreadable)?;
    if !present.as_bool() || dacl.is_null() {
        return Err("has a null DACL (everyone has full access)".into());
    }
    let mut size = ACL_SIZE_INFORMATION::default();
    // SAFETY: `dacl` points into a live security descriptor; `size` is the
    // structure `AclSizeInformation` asks for.
    unsafe {
        GetAclInformation(dacl, (&raw mut size).cast(), size_of::<ACL_SIZE_INFORMATION>() as u32, AclSizeInformation)
    }
    .map_err(|e| format!("its DACL cannot be read: {e}"))?;
    let mut out = Vec::new();
    for i in 0..size.AceCount {
        let mut ace: *mut core::ffi::c_void = std::ptr::null_mut();
        // SAFETY: `i` is below the ACL's count; `ace` points into the ACL.
        unsafe { GetAce(dacl, i, &mut ace) }.map_err(|e| format!("its DACL cannot be read: {e}"))?;
        // SAFETY: every ACE starts with an ACE_HEADER.
        let header = unsafe { &*(ace as *const ACE_HEADER) };
        let (mut mask, mut sid) = (0, None);
        if header.AceType == ACCESS_ALLOWED_ACE_TYPE {
            let allowed = ace as *const ACCESS_ALLOWED_ACE;
            // SAFETY: an allowed ACE: its mask follows the header, and its SID
            // starts at `SidStart`, inside the ACE.
            mask = unsafe { (*allowed).Mask };
            sid = Some(sid_string(PSID(unsafe { (&raw const (*allowed).SidStart) } as *mut _))?);
        }
        out.push(Entry { kind: header.AceType, flags: header.AceFlags, mask, sid });
    }
    Ok(out)
}

fn sid_string(sid: PSID) -> Result<String, String> {
    let mut s = PWSTR::null();
    // SAFETY: `sid` is a valid SID; the string is freed below with LocalFree.
    unsafe { ConvertSidToStringSidW(sid, &mut s) }.map_err(|e| format!("a SID cannot be read: {e}"))?;
    // SAFETY: a NUL-terminated string allocated by the call above.
    let out = unsafe { s.to_string() }.map_err(|e| e.to_string());
    // SAFETY: allocated with LocalAlloc by ConvertSidToStringSidW.
    unsafe { LocalFree(Some(HLOCAL(s.0.cast()))) };
    out
}

#[cfg(test)]
mod tests {
    use windows::Win32::Security::Authorization::SDDL_REVISION_1;

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("atlas-datadir-{name}-{}", std::process::id()));
        let _ = std::process::Command::new("cmd").args(["/c", "rmdir", "/s", "/q"]).arg(&d).output();
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Creates `dir` with `sddl` (needs administrator rights when it names an owner).
    fn create_with(dir: &Path, sddl: &str) {
        let sd = SecurityDescriptor::from_sddl(sddl).unwrap();
        let attrs = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0.0,
            bInheritHandle: false.into(),
        };
        let path = wide(&dir.to_string_lossy());
        // SAFETY: as in `create_or_verify`.
        unsafe { CreateDirectoryW(PCWSTR(path.as_ptr()), Some(&attrs)) }.unwrap();
    }

    /// The descriptor Windows actually gave `path`, as SDDL (owner and DACL).
    fn actual_sddl(path: &Path) -> String {
        use windows::Win32::Security::Authorization::{
            ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW,
        };
        let name = wide(&path.to_string_lossy());
        let info = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: a NUL-terminated name; the descriptor is freed by `SecurityDescriptor`.
        let err = unsafe {
            GetNamedSecurityInfoW(PCWSTR(name.as_ptr()), SE_FILE_OBJECT, info, None, None, None, None, &mut sd)
        };
        if err.is_err() {
            return format!("unreadable: {err:?}");
        }
        let sd = SecurityDescriptor(sd);
        let mut text = PWSTR::null();
        // SAFETY: a valid descriptor; the string is freed with LocalFree.
        unsafe { ConvertSecurityDescriptorToStringSecurityDescriptorW(sd.0, SDDL_REVISION_1, info, &mut text, None) }
            .unwrap();
        // SAFETY: a NUL-terminated string allocated by the call above.
        let out = unsafe { text.to_string() }.unwrap();
        // SAFETY: allocated with LocalAlloc.
        unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
        out
    }

    /// What an elevated `mkdir` makes: owner Administrators, the parent's ACL inherited.
    fn create_inheriting_owned_by_administrators(dir: &Path) {
        use windows::Win32::Security::Authorization::{ConvertStringSidToSidW, SetNamedSecurityInfoW};
        std::fs::create_dir(dir).unwrap();
        let mut sid = PSID::default();
        // SAFETY: a NUL-terminated SID string; the SID is freed with LocalFree.
        unsafe { ConvertStringSidToSidW(PCWSTR(wide(ADMINISTRATORS).as_ptr()), &mut sid) }.unwrap();
        let name = wide(&dir.to_string_lossy());
        // SAFETY: a NUL-terminated name and a valid SID; only the owner changes.
        let err = unsafe {
            SetNamedSecurityInfoW(
                PCWSTR(name.as_ptr()),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION,
                Some(sid),
                None,
                None,
                None,
            )
        };
        // SAFETY: allocated with LocalAlloc by ConvertStringSidToSidW.
        unsafe { LocalFree(Some(HLOCAL(sid.0))) };
        assert!(err.is_ok(), "{err:?}");
    }

    fn reason(dir: &Path) -> String {
        verify(dir).unwrap_err().reason
    }

    /// A folder under C:\ProgramData that only SYSTEM and Administrators
    /// control (made elevated), deleted on drop, also on failure.
    struct AdminScratch(PathBuf);

    impl AdminScratch {
        fn new(name: &str) -> Self {
            let d = PathBuf::from(format!(r"C:\ProgramData\Atlas-Test-datadir-{name}-{}", std::process::id()));
            let _ = std::process::Command::new("cmd").args(["/c", "rmdir", "/s", "/q"]).arg(&d).output();
            create_with(&d, SDDL);
            AdminScratch(d)
        }
    }

    impl Drop for AdminScratch {
        fn drop(&mut self) {
            let _ = std::process::Command::new("cmd").args(["/c", "rmdir", "/s", "/q"]).arg(&self.0).output();
        }
    }

    /// The checks on descriptors built from SDDL: no directory, no elevation.
    #[test]
    fn descriptors_are_checked() {
        let check = |sddl: &str| check_descriptor(&SecurityDescriptor::from_sddl(sddl).unwrap());
        assert_eq!(check(SDDL), Ok(()));
        assert_eq!(check("O:SYD:P(A;OICI;FA;;;SY)"), Ok(()), "owned by SYSTEM");
        assert_eq!(check("O:BAD:P(D;OICI;FW;;;WD)(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)"), Ok(()), "a deny entry");
        for (sddl, needle) in [
            ("O:BAD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FR;;;BU)", "grants access to S-1-5-32-545"),
            ("O:BAD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;WD)", "grants access to S-1-1-0"),
            ("O:BAD:PNO_ACCESS_CONTROL", "null DACL"),
            ("O:BAD:AI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)", "not protected"),
            ("O:BUD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)", "owned by S-1-5-32-545"),
            ("D:PAI(A;OICI;FA;;;SY)", "no owner"),
            // Review finding 8: what files inside would inherit, and kinds the
            // check does not read (a conditional and an object entry).
            ("O:BAD:PAI(A;OICI;FA;;;SY)(A;OICIIO;FA;;;BU)", "grants access to S-1-5-32-545"),
            ("O:BAD:PAI(A;OICI;FA;;;SY)(XA;;FA;;;WD;(Member_of {SID(BA)}))", "entry of type 9"),
            ("O:BAD:PAI(A;OICI;FA;;;SY)(OA;;FA;bf967aba-0de6-11d0-a285-00aa003049e2;;BU)", "entry of type 5"),
        ] {
            let r = check(sddl);
            assert!(r.as_ref().is_err_and(|e| e.contains(needle)), "{sddl}: {r:?}");
        }
    }

    /// The checks on the folders above, on descriptors built from SDDL.
    #[test]
    fn ancestor_descriptors_are_checked() {
        let check = |sddl: &str| check_ancestor_descriptor(&SecurityDescriptor::from_sddl(sddl).unwrap());
        for (sddl, what) in [
            // C:\ as Windows sets it: Users read; Authenticated Users add
            // folders, and modify only what they create (inherit-only).
            (
                "O:S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464D:PAI(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)\
                 (A;OICI;0x1200a9;;;BU)(A;OICIIO;SDGXGWGR;;;AU)(A;;0x4;;;AU)",
                "the volume root",
            ),
            // C:\ProgramData: Users add files and folders, CREATOR OWNER inherit-only.
            (
                "O:SYD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICIIO;GA;;;CO)(A;OICI;0x1200a9;;;BU)(A;CI;0x116;;;BU)",
                "ProgramData",
            ),
            ("O:BAD:P(D;;FA;;;WD)(A;OICI;FA;;;SY)", "a deny entry"),
        ] {
            assert_eq!(check(sddl), Ok(()), "{what}");
        }
        for (sddl, needle) in [
            ("O:BUD:PAI(A;OICI;FA;;;SY)", "is owned by S-1-5-32-545"),
            ("O:BAD:PAI(A;OICI;FA;;;SY)(A;;FA;;;WD)", "lets S-1-1-0"),
            ("O:BAD:PAI(A;OICI;FA;;;SY)(A;;SD;;;BU)", "lets S-1-5-32-545"),
            ("O:BAD:PAI(A;OICI;FA;;;SY)(A;;WD;;;BU)", "lets S-1-5-32-545"),
            ("O:BAD:PAI(A;OICI;FA;;;SY)(A;;WO;;;BU)", "lets S-1-5-32-545"),
            ("O:BAD:PAI(A;OICI;FA;;;SY)(A;;0x40;;;BU)", "lets S-1-5-32-545"),
            ("O:BAD:PAI(A;OICI;FA;;;SY)(A;;GA;;;BU)", "lets S-1-5-32-545"),
            ("O:BAD:PNO_ACCESS_CONTROL", "null DACL"),
            ("D:PAI(A;OICI;FA;;;SY)", "no owner"),
            ("O:BAD:PAI(A;OICI;FA;;;SY)(XA;;FA;;;WD;(Member_of {SID(BA)}))", "entry of type 9"),
            ("O:BAD:PAI(A;OICI;FA;;;SY)(OA;;FA;bf967aba-0de6-11d0-a285-00aa003049e2;;BU)", "entry of type 5"),
        ] {
            let r = check(sddl);
            assert!(r.as_ref().is_err_and(|e| e.contains(needle)), "{sddl}: {r:?}");
        }
    }

    #[test]
    fn a_data_directory_below_the_users_temp_is_refused() {
        let base = scratch("below-temp");
        let dir = base.join("Atlas");
        let e = prepare(&dir).unwrap_err();
        // Refused at the user's own folder, not before (C:\ and C:\Users pass).
        assert!(e.reason.starts_with(r"C:\Users\") && e.reason.contains("a folder above it"), "{e}");
        assert!(!dir.exists(), "nothing is created below it");
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// The real folders above the default directory, as this machine has them.
    #[test]
    fn the_folders_above_the_default_pass() {
        check_ancestors(Path::new(r"C:\ProgramData\Atlas")).unwrap();
    }

    #[test]
    fn a_relative_or_dotted_path_is_refused() {
        // A `.` inside a path is dropped by `Path` and by Windows alike.
        for p in ["Atlas", r"C:\ProgramData\..\Users\Atlas", r"ProgramData\Atlas"] {
            let e = prepare(Path::new(p)).unwrap_err();
            assert!(e.reason.contains("absolute path without"), "{p}: {e}");
        }
    }

    #[test]
    fn a_folder_with_an_inherited_acl_is_refused() {
        let base = scratch("inherited");
        let dir = base.join("Atlas");
        std::fs::create_dir(&dir).unwrap();
        // Unelevated, the owner is the user; elevated, the DACL is inherited.
        let r = reason(&dir);
        assert!(r.contains("owned by") || r.contains("not protected"), "{r}");
        // The elevated tests' diagnostic reads it back: an owner and inherited entries.
        let sddl = actual_sddl(&dir);
        assert!(sddl.starts_with("O:") && sddl.contains("D:") && sddl.contains("ID;FA;"), "{sddl}");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_junction_is_refused_without_following_it() {
        let base = scratch("junction");
        let target = base.join("elsewhere");
        std::fs::create_dir(&target).unwrap();
        let link = base.join("Atlas");
        let out =
            std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(&link).arg(&target).output().unwrap();
        assert!(out.status.success(), "mklink: {out:?}");
        assert!(reason(&link).contains("reparse point"));
        let _ = std::process::Command::new("cmd").args(["/c", "rmdir"]).arg(&link).output();
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_file_is_refused() {
        let base = scratch("file");
        let f = base.join("Atlas");
        std::fs::write(&f, b"x").unwrap();
        assert!(reason(&f).contains("not a directory"));
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    #[ignore = "needs administrator rights; CI's agent-live job runs it"]
    fn prepare_creates_a_protected_directory_and_accepts_it_again() {
        let base = AdminScratch::new("prepare");
        let dir = base.0.join("Atlas");
        prepare(&dir).unwrap();
        for d in [dir.clone(), dir.join("buffer"), dir.join("logs")] {
            verify(&d).unwrap();
        }
        prepare(&dir).unwrap();
        // A file created inside inherits SYSTEM and Administrators only.
        let file = dir.join("buffer").join("probe");
        std::fs::write(&file, b"x").unwrap();
        let sddl = actual_sddl(&file);
        let dacl = sddl.split_once("D:").map_or("", |(_, d)| d);
        let trustees: Vec<&str> =
            dacl.split('(').skip(1).map(|ace| ace.trim_end_matches(')').rsplit(';').next().unwrap()).collect();
        assert!(!trustees.is_empty() && trustees.iter().all(|t| *t == "SY" || *t == "BA"), "{sddl}");
    }

    #[test]
    #[ignore = "needs administrator rights; CI's agent-live job runs it"]
    fn folders_above_that_others_control_are_refused() {
        let base = AdminScratch::new("above");
        // Making Users the owner takes the restore privilege.
        assert!(super::super::privilege::enable("SeRestorePrivilege"));
        let admin = "(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";
        let mut wrong = Vec::new();
        for (name, sddl, needle) in [
            ("owned", format!("O:BUD:P{admin}"), Some("is owned by S-1-5-32-545")),
            ("delete", format!("O:BAD:P{admin}(A;;SD;;;BU)"), Some("lets S-1-5-32-545")),
            ("children", format!("O:BAD:P{admin}(A;;0x40;;;BU)"), Some("lets S-1-5-32-545")),
            ("dacl", format!("O:BAD:P{admin}(A;;WD;;;BU)"), Some("lets S-1-5-32-545")),
            ("inherit-only", format!("O:BAD:P{admin}(A;OICIIO;FA;;;BU)"), None),
        ] {
            let parent = base.0.join(name);
            create_with(&parent, &sddl);
            let dir = parent.join("Atlas");
            let r = prepare(&dir);
            let ok = match needle {
                Some(n) => r.as_ref().is_err_and(|e| e.reason.contains(n)) && !dir.exists(),
                None => r.is_ok(),
            };
            if !ok {
                wrong.push(format!("{name}: got {}: {r:?}", actual_sddl(&parent)));
            }
        }
        assert!(wrong.is_empty(), "{wrong:#?}");

        // A junction above is refused, not followed.
        let target = base.0.join("target");
        create_with(&target, &format!("O:BAD:P{admin}"));
        let link = base.0.join("link");
        let out =
            std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(&link).arg(&target).output().unwrap();
        assert!(out.status.success(), "mklink: {out:?}");
        let e = prepare(&link.join("Atlas")).unwrap_err();
        assert!(e.reason.contains("reparse point"), "{e}");
        assert!(!target.join("Atlas").exists(), "nothing is created through it");
        let _ = std::process::Command::new("cmd").args(["/c", "rmdir"]).arg(&link).output();
    }

    #[test]
    #[ignore = "needs administrator rights; CI's agent-live job runs it"]
    fn loose_protected_directories_are_refused() {
        let base = scratch("loose");
        // Making Users the owner takes the restore privilege.
        assert!(super::super::privilege::enable("SeRestorePrivilege"));
        let mut wrong = Vec::new();
        for (name, sddl, needle) in [
            ("users", "O:BAD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FR;;;BU)", "grants access to S-1-5-32-545"),
            ("null", "O:BAD:PNO_ACCESS_CONTROL", "null DACL"),
            ("owner", "O:BUD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)", "owned by S-1-5-32-545"),
        ] {
            let dir = base.join(name);
            create_with(&dir, sddl);
            let r = verify(&dir);
            let actual = actual_sddl(&dir);
            println!("{name}: asked {sddl}, got {actual}: {r:?}");
            if !r.as_ref().is_err_and(|e| e.reason.contains(needle)) {
                wrong.push(format!("{name}: got {actual}: {r:?}"));
            }
        }
        // An explicit DACL given at creation is made protected (NTFS), so the
        // inheriting case is made the way an elevated `mkdir` makes it.
        let open = base.join("open");
        create_inheriting_owned_by_administrators(&open);
        let r = verify(&open);
        println!("open: got {}: {r:?}", actual_sddl(&open));
        if !r.as_ref().is_err_and(|e| e.reason.contains("not protected")) {
            wrong.push(format!("open: got {}: {r:?}", actual_sddl(&open)));
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
        // A deny entry only takes access away (here: adding files, so the
        // directory can still be deleted).
        let deny = base.join("deny");
        create_with(&deny, "O:BAD:P(D;OICI;0x2;;;WD)(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)");
        verify(&deny).unwrap();
        std::fs::remove_dir_all(&base).unwrap();
    }
}
