//! A bounds-checked cursor over an event payload. Every read checks the
//! remaining length first, so a short or malformed payload is an error, never a
//! panic (sensor spec §4.3).

use super::{ParseError, PointerSize, WStr};

pub(crate) struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    /// Name of the field being read, for error messages.
    field: &'static str,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0, field: "" }
    }

    /// Names the next field; errors from the reads that follow carry it.
    pub(crate) fn field(&mut self, name: &'static str) -> &mut Self {
        self.field = name;
        self
    }

    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    pub(crate) fn rest(&self) -> &'a [u8] {
        &self.data[self.pos..]
    }

    /// The bytes read since position `start`.
    pub(crate) fn consumed_since(&self, start: usize) -> &'a [u8] {
        &self.data[start..self.pos]
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], ParseError> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.data.len());
        match end {
            Some(end) => {
                let s = &self.data[self.pos..end];
                self.pos = end;
                Ok(s)
            }
            None => Err(ParseError::Truncated { field: self.field, offset: self.pos }),
        }
    }

    pub(crate) fn skip(&mut self, n: usize) -> Result<(), ParseError> {
        self.take(n).map(|_| ())
    }

    pub(crate) fn bytes<const N: usize>(&mut self) -> Result<[u8; N], ParseError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    pub(crate) fn u16(&mut self) -> Result<u16, ParseError> {
        self.bytes().map(u16::from_le_bytes)
    }

    /// A 16-bit value in network byte order (manifest out-type `win:Port`).
    pub(crate) fn u16_be(&mut self) -> Result<u16, ParseError> {
        self.bytes().map(u16::from_be_bytes)
    }

    pub(crate) fn u32(&mut self) -> Result<u32, ParseError> {
        self.bytes().map(u32::from_le_bytes)
    }

    pub(crate) fn u64(&mut self) -> Result<u64, ParseError> {
        self.bytes().map(u64::from_le_bytes)
    }

    /// A pointer-sized value, widened to 64 bits.
    pub(crate) fn ptr(&mut self, size: PointerSize) -> Result<u64, ParseError> {
        match size {
            PointerSize::P32 => self.u32().map(u64::from),
            PointerSize::P64 => self.u64(),
        }
    }

    /// A NUL-terminated UTF-16 string (`win:UnicodeString`). The terminator is
    /// consumed and not kept. A missing terminator is an error: the field would
    /// otherwise silently swallow the fields after it.
    pub(crate) fn wstr(&mut self) -> Result<WStr, ParseError> {
        let rest = self.rest();
        let units = rest.len() / 2;
        let end = (0..units).find(|&i| rest[2 * i] == 0 && rest[2 * i + 1] == 0);
        match end {
            Some(n) => {
                let s = WStr::from_le_bytes(&rest[..2 * n]);
                self.pos += 2 * n + 2;
                Ok(s)
            }
            None => Err(ParseError::Unterminated { field: self.field, offset: self.pos }),
        }
    }

    /// A UTF-16 name that may contain embedded NULs, followed by a terminator and
    /// then fields of variable size. The terminator is the **last** NUL unit after
    /// which `trailer_len` accounts for exactly the rest of the payload. Registry
    /// names are counted strings and the kernel logs them whole (`a`, NUL, `b`,
    /// then the terminator), so stopping at the first NUL would misread the
    /// fields after it. Falls back to [`Reader::wstr`] when no NUL fits.
    ///
    /// Also returns whether **another** NUL fits too. Then the split is ambiguous:
    /// the trailer's own data could be read as part of the name. That cannot
    /// happen while the trailer's variable parts are empty (as S4 found for
    /// SetValueKey), but the caller is told rather than guessing silently.
    pub(crate) fn counted_wstr(
        &mut self,
        trailer_len: impl Fn(&[u8]) -> Option<usize>,
    ) -> Result<(WStr, bool), ParseError> {
        let rest = self.rest();
        let mut fits = (0..rest.len() / 2)
            .rev()
            .filter(|&i| rest[2 * i] == 0 && rest[2 * i + 1] == 0)
            .filter(|&i| trailer_len(&rest[2 * i + 2..]) == Some(rest.len() - 2 * i - 2));
        match fits.next() {
            Some(n) => {
                let ambiguous = fits.next().is_some();
                let s = WStr::from_le_bytes(&rest[..2 * n]);
                self.pos += 2 * n + 2;
                Ok((s, ambiguous))
            }
            None => self.wstr().map(|s| (s, false)),
        }
    }

    /// A NUL-terminated 8-bit string (`win:AnsiString`), kept as bytes.
    pub(crate) fn astr(&mut self) -> Result<Box<[u8]>, ParseError> {
        let rest = self.rest();
        match rest.iter().position(|&b| b == 0) {
            Some(n) => {
                let s: Box<[u8]> = rest[..n].into();
                self.pos += n + 1;
                Ok(s)
            }
            None => Err(ParseError::Unterminated { field: self.field, offset: self.pos }),
        }
    }

    /// A SID (`win:SID`): revision, sub-authority count, 6-byte authority, then
    /// 4 bytes per sub-authority. Returned as its raw bytes.
    pub(crate) fn sid(&mut self) -> Result<Sid, ParseError> {
        let start = self.pos;
        let head = self.bytes::<8>()?;
        let count = usize::from(head[1]);
        if head[0] != 1 || count > Sid::MAX_SUB_AUTHORITIES {
            return Err(ParseError::Malformed { field: self.field, offset: start });
        }
        self.skip(4 * count)?;
        Ok(Sid(self.data[start..self.pos].into()))
    }
}

/// A security identifier in its binary form (`SID` structure).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Sid(Box<[u8]>);

impl Sid {
    /// `SID_MAX_SUB_AUTHORITIES` in winnt.h.
    pub const MAX_SUB_AUTHORITIES: usize = 15;

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    fn authority(&self) -> u64 {
        self.0[2..8].iter().fold(0u64, |acc, &b| (acc << 8) | u64::from(b))
    }

    /// The sub-authorities, in order.
    pub fn sub_authorities(&self) -> impl Iterator<Item = u32> + '_ {
        self.0[8..].chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
    }

    /// The last sub-authority (the RID), if any. For a mandatory label
    /// (`S-1-16-X`) this is the integrity level.
    pub fn rid(&self) -> Option<u32> {
        self.sub_authorities().last()
    }

    /// The SID's authority and the first sub-authority, used to recognise
    /// mandatory labels (`S-1-16-…`).
    pub fn is_mandatory_label(&self) -> bool {
        self.authority() == 16 && self.0[1] == 1
    }
}

/// `S-1-5-21-…` form, as `ConvertSidToStringSidW` writes it.
impl std::fmt::Display for Sid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let auth = self.authority();
        if auth < (1 << 32) {
            write!(f, "S-{}-{}", self.0[0], auth)?;
        } else {
            write!(f, "S-{}-0x{:012X}", self.0[0], auth)?;
        }
        for s in self.sub_authorities() {
            write!(f, "-{s}")?;
        }
        Ok(())
    }
}

impl std::fmt::Debug for Sid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Sid({self})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_past_the_end_are_errors_with_the_field_name() {
        let mut r = Reader::new(&[1, 2, 3]);
        assert_eq!(r.field("Status").u16(), Ok(0x0201));
        assert_eq!(r.field("Disposition").u32(), Err(ParseError::Truncated { field: "Disposition", offset: 2 }));
        // A failed read consumes nothing.
        assert_eq!(r.bytes::<1>(), Ok([3]));
    }

    #[test]
    fn a_huge_skip_does_not_overflow() {
        let mut r = Reader::new(&[0; 4]);
        r.skip(2).unwrap();
        assert!(r.skip(usize::MAX).is_err());
    }

    #[test]
    fn wide_strings_need_their_terminator() {
        // "ab\0" then one more unit.
        let mut r = Reader::new(&[b'a', 0, b'b', 0, 0, 0, b'c', 0]);
        assert_eq!(r.wstr().unwrap().to_string_lossy(), "ab");
        assert_eq!(r.pos(), 6);
        assert_eq!(r.field("Name").wstr(), Err(ParseError::Unterminated { field: "Name", offset: 6 }));
    }

    #[test]
    fn a_terminator_must_be_unit_aligned() {
        // Bytes 1..3 are zero but straddle two units: not a terminator.
        let mut r = Reader::new(&[b'a', 0, 0, b'b', 0, 0]);
        assert_eq!(r.wstr().unwrap().as_units(), &[u16::from(b'a'), u16::from(b'b') << 8]);
    }

    #[test]
    fn counted_names_keep_embedded_nuls() {
        // "a", NUL, "b", the terminator, then a 2-byte trailer that must end the payload.
        let b = [b'a', 0, 0, 0, b'b', 0, 0, 0, 0xEE, 0xEE];
        let two = |t: &[u8]| (t.len() >= 2).then_some(2);
        let mut r = Reader::new(&b);
        assert_eq!(r.counted_wstr(two).unwrap(), (WStr::from_units(&[u16::from(b'a'), 0, u16::from(b'b')]), false));
        assert_eq!(r.rest(), &[0xEE, 0xEE]); // the trailer is left for the fields after the name
        // Without embedded NULs it is the ordinary string.
        let mut r = Reader::new(&[b'a', 0, 0, 0, 0xEE, 0xEE]);
        assert_eq!(r.counted_wstr(two).unwrap(), (WStr::from("a"), false));
        // No NUL fits the trailer: the first NUL ends the string, as for wstr().
        let mut r = Reader::new(&[b'a', 0, 0, 0, b'b', 0, 0, 0]);
        assert_eq!(r.counted_wstr(|_| None).unwrap(), (WStr::from("a"), false));
        // Two NULs fit a trailer of any length up to the end: ambiguous, last one wins.
        let mut r = Reader::new(&[b'a', 0, 0, 0, b'b', 0, 0, 0]);
        let any = |t: &[u8]| Some(t.len());
        assert_eq!(r.counted_wstr(any).unwrap(), (WStr::from_units(&[u16::from(b'a'), 0, u16::from(b'b')]), true));
    }

    #[test]
    fn ansi_strings_stop_at_nul() {
        let mut r = Reader::new(b"cmd.exe\0rest");
        assert_eq!(&*r.astr().unwrap(), b"cmd.exe");
        assert_eq!(r.rest(), b"rest");
    }

    #[test]
    fn sids_format_like_windows() {
        // S-1-5-21-1-2-3-1001
        let mut b = vec![1, 5, 0, 0, 0, 0, 0, 5];
        for s in [21u32, 1, 2, 3, 1001] {
            b.extend(s.to_le_bytes());
        }
        let sid = Reader::new(&b).sid().unwrap();
        assert_eq!(sid.to_string(), "S-1-5-21-1-2-3-1001");
        assert_eq!(sid.rid(), Some(1001));
        assert!(!sid.is_mandatory_label());
        // S-1-16-12288 (High integrity)
        let label = Reader::new(&[1, 1, 0, 0, 0, 0, 0, 16, 0, 0x30, 0, 0]).sid().unwrap();
        assert_eq!(label.to_string(), "S-1-16-12288");
        assert!(label.is_mandatory_label());
        assert_eq!(label.rid(), Some(12288));
    }

    #[test]
    fn bad_sids_are_rejected() {
        // Revision 2.
        assert!(Reader::new(&[2, 0, 0, 0, 0, 0, 0, 5]).sid().is_err());
        // 16 sub-authorities.
        assert!(Reader::new(&[1, 16, 0, 0, 0, 0, 0, 5]).sid().is_err());
        // Count says 2, data has 1.
        assert!(Reader::new(&[1, 2, 0, 0, 0, 0, 0, 5, 1, 0, 0, 0]).sid().is_err());
    }
}
