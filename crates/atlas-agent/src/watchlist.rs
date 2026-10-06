//! The sensitive-path watchlist (sensor spec §7.2): a successful Create on a
//! matching path emits File System Activity `Open`.
//!
//! Patterns are volume-relative and case-insensitive, compiled once into one
//! `GlobSet`. They are matched against the path with its volume and any
//! alternate data stream removed, so they match shadow copies too. `*` stays
//! within one component; `**` crosses components.

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use crate::paths::{strip_stream, volume_relative};

/// The built-in list (§7.2), replaceable or extendable in config.
pub const DEFAULT: &[&str] = &[
    // Chromium browsers (Chrome, Edge, Brave, …): saved passwords, cookies, the key.
    r"\Users\*\AppData\Local\**\User Data\*\Login Data",
    r"\Users\*\AppData\Local\**\User Data\*\Cookies",
    r"\Users\*\AppData\Local\**\User Data\*\Network\Cookies",
    r"\Users\*\AppData\Local\**\User Data\Local State",
    // Firefox.
    r"\Users\*\AppData\Roaming\Mozilla\Firefox\Profiles\*\logins.json",
    r"\Users\*\AppData\Roaming\Mozilla\Firefox\Profiles\*\key4.db",
    // Registry hives and their copies.
    r"\Windows\System32\config\SAM",
    r"\Windows\System32\config\SECURITY",
    r"\Windows\System32\config\SYSTEM",
    r"\Windows\System32\config\*.sav",
    r"\Windows\System32\config\*.bak",
    // Active Directory.
    r"\Windows\NTDS\ntds.dit",
    // Keys and cloud credentials.
    r"\Users\*\.ssh\*",
    r"\Users\*\.aws\credentials",
    r"\Users\*\.azure\**",
    r"\Users\*\AppData\Roaming\gcloud\**",
    // KeePass databases anywhere.
    r"**\*.kdbx",
];

pub struct Watchlist {
    set: GlobSet,
}

/// A pattern that does not compile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadPattern {
    pub pattern: String,
    pub error: String,
}

impl Watchlist {
    /// `replace`: use these patterns instead of [`DEFAULT`]; `extend`: add these.
    pub fn new(replace: Option<&[String]>, extend: &[String]) -> Result<Self, BadPattern> {
        let mut b = GlobSetBuilder::new();
        let base: Vec<String> = match replace {
            Some(r) => r.to_vec(),
            None => DEFAULT.iter().map(|s| s.to_string()).collect(),
        };
        for p in base.iter().chain(extend) {
            let glob = GlobBuilder::new(&slashes(p))
                .case_insensitive(true)
                .literal_separator(true)
                .backslash_escape(false)
                .build()
                .map_err(|e| BadPattern { pattern: p.clone(), error: e.to_string() })?;
            b.add(glob);
        }
        let set = b.build().map_err(|e| BadPattern { pattern: String::new(), error: e.to_string() })?;
        Ok(Watchlist { set })
    }

    /// Whether an NT path (`\Device\HarddiskVolume3\…`) is on the list.
    pub fn matches(&self, nt_path: &str) -> bool {
        let p = strip_stream(volume_relative(nt_path));
        !p.is_empty() && self.set.is_match(slashes(p))
    }
}

/// globset separates on `/`; Windows paths use `\`.
fn slashes(s: &str) -> String {
    s.replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wl() -> Watchlist {
        Watchlist::new(None, &[]).unwrap()
    }

    #[test]
    fn the_default_list() {
        let w = wl();
        for p in [
            r"\Device\HarddiskVolume3\Windows\System32\config\SAM",
            r"\Device\HarddiskVolume3\windows\system32\CONFIG\sam",
            r"\Device\HarddiskVolumeShadowCopy4\Windows\System32\config\SYSTEM",
            r"\Device\HarddiskVolume3\Windows\System32\config\SAM.sav",
            r"\Device\HarddiskVolume3\Windows\NTDS\ntds.dit",
            r"\Device\HarddiskVolume3\Users\jake\AppData\Local\Google\Chrome\User Data\Default\Login Data",
            r"\Device\HarddiskVolume3\Users\jake\AppData\Local\Microsoft\Edge\User Data\Profile 1\Network\Cookies",
            r"\Device\HarddiskVolume3\Users\jake\AppData\Local\Google\Chrome\User Data\Local State",
            r"\Device\HarddiskVolume3\Users\jake\AppData\Roaming\Mozilla\Firefox\Profiles\ab.default\key4.db",
            r"\Device\HarddiskVolume3\Users\jake\.ssh\id_ed25519",
            r"\Device\HarddiskVolume3\Users\jake\.aws\credentials",
            r"\Device\HarddiskVolume3\Users\jake\.azure\a\b",
            r"\Device\HarddiskVolume3\Users\jake\Documents\vault.KDBX",
            // Alternate data streams are stripped (§7.2).
            r"\Device\HarddiskVolume3\Windows\System32\config\SAM::$DATA",
        ] {
            assert!(w.matches(p), "{p}");
        }
        for p in [
            r"\Device\HarddiskVolume3\Windows\System32\config\SAMx",
            r"\Device\HarddiskVolume3\Windows\System32\cmd.exe",
            r"\Device\HarddiskVolume3\Users\jake\.ssh", // the directory itself
            r"\Device\HarddiskVolume3\Users\a\b\.ssh\id_rsa", // * is one component
            r"\Device\HarddiskVolume3",
        ] {
            assert!(!w.matches(p), "{p}");
        }
    }

    #[test]
    fn replace_and_extend() {
        let w = Watchlist::new(Some(&[r"\secret\*".to_string()]), &[r"**\*.pem".to_string()]).unwrap();
        assert!(w.matches(r"\Device\HarddiskVolume3\secret\x"));
        assert!(w.matches(r"\Device\HarddiskVolume3\a\b\c.pem"));
        assert!(!w.matches(r"\Device\HarddiskVolume3\Windows\System32\config\SAM"));
    }

    #[test]
    fn a_bad_pattern_is_reported() {
        let e = Watchlist::new(None, &["[".to_string()]).err().unwrap();
        assert_eq!(e.pattern, "[");
    }
}
