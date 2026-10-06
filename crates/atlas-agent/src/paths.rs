//! Path normalization (sensor spec §5.5, §7.2).

use atlas_schema::limits::truncate_utf8;

/// `s` cut to at most `max` UTF-8 bytes on a character boundary. The schema
/// rejects longer strings, and paths and names have no `*_truncated` flag, so a
/// pathological NT path (up to 32,767 UTF-16 units) is cut rather than making
/// the event invalid.
pub fn fit(s: String, max: usize) -> String {
    match truncate_utf8(&s, max) {
        (_, false) => s,
        (t, true) => t.to_string(),
    }
}

/// Registry NT names to the forms Sigma uses (§5.5): `\REGISTRY\MACHINE\…` →
/// `HKLM\…`, `\REGISTRY\USER\…` → `HKU\…`, and `HKLM\SYSTEM\ControlSet00N\…` →
/// `HKLM\SYSTEM\CurrentControlSet\…` when N is the current control set.
/// The prefixes match case-insensitively (the kernel logs both `\REGISTRY\MACHINE`
/// and `\Registry\Machine`); the rest keeps its case.
pub fn registry(nt: &str, current_control_set: u32) -> String {
    let (root, rest) = if let Some(r) = strip_prefix_ci(nt, r"\REGISTRY\MACHINE") {
        ("HKLM", r)
    } else if let Some(r) = strip_prefix_ci(nt, r"\REGISTRY\USER") {
        ("HKU", r)
    } else {
        return nt.to_string();
    };
    if !(rest.is_empty() || rest.starts_with('\\')) {
        return nt.to_string(); // `\REGISTRY\MACHINEX` is not HKLM
    }
    let mut out = String::with_capacity(nt.len());
    out.push_str(root);
    let current = format!(r"\SYSTEM\ControlSet{current_control_set:03}");
    match (root, strip_prefix_ci(rest, &current)) {
        ("HKLM", Some(tail)) if tail.is_empty() || tail.starts_with('\\') => {
            out.push_str(r"\SYSTEM\CurrentControlSet");
            out.push_str(tail);
        }
        _ => out.push_str(rest),
    }
    out
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &s[prefix.len()..])
}

/// The path with its volume removed, for watchlist matching (§7.2):
/// `\Device\HarddiskVolumeShadowCopy3\Windows\x` → `\Windows\x`. A path that
/// is not `\Device\<volume>\…` is returned as is.
pub fn volume_relative(nt: &str) -> &str {
    let Some(rest) = strip_prefix_ci(nt, r"\Device\") else { return nt };
    match rest.find('\\') {
        Some(i) => &rest[i..],
        None => "",
    }
}

/// Removes an alternate data stream from the last component (§7.2):
/// `file.txt:stream` and `file::$DATA` match `file.txt` and `file`.
pub fn strip_stream(path: &str) -> &str {
    let start = path.rfind('\\').map_or(0, |i| i + 1);
    match path[start..].find(':') {
        Some(i) => &path[..start + i],
        None => path,
    }
}

/// Whether a component looks like an 8.3 short name (§7.2):
/// `^[^.~]{1,6}~[0-9]+(\.[^.]{0,3})?$`.
pub fn is_short_name(component: &str) -> bool {
    let (base, ext) = match component.split_once('.') {
        Some((b, e)) => (b, Some(e)),
        None => (component, None),
    };
    if ext.is_some_and(|e| e.len() > 3 || e.contains('.')) {
        return false;
    }
    let Some((stem, digits)) = base.split_once('~') else { return false };
    (1..=6).contains(&stem.chars().count())
        && !stem.contains(['.', '~'])
        && !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
}

/// Whether any component of the path is an 8.3 short name.
pub fn has_short_name(path: &str) -> bool {
    path.split('\\').any(is_short_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_cuts_on_a_character_boundary() {
        assert_eq!(fit("abc".into(), 3), "abc");
        assert_eq!(fit("aé".into(), 2), "a"); // é is two bytes
    }

    #[test]
    fn registry_roots_and_control_set() {
        assert_eq!(registry(r"\REGISTRY\MACHINE\SOFTWARE\x", 1), r"HKLM\SOFTWARE\x");
        assert_eq!(registry(r"\Registry\Machine\Software\x", 1), r"HKLM\Software\x");
        assert_eq!(registry(r"\REGISTRY\USER\S-1-5-21-1-2-3-1001\Software", 1), r"HKU\S-1-5-21-1-2-3-1001\Software");
        assert_eq!(
            registry(r"\REGISTRY\MACHINE\SYSTEM\ControlSet001\Services\x", 1),
            r"HKLM\SYSTEM\CurrentControlSet\Services\x"
        );
        assert_eq!(registry(r"\REGISTRY\MACHINE\SYSTEM\ControlSet001", 1), r"HKLM\SYSTEM\CurrentControlSet");
        // Not the current set, and not a component boundary.
        assert_eq!(registry(r"\REGISTRY\MACHINE\SYSTEM\ControlSet002\x", 1), r"HKLM\SYSTEM\ControlSet002\x");
        assert_eq!(registry(r"\REGISTRY\MACHINE\SYSTEM\ControlSet0011", 1), r"HKLM\SYSTEM\ControlSet0011");
        assert_eq!(registry(r"\REGISTRY\MACHINE", 1), "HKLM");
        assert_eq!(registry(r"\REGISTRY\MACHINEX\a", 1), r"\REGISTRY\MACHINEX\a");
        assert_eq!(registry(r"\REGISTRY\A\{guid}", 1), r"\REGISTRY\A\{guid}");
        assert_eq!(registry(r"Software\Relative", 1), r"Software\Relative");
    }

    #[test]
    fn volume_relative_paths() {
        assert_eq!(
            volume_relative(r"\Device\HarddiskVolume3\Windows\System32\config\SAM"),
            r"\Windows\System32\config\SAM"
        );
        assert_eq!(
            volume_relative(r"\Device\HarddiskVolumeShadowCopy7\Windows\NTDS\ntds.dit"),
            r"\Windows\NTDS\ntds.dit"
        );
        assert_eq!(volume_relative(r"\Device\HarddiskVolume3"), "");
        assert_eq!(volume_relative(r"C:\x"), r"C:\x");
    }

    #[test]
    fn streams_are_stripped_from_the_last_component_only() {
        assert_eq!(strip_stream(r"\a\file.txt:secret"), r"\a\file.txt");
        assert_eq!(strip_stream(r"\a\file::$DATA"), r"\a\file");
        assert_eq!(strip_stream(r"\a\file.txt"), r"\a\file.txt");
    }

    #[test]
    fn short_names() {
        for s in ["ATLASS~1", "LONG-F~1.TXT", "a~12", "PROGRA~1", "x~1.c"] {
            assert!(is_short_name(s), "{s}");
        }
        for s in ["readme.txt", "a~", "~1", "TOOLONGN~1", "a.b~1", "a~1.long", "a~x", "a~1.b.c"] {
            assert!(!is_short_name(s), "{s}");
        }
        assert!(has_short_name(r"\Device\HarddiskVolume3\Users\ATLASS~1\x.txt"));
        assert!(!has_short_name(r"\Device\HarddiskVolume3\Users\jake\x.txt"));
    }
}
