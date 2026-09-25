//! Size limits enforced on decode (spec section 6.1).
//!
//! An honest sensor truncates to fit (use [`truncate_utf8`]) and sets the
//! matching `*_truncated` flag; the validator rejects anything over a limit.

/// `process.cmd_line`, in UTF-8 bytes.
pub const CMD_LINE_MAX: usize = 64 * 1024;
/// Any path or name: file paths/names, registry key paths, registry value names.
pub const PATH_MAX: usize = 32 * 1024;
/// `reg_value.data`, in bytes.
pub const REG_DATA_MAX: usize = 4 * 1024;
/// `query.hostname`, in UTF-8 bytes.
pub const DNS_HOSTNAME_MAX: usize = 1024;
/// Each `answers[].data`, in UTF-8 bytes.
pub const DNS_ANSWER_DATA_MAX: usize = 1024;
/// Number of `answers[]` entries.
pub const DNS_ANSWERS_MAX: usize = 64;
/// `user.uid` (a SID string; the longest real SID is under 200 characters).
pub const USER_UID_MAX: usize = 256;
/// `user.name` (`DOMAIN\user`).
pub const USER_NAME_MAX: usize = 1024;
/// `file.signature.signer`.
pub const SIGNER_MAX: usize = 1024;
/// A whole encoded event, checked before protobuf decoding.
pub const EVENT_MAX: usize = 256 * 1024;

/// Truncates `s` to at most `max_bytes` UTF-8 bytes without splitting a
/// character. Returns the kept prefix and whether anything was cut.
pub fn truncate_utf8(s: &str, max_bytes: usize) -> (&str, bool) {
    if s.len() <= max_bytes {
        return (s, false);
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    (&s[..end], true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_string_is_untouched() {
        assert_eq!(truncate_utf8("abc", 3), ("abc", false));
    }

    #[test]
    fn long_ascii_is_cut_exactly() {
        assert_eq!(truncate_utf8("abcdef", 4), ("abcd", true));
    }

    #[test]
    fn never_splits_a_multibyte_char() {
        // "é" is 2 bytes; a 2-byte budget after "a" would split it.
        assert_eq!(truncate_utf8("aé", 2), ("a", true));
        // "😀" is 4 bytes.
        assert_eq!(truncate_utf8("😀x", 3), ("", true));
    }
}
