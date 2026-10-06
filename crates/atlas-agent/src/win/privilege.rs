//! Enabling token privileges (sensor spec §7.4, §7.5).

use windows::Win32::Foundation::{ERROR_NOT_ALL_ASSIGNED, GetLastError, HANDLE, LUID};
use windows::Win32::Security::{
    AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES,
    TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::PCWSTR;

use super::util::{Owned, wide};

/// Object addresses in the handle table, and duplicating other processes' handles (§7.4).
pub const DEBUG: &str = "SeDebugPrivilege";
/// Opening registry keys past their DACL for value reads (§7.5).
pub const BACKUP: &str = "SeBackupPrivilege";

/// Enables `name` in the process token. Returns whether it is now enabled:
/// false when the token does not hold it (an unelevated run).
pub fn enable(name: &str) -> bool {
    let mut token = HANDLE::default();
    // SAFETY: opens our own token; the handle is closed by `Owned`.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token) }.is_err() {
        return false;
    }
    let token = Owned(token);
    let name = wide(name);
    let mut luid = LUID::default();
    // SAFETY: `name` is NUL-terminated and outlives the call.
    if unsafe { LookupPrivilegeValueW(PCWSTR::null(), PCWSTR(name.as_ptr()), &mut luid) }.is_err() {
        return false;
    }
    let tp = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
    };
    // SAFETY: `tp` is a valid one-entry TOKEN_PRIVILEGES. The call succeeds even
    // when the privilege is not held, so the last error tells.
    let ok = unsafe { AdjustTokenPrivileges(token.raw(), false, Some(&tp), 0, None, None) }.is_ok();
    ok && unsafe { GetLastError() } != ERROR_NOT_ALL_ASSIGNED
}
