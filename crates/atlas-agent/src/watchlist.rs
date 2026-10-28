//! The sensitive-path watchlist (sensor spec §7.2): a successful Create on a
//! matching path emits File System Activity `Open`.
//!
//! Patterns are volume-relative and case-insensitive, compiled once into one
//! `GlobSet`. They are matched against the path with its volume and any
//! alternate data stream removed, so they match shadow copies too. `*` stays
//! within one component; `**` crosses components.
//!
//! A pre-check runs first (plan 1b-4a-p2, R3): a path can match a pattern only
//! if it contains the pattern's last literal run (`\Login Data`, `\.ssh\`,
//! `.kdbx`), so one scan for those runs rules out almost every path before the
//! `GlobSet`. Both fold ASCII case only (a test pins the `GlobSet`'s side). A
//! pattern with no literal run turns the pre-check off.

use aho_corasick::AhoCorasick;
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
    /// Every pattern's last literal run; `None` turns the pre-check off.
    must_contain: Option<AhoCorasick>,
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
        // globset reads `/` as `\` (both become `/` before matching); paths keep `\`.
        let runs: Vec<String> =
            base.iter().chain(extend).map(|p| last_literal_run(&p.replace('/', "\\")).to_string()).collect();
        let must_contain = if runs.iter().all(|r| !r.is_empty()) {
            AhoCorasick::builder().ascii_case_insensitive(true).build(&runs).ok()
        } else {
            None
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
        Ok(Watchlist { set, must_contain })
    }

    /// Whether an NT path (`\Device\HarddiskVolume3\…`) is on the list.
    pub fn matches(&self, nt_path: &str) -> bool {
        let p = strip_stream(volume_relative(nt_path));
        if p.is_empty() {
            return false;
        }
        if let Some(runs) = &self.must_contain
            && !runs.is_match(p)
        {
            return false;
        }
        self.set.is_match(slashes(p))
    }
}

/// The last run of literal characters in a pattern: every path it matches
/// contains that run. `*`, `?`, and whatever is inside `[…]` or `{…}` (a class
/// or alternatives, none of them required) end a run. A class is read as
/// globset reads it: a `]` right after `[`, `[!` or `[^` is a member. When in
/// doubt a character counts as wild, which only shortens the run (review 1).
fn last_literal_run(pattern: &str) -> &str {
    let (mut best, mut start, mut braces) = ("", None::<usize>, 0u32);
    // Inside `[…]`: where its members start.
    let mut class: Option<usize> = None;
    for (i, c) in pattern.char_indices() {
        let wild = match (class, c) {
            (Some(m), '!' | '^') if i == m => {
                class = Some(i + 1);
                true
            }
            (Some(m), ']') if i > m => {
                class = None;
                true
            }
            (Some(_), _) => true,
            (None, '[') => {
                class = Some(i + 1);
                true
            }
            (None, '{') => {
                braces += 1;
                true
            }
            (None, '}') => {
                braces = braces.saturating_sub(1);
                true
            }
            (None, ']' | '*' | '?') => true,
            _ => braces > 0,
        };
        match (wild, start) {
            (true, Some(s)) => {
                best = &pattern[s..i];
                start = None;
            }
            (false, None) => start = Some(i),
            _ => {}
        }
    }
    start.map_or(best, |s| &pattern[s..])
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

    /// What the `GlobSet` alone says, without the pre-check.
    fn glob_only(w: &Watchlist, nt_path: &str) -> bool {
        let p = strip_stream(volume_relative(nt_path));
        !p.is_empty() && w.set.is_match(slashes(p))
    }

    #[test]
    fn the_last_literal_runs() {
        assert_eq!(last_literal_run(r"\Users\*\AppData\Local\**\User Data\*\Login Data"), r"\Login Data");
        assert_eq!(last_literal_run(r"\Users\*\.ssh\*"), r"\.ssh\");
        assert_eq!(last_literal_run(r"**\*.kdbx"), ".kdbx");
        assert_eq!(last_literal_run(r"\Windows\NTDS\ntds.dit"), r"\Windows\NTDS\ntds.dit");
        assert_eq!(last_literal_run(r"\a\{x,yz}\b"), r"\b");
        assert_eq!(last_literal_run(r"\a\[xy]"), r"\a\");
        assert_eq!(last_literal_run(r"\a\{x,yz}"), r"\a\");
        assert_eq!(last_literal_run("**"), "");
        // A `]` first in a class is a member (review 1).
        assert_eq!(last_literal_run(r"\x[]a]"), r"\x");
        assert_eq!(last_literal_run(r"\x[!]a]b"), "b");
        assert_eq!(last_literal_run(r"\x[^]]\y"), r"\y");
    }

    #[test]
    fn the_pre_check_never_hides_a_match_of_a_users_pattern() {
        // Review 1: `/` as a separator, and a class that starts with `]`.
        let user = ["/Users/*/secret.txt", "**/*.KEY", r"\x[]a]", r"\q[!]a]z"].map(String::from);
        let w = Watchlist::new(Some(&user), &[]).unwrap();
        assert!(w.must_contain.is_some());
        for p in [
            r"\Device\HarddiskVolume3\Users\jake\secret.txt",
            r"\Device\HarddiskVolume3\a\b.key",
            r"\Device\HarddiskVolume3\x]",
            r"\Device\HarddiskVolume3\xa",
            r"\Device\HarddiskVolume3\qbz",
        ] {
            assert!(glob_only(&w, p), "the GlobSet itself matches {p}");
            assert!(w.matches(p), "{p}");
        }
        for p in [r"\Device\HarddiskVolume3\Users\jake\other.txt", r"\Device\HarddiskVolume3\q]z"] {
            assert_eq!(w.matches(p), glob_only(&w, p), "{p}");
        }
    }

    #[test]
    fn the_pre_check_never_hides_a_match() {
        let w = wl();
        assert!(w.must_contain.is_some(), "the default list has a literal run in every pattern");
        for p in [
            r"\Device\HarddiskVolume3\Windows\System32\config\SAM",
            r"\Device\HarddiskVolume3\windows\system32\CONFIG\sam",
            r"\Device\HarddiskVolume3\Windows\System32\config\RegBack\SAM",
            r"\Device\HarddiskVolume3\Windows\System32\config\SAM.sav",
            r"\Device\HarddiskVolume3\Users\jake\AppData\Local\Google\Chrome\User Data\Default\Login Data",
            r"\Device\HarddiskVolume3\Users\jake\AppData\Local\a\b\User Data\Default\LOGIN DATA",
            r"\Device\HarddiskVolume3\Users\jake\AppData\Local\Google\Chrome\User Data\Default\Login Data-journal",
            r"\Device\HarddiskVolume3\Users\jake\.SSH\id_rsa",
            r"\Device\HarddiskVolume3\Users\jake\.ssh",
            r"\Device\HarddiskVolume3\Users\jake\.azure\x\y\z",
            r"\Device\HarddiskVolume3\Users\jake\Documents\a.kdbx",
            r"\Device\HarddiskVolume3\a.KdBx",
            r"\Device\HarddiskVolume3\Windows\System32\kernel32.dll",
            r"\Device\HarddiskVolume3\Users\jake\AppData\Local\Temp\x.tmp",
            r"\Device\HarddiskVolume3\Windows\System32\config\SAM::$DATA",
        ] {
            assert_eq!(w.matches(p), glob_only(&w, p), "{p}");
        }
    }

    #[test]
    fn the_glob_set_folds_ascii_case_only_as_the_pre_check_does() {
        // If a globset upgrade folds more (the Kelvin sign to `k`, `Ä` to `ä`),
        // the ASCII-only scan would hide matches: this fails first.
        let w = Watchlist::new(Some(&["**\\*.\u{e4}b".to_string(), r"**\*.kdbx".to_string()]), &[]).unwrap();
        assert!(glob_only(&w, r"\Device\HarddiskVolume3\x.KDBX"));
        assert!(!glob_only(&w, "\\Device\\HarddiskVolume3\\x.\u{212A}dbx"));
        assert!(!glob_only(&w, "\\Device\\HarddiskVolume3\\x.\u{c4}B"));
        for p in [
            "\\Device\\HarddiskVolume3\\x.\u{e4}B",
            "\\Device\\HarddiskVolume3\\Users\\j\u{e9}r\u{f4}me\\Documents\\a.KDBX",
        ] {
            assert!(w.matches(p) && glob_only(&w, p), "{p}");
        }
    }

    #[test]
    fn a_pattern_without_a_literal_run_turns_the_pre_check_off() {
        let w = Watchlist::new(None, &["**".to_string()]).unwrap();
        assert!(w.must_contain.is_none());
        assert!(w.matches(r"\Device\HarddiskVolume3\anything\at\all"));
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
