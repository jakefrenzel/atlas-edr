//! The single-instance lock (sensor spec §11.3; plan 1b-4a): `run` cannot tear
//! down the service's sessions, nor a second `run` the first one's.
//!
//! A mutex in `Global\` will not do: creating a mutex there takes no
//! privilege, so any user could take the name first and keep the agent from
//! ever starting. The mutex ([`MUTEX`]) lives instead in a private namespace
//! whose boundary holds the Administrators SID:
//! - only a token with Administrators enabled (elevated, or SYSTEM) can create
//!   that namespace;
//! - its DACL ([`SDDL`]) lets only SYSTEM and Administrators open it;
//! - a namespace of the same name with another boundary (no SID, a user's SID)
//!   is a different namespace, so squatting the name blocks nothing;
//! - private namespaces are not per session: the service in session 0 and
//!   `run` in a user's session find the same one.
//!
//! The namespace goes away with its last handle, so a process that dies
//! without closing it does not block the next start.

use std::fmt;

use windows::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_PATH_NOT_FOUND, GetLastError, HANDLE, WIN32_ERROR,
};
use windows::Win32::Security::{
    CreateWellKnownSid, PSID, SECURITY_ATTRIBUTES, SECURITY_MAX_SID_SIZE, WinBuiltinAdministratorsSid,
};
use windows::Win32::System::Threading::{
    AddSIDToBoundaryDescriptor, ClosePrivateNamespace, CreateBoundaryDescriptorW, CreateMutexW,
    CreatePrivateNamespaceW, DeleteBoundaryDescriptor, OpenPrivateNamespaceW,
};
use windows::core::PCWSTR;

use super::util::{Owned, SecurityDescriptor, wide};

/// The boundary's name, and the namespace's alias in this process.
pub const NAMESPACE: &str = "Atlas";
/// The mutex, inside the namespace (as named through [`NAMESPACE`]).
pub const MUTEX: &str = r"Atlas\Agent";
const MUTEX_LEAF: &str = "Agent";
/// The namespace's and the mutex's DACL: SYSTEM and Administrators only, so
/// the service and an elevated `run` can each open what the other created.
pub const SDDL: &str = "D:P(A;;GA;;;SY)(A;;GA;;;BA)";

/// Held for the life of the agent; dropping it releases the lock.
#[derive(Debug)]
pub struct Instance {
    // Field order is drop order: the mutex, then the namespace it lives in.
    _mutex: Owned,
    _namespace: Namespace,
}

/// Why the lock was not taken.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// Another instance holds it.
    Running,
    /// The token has no enabled Administrators SID.
    Denied,
    Failed(String),
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refused::Running => write!(f, "another instance is running ({MUTEX} is held)"),
            Refused::Denied => write!(f, "the single-instance lock ({MUTEX}) needs administrator rights"),
            Refused::Failed(e) => write!(f, "the single-instance lock ({MUTEX}) cannot be taken: {e}"),
        }
    }
}

/// Takes the lock, or says why not.
pub fn acquire() -> Result<Instance, Refused> {
    acquire_as(NAMESPACE)
}

/// [`acquire`] with the namespace under `alias`. An alias names the namespace
/// in one process only and cannot be used twice there at once
/// (`ERROR_DUP_NAME`), so tests that hold the lock in-process take it again
/// under another alias, as a second process would.
fn acquire_as(alias: &str) -> Result<Instance, Refused> {
    let namespace = namespace(alias)?;
    let sd = SecurityDescriptor::from_sddl(SDDL).map_err(Refused::Failed)?;
    let sa = attributes(&sd);
    let name = wide(&format!(r"{alias}\{MUTEX_LEAF}"));
    // SAFETY: `name` is NUL-terminated and `sa` outlives the call; the handle
    // is closed by `Owned`.
    let created = unsafe { CreateMutexW(Some(&sa), false, PCWSTR(name.as_ptr())) };
    // SAFETY: no arguments; read at once after the call above.
    let last = unsafe { GetLastError() };
    match created {
        Ok(h) if last == ERROR_ALREADY_EXISTS => {
            drop(Owned(h));
            Err(Refused::Running)
        }
        Ok(h) => Ok(Instance { _mutex: Owned(h), _namespace: namespace }),
        // Held by an instance whose DACL differs: still another instance.
        Err(e) if e.code() == ERROR_ACCESS_DENIED.to_hresult() => Err(Refused::Running),
        Err(e) => Err(Refused::Failed(format!("mutex: {e}"))),
    }
}

/// Creates the namespace, or opens the one another instance created.
///
/// A token without Administrators enabled is refused at the create, whether
/// the namespace exists or not: the boundary is checked first. The open is
/// reached only by a token the DACL ([`SDDL`]) admits.
fn namespace(alias: &str) -> Result<Namespace, Refused> {
    let boundary = Boundary::administrators()?;
    let sd = SecurityDescriptor::from_sddl(SDDL).map_err(Refused::Failed)?;
    let sa = attributes(&sd);
    let alias = wide(alias);
    // A holder that exits between a refused create and the open: try again.
    for _ in 0..3 {
        // SAFETY: the boundary is valid for the call; `alias` is NUL-terminated;
        // `sa` outlives the call. The handle is closed by `Namespace`.
        let h = unsafe { CreatePrivateNamespaceW(Some(&sa), boundary.0.0, PCWSTR(alias.as_ptr())) };
        if !h.is_invalid() && !h.0.is_null() {
            return Ok(Namespace(h));
        }
        // SAFETY: no arguments; read at once after the call above.
        match unsafe { GetLastError() } {
            ERROR_ALREADY_EXISTS => {}
            ERROR_ACCESS_DENIED => return Err(Refused::Denied),
            e => return Err(failed("create the namespace", e)),
        }
        // SAFETY: as above, without the attributes.
        let h = unsafe { OpenPrivateNamespaceW(boundary.0.0, PCWSTR(alias.as_ptr())) };
        if !h.is_invalid() && !h.0.is_null() {
            return Ok(Namespace(h));
        }
        // SAFETY: no arguments; read at once after the call above.
        match unsafe { GetLastError() } {
            ERROR_PATH_NOT_FOUND => continue,
            e => return Err(failed("open the namespace", e)),
        }
    }
    Err(Refused::Failed("the namespace kept vanishing between create and open".into()))
}

fn attributes(sd: &SecurityDescriptor) -> SECURITY_ATTRIBUTES {
    SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0.0,
        bInheritHandle: false.into(),
    }
}

fn failed(what: &str, e: WIN32_ERROR) -> Refused {
    Refused::Failed(format!("{what}: {}", windows::core::Error::from(e.to_hresult())))
}

/// A boundary descriptor, deleted on drop.
struct Boundary(HANDLE);

impl Boundary {
    /// Named [`NAMESPACE`], holding the Administrators SID.
    fn administrators() -> Result<Boundary, Refused> {
        let name = wide(NAMESPACE);
        // SAFETY: `name` is NUL-terminated; the descriptor is deleted on drop.
        let h = unsafe { CreateBoundaryDescriptorW(PCWSTR(name.as_ptr()), 0) };
        if h.is_invalid() || h.0.is_null() {
            // SAFETY: no arguments; read at once after the call above.
            return Err(failed("boundary descriptor", unsafe { GetLastError() }));
        }
        let mut boundary = Boundary(h);
        let mut sid = [0u8; SECURITY_MAX_SID_SIZE as usize];
        let mut len = sid.len() as u32;
        let psid = PSID(sid.as_mut_ptr().cast());
        // SAFETY: `sid` has room for any SID, `len` says so.
        unsafe { CreateWellKnownSid(WinBuiltinAdministratorsSid, None, Some(psid), &mut len) }
            .map_err(|e| Refused::Failed(format!("Administrators SID: {e}")))?;
        // SAFETY: the descriptor may be reallocated, which updates `boundary.0`;
        // the SID is copied into it.
        unsafe { AddSIDToBoundaryDescriptor(&mut boundary.0, psid) }
            .map_err(|e| Refused::Failed(format!("boundary SID: {e}")))?;
        Ok(boundary)
    }
}

impl Drop for Boundary {
    fn drop(&mut self) {
        // SAFETY: created by CreateBoundaryDescriptorW, deleted only here.
        unsafe { DeleteBoundaryDescriptor(self.0) };
    }
}

/// A private namespace handle, closed (not destroyed) on drop: the namespace
/// goes away with its last handle.
#[derive(Debug)]
struct Namespace(HANDLE);

impl Drop for Namespace {
    fn drop(&mut self) {
        // SAFETY: returned by Create/OpenPrivateNamespaceW, closed only here.
        unsafe { ClosePrivateNamespace(self.0, 0) };
    }
}

// SAFETY: a kernel handle value can be used and closed from any thread.
unsafe impl Send for Namespace {}

#[cfg(test)]
mod tests {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1, SE_KERNEL_OBJECT,
    };
    use windows::Win32::Security::{
        CREATE_RESTRICTED_TOKEN_FLAGS, CreateRestrictedToken, DACL_SECURITY_INFORMATION, ImpersonateLoggedOnUser,
        PSECURITY_DESCRIPTOR, RevertToSelf, SID_AND_ATTRIBUTES, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE,
        TOKEN_IMPERSONATE, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows::core::PWSTR;

    use super::*;

    /// Runs `f` on this thread as this token with Administrators deny-only:
    /// what an unelevated administrator, or a standard user, has.
    fn without_administrators<T>(f: impl FnOnce() -> T) -> T {
        struct Revert;
        impl Drop for Revert {
            fn drop(&mut self) {
                // SAFETY: ends this thread's impersonation.
                unsafe { RevertToSelf() }.unwrap();
            }
        }
        let mut token = HANDLE::default();
        // SAFETY: the process pseudo-handle; the token is closed by `Owned`.
        unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY | TOKEN_IMPERSONATE,
                &mut token,
            )
        }
        .unwrap();
        let token = Owned(token);
        let mut sid = [0u8; SECURITY_MAX_SID_SIZE as usize];
        let mut len = sid.len() as u32;
        let psid = PSID(sid.as_mut_ptr().cast());
        // SAFETY: `sid` has room for any SID.
        unsafe { CreateWellKnownSid(WinBuiltinAdministratorsSid, None, Some(psid), &mut len) }.unwrap();
        let disable = [SID_AND_ATTRIBUTES { Sid: psid, Attributes: 0 }];
        let mut restricted = HANDLE::default();
        // SAFETY: a valid token and SID; the new token is closed by `Owned`.
        unsafe {
            CreateRestrictedToken(
                token.raw(),
                CREATE_RESTRICTED_TOKEN_FLAGS(0),
                Some(&disable),
                None,
                None,
                &mut restricted,
            )
        }
        .unwrap();
        let restricted = Owned(restricted);
        // SAFETY: a token derived from this process's own.
        unsafe { ImpersonateLoggedOnUser(restricted.raw()) }.unwrap();
        let _revert = Revert;
        f()
    }

    fn dacl(h: HANDLE) -> String {
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: a valid handle; the descriptor is freed by `SecurityDescriptor`.
        let err = unsafe {
            GetSecurityInfo(h, SE_KERNEL_OBJECT, DACL_SECURITY_INFORMATION, None, None, None, None, Some(&mut sd))
        };
        assert!(err.is_ok(), "GetSecurityInfo: {err:?}");
        let sd = SecurityDescriptor(sd);
        let mut text = PWSTR::null();
        // SAFETY: a valid descriptor; the string is freed with LocalFree.
        unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                sd.0,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &mut text,
                None,
            )
        }
        .unwrap();
        // SAFETY: a NUL-terminated string allocated by the call above.
        let out = unsafe { text.to_string() }.unwrap();
        // SAFETY: allocated with LocalAlloc.
        unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
        out
    }

    #[test]
    #[ignore = "needs administrator rights; CI's agent-live job runs it"]
    fn the_lock_is_taken_once_until_dropped() {
        let first = acquire().expect("the first instance");
        assert_eq!(acquire_as("Second").unwrap_err(), Refused::Running);
        drop(first);
        let again = acquire().expect("free again once dropped");
        drop(again);
    }

    #[test]
    #[ignore = "needs administrator rights; CI's agent-live job runs it"]
    fn squatted_names_do_not_block_it() {
        // What a standard user can make: the old global mutex, and a namespace
        // of the same name with no SID in its boundary.
        let old = wide(r"Global\Atlas-Agent");
        // SAFETY: NUL-terminated; closed by `Owned`.
        let _old = Owned(unsafe { CreateMutexW(None, false, PCWSTR(old.as_ptr())) }.unwrap());
        let name = wide(NAMESPACE);
        // SAFETY: NUL-terminated; deleted by `Boundary`.
        let bare = Boundary(unsafe { CreateBoundaryDescriptorW(PCWSTR(name.as_ptr()), 0) });
        let alias = wide("Squat");
        // SAFETY: a valid boundary; closed by `Namespace`.
        let squat = Namespace(unsafe { CreatePrivateNamespaceW(None, bare.0.0, PCWSTR(alias.as_ptr())) });
        // SAFETY: no arguments; read at once after the call above.
        let last = unsafe { GetLastError() };
        assert!(!squat.0.0.is_null(), "the squat namespace: {last:?}");
        let inner = wide(r"Squat\Agent");
        // SAFETY: NUL-terminated; closed by `Owned`.
        let _inner = Owned(unsafe { CreateMutexW(None, false, PCWSTR(inner.as_ptr())) }.unwrap());

        let instance = acquire().expect("squats do not block the lock");
        drop(instance);
    }

    #[test]
    #[ignore = "needs administrator rights; CI's agent-live job runs it"]
    fn a_token_without_administrators_is_refused_held_or_not() {
        // Nothing held: the create is refused.
        assert_eq!(without_administrators(acquire).unwrap_err(), Refused::Denied);
        // Held: still refused at the create, not reported as another instance.
        let held = acquire().expect("elevated");
        assert_eq!(without_administrators(|| acquire_as("Second")).unwrap_err(), Refused::Denied);
        drop(held);
    }

    #[test]
    #[ignore = "needs administrator rights; CI's agent-live job runs it"]
    fn a_lock_held_under_another_dacl_is_another_instance() {
        // The namespace, and the mutex with a DACL this token is not in.
        let ns = namespace(NAMESPACE).expect("elevated");
        let sd = SecurityDescriptor::from_sddl("D:P(A;;GA;;;SY)").unwrap();
        let sa = attributes(&sd);
        let name = wide(MUTEX);
        // SAFETY: NUL-terminated; `sa` outlives the call; closed by `Owned`.
        let _other = Owned(unsafe { CreateMutexW(Some(&sa), false, PCWSTR(name.as_ptr())) }.unwrap());
        assert_eq!(acquire_as("Second").unwrap_err(), Refused::Running);
        drop(ns);
    }

    #[test]
    #[ignore = "needs administrator rights; CI's agent-live job runs it"]
    fn the_namespace_and_the_mutex_grant_system_and_administrators_only() {
        let held = acquire().expect("elevated");
        for (what, h) in [("namespace", held._namespace.0), ("mutex", held._mutex.raw())] {
            let d = dacl(h);
            let aces: Vec<&str> = d.split('(').skip(1).collect();
            assert!(d.starts_with("D:P"), "{what} DACL not protected: {d}");
            assert_eq!(aces.len(), 2, "{what} DACL: {d}");
            assert!(aces.iter().any(|a| a.ends_with(";;;SY)")), "{what} DACL: {d}");
            assert!(aces.iter().any(|a| a.ends_with(";;;BA)")), "{what} DACL: {d}");
        }
    }
}
