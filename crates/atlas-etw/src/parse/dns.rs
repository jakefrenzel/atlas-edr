//! DNS-Client 3008's `QueryResults` string (sensor spec §5.4, S3).
//!
//! Entries are separated by `;` (the last one is followed by one too).
//! Addresses are in text form, IPv4 or IPv6 (including IPv4-mapped IPv6 such
//! as `::ffff:192.0.2.1`). Other records appear as `type: N <data>`; CNAMEs
//! (`type: 5`) come before the addresses. A failed query has an empty string.

use std::net::IpAddr;

/// One entry of `QueryResults`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DnsAnswer {
    /// An address: an A record if IPv4, an AAAA record if IPv6.
    Address(IpAddr),
    /// A `type: N <data>` entry.
    Record { rtype: u16, data: String },
    /// An entry in neither form, kept as logged.
    Unrecognized(String),
}

/// The parsed entries, at most [`DnsAnswers::MAX`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DnsAnswers {
    pub answers: Vec<DnsAnswer>,
    /// More than [`DnsAnswers::MAX`] entries were present; the rest were dropped.
    pub truncated: bool,
}

impl DnsAnswers {
    /// 0a's limit on `answers[]` (`DNS_ANSWERS_MAX`).
    pub const MAX: usize = 64;
}

/// Parses `QueryResults`. Never fails: an entry it does not understand is kept
/// as [`DnsAnswer::Unrecognized`].
pub fn parse_query_results(units: &[u16]) -> DnsAnswers {
    let text = String::from_utf16_lossy(units);
    let mut out = DnsAnswers::default();
    for entry in text.split(';').map(str::trim).filter(|e| !e.is_empty()) {
        if out.answers.len() == DnsAnswers::MAX {
            out.truncated = true;
            break;
        }
        out.answers.push(parse_entry(entry));
    }
    out
}

fn parse_entry(entry: &str) -> DnsAnswer {
    if let Some(rest) = entry.strip_prefix("type:") {
        let rest = rest.trim_start();
        let (num, data) = rest.split_once(' ').unwrap_or((rest, ""));
        if let Ok(rtype) = num.parse::<u16>() {
            return DnsAnswer::Record { rtype, data: data.to_string() };
        }
    } else if let Ok(ip) = entry.parse::<IpAddr>() {
        return DnsAnswer::Address(ip);
    }
    DnsAnswer::Unrecognized(entry.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> DnsAnswers {
        parse_query_results(&s.encode_utf16().collect::<Vec<_>>())
    }

    #[test]
    fn addresses_and_cnames_as_logged_in_s3() {
        let a = parse("type: 5 www.example.com-c-3.edgekey.net;type: 5 e1.dscb.akamaiedge.net;23.62.177.155;");
        assert_eq!(
            a.answers,
            vec![
                DnsAnswer::Record { rtype: 5, data: "www.example.com-c-3.edgekey.net".into() },
                DnsAnswer::Record { rtype: 5, data: "e1.dscb.akamaiedge.net".into() },
                DnsAnswer::Address("23.62.177.155".parse().unwrap()),
            ]
        );
        assert!(!a.truncated);
    }

    #[test]
    fn ipv6_including_mapped_ipv4() {
        let a = parse("2606:4700::6810:179a;::ffff:172.66.157.237;");
        assert_eq!(
            a.answers,
            vec![
                DnsAnswer::Address("2606:4700::6810:179a".parse().unwrap()),
                DnsAnswer::Address("::ffff:172.66.157.237".parse().unwrap()),
            ]
        );
    }

    #[test]
    fn a_failed_query_has_no_answers() {
        assert_eq!(parse(""), DnsAnswers::default());
        assert_eq!(parse(";;"), DnsAnswers::default());
    }

    #[test]
    fn odd_entries_are_kept_not_dropped() {
        assert_eq!(
            parse("type: x y;not-an-address;type: 16 \"v=spf1 -all\";").answers,
            vec![
                DnsAnswer::Unrecognized("type: x y".into()),
                DnsAnswer::Unrecognized("not-an-address".into()),
                DnsAnswer::Record { rtype: 16, data: "\"v=spf1 -all\"".into() },
            ]
        );
    }

    #[test]
    fn at_most_64_entries() {
        let s: String = (0..70).map(|i| format!("10.0.0.{i};")).collect();
        let a = parse(&s);
        assert_eq!(a.answers.len(), 64);
        assert!(a.truncated);
        let exactly: String = (0..64).map(|i| format!("10.0.0.{i};")).collect();
        assert!(!parse(&exactly).truncated);
    }

    #[test]
    fn unpaired_surrogates_do_not_panic() {
        let a = parse_query_results(&[0xD800, u16::from(b';'), u16::from(b'1')]);
        assert_eq!(a.answers, vec![DnsAnswer::Unrecognized("\u{FFFD}".into()), DnsAnswer::Unrecognized("1".into())]);
    }
}
