//! Record framing (sensor spec §8.1): `[u32 LE length][u32 LE CRC32C of payload][payload]`.
//!
//! Pure functions over byte slices, so recovery logic can be fuzzed without a file system.

/// Bytes before each payload: length + CRC.
pub const RECORD_HEADER: usize = 8;

/// Largest payload accepted: the 0a encoded-event limit (256 KiB). Checked
/// before anything is allocated or read.
pub const MAX_PAYLOAD: usize = 256 * 1024;

/// Appends one framed record to `out`. The caller has checked `1..=MAX_PAYLOAD`.
pub(crate) fn encode(payload: &[u8], out: &mut Vec<u8>) {
    debug_assert!(!payload.is_empty() && payload.len() <= MAX_PAYLOAD);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&crc32c::crc32c(payload).to_le_bytes());
    out.extend_from_slice(payload);
}

/// What sits at an offset in a segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    /// A whole, valid record: the payload is `data[payload_start..next]`.
    Record { payload_start: usize, next: usize },
    /// The record extends past the end of the data: not written yet, or torn.
    Incomplete,
    /// The length is 0 or above `MAX_PAYLOAD`, or the CRC does not match.
    Invalid,
}

/// Parses the frame starting at `at`. Never panics, never allocates.
pub fn parse(data: &[u8], at: usize) -> Frame {
    let Some(header) = data.get(at..).and_then(|rest| rest.get(..RECORD_HEADER)) else {
        return Frame::Incomplete;
    };
    let len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
    let crc = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    if len == 0 || len > MAX_PAYLOAD {
        return Frame::Invalid;
    }
    let payload_start = at + RECORD_HEADER;
    let Some(payload) = data.get(payload_start..payload_start + len) else {
        return Frame::Incomplete;
    };
    if crc32c::crc32c(payload) != crc {
        return Frame::Invalid;
    }
    Frame::Record { payload_start, next: payload_start + len }
}

/// The result of scanning a segment's records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scan {
    /// End of the last valid record: everything after it is torn or corrupt.
    pub valid_end: usize,
    /// `(payload_start, payload_end)` of every valid record, in order.
    pub records: Vec<(usize, usize)>,
}

/// Scans records from `start` until the first frame that is not a whole valid
/// record (sensor spec §8.2).
pub fn scan(data: &[u8], start: usize) -> Scan {
    let mut at = start.min(data.len());
    let mut records = Vec::new();
    while let Frame::Record { payload_start, next } = parse(data, at) {
        records.push((payload_start, next));
        at = next;
    }
    Scan { valid_end: at, records }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framed(payloads: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        for p in payloads {
            encode(p, &mut out);
        }
        out
    }

    #[test]
    fn encode_then_parse_round_trips() {
        let data = framed(&[b"abc", b"de"]);
        assert_eq!(parse(&data, 0), Frame::Record { payload_start: 8, next: 11 });
        assert_eq!(parse(&data, 11), Frame::Record { payload_start: 19, next: 21 });
        assert_eq!(parse(&data, 21), Frame::Incomplete);
        assert_eq!(scan(&data, 0), Scan { valid_end: 21, records: vec![(8, 11), (19, 21)] });
    }

    #[test]
    fn short_header_or_payload_is_incomplete() {
        let data = framed(&[b"abcdef"]);
        for cut in 0..data.len() {
            assert_eq!(parse(&data[..cut], 0), Frame::Incomplete, "cut at {cut}");
        }
    }

    #[test]
    fn bad_length_or_crc_is_invalid() {
        let mut zero_len = framed(&[b"x"]);
        zero_len[..4].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(parse(&zero_len, 0), Frame::Invalid);

        // Rejected from the header alone: the 4 GiB "payload" is never looked for.
        let mut huge = framed(&[b"x"]);
        huge[..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(parse(&huge, 0), Frame::Invalid);

        let mut over = vec![0u8; RECORD_HEADER];
        over[..4].copy_from_slice(&((MAX_PAYLOAD + 1) as u32).to_le_bytes());
        assert_eq!(parse(&over, 0), Frame::Invalid);

        let mut flipped = framed(&[b"abc"]);
        flipped[9] ^= 1;
        assert_eq!(parse(&flipped, 0), Frame::Invalid);
    }

    #[test]
    fn scan_stops_at_the_first_bad_frame() {
        let mut data = framed(&[b"one", b"two", b"three"]);
        data[20] ^= 0xff; // inside "two"'s payload (bytes 19..22)
        assert_eq!(scan(&data, 0), Scan { valid_end: 11, records: vec![(8, 11)] });
    }

    #[test]
    fn offsets_past_the_end_are_incomplete_not_a_panic() {
        assert_eq!(parse(&[], usize::MAX), Frame::Incomplete);
        assert_eq!(scan(&[1, 2, 3], 99), Scan { valid_end: 3, records: vec![] });
    }
}
