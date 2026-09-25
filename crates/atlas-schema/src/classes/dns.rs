//! DNS Activity (OCSF 4003), spec section 5.8.

use atlas_proto::v1 as wire;
use atlas_proto::v1::dns_activity::Activity as W;

use crate::convert::{Result, bounded, err, require, u16_field};
use crate::error::SchemaErrorKind;
use crate::limits::{DNS_ANSWER_DATA_MAX, DNS_ANSWERS_MAX, DNS_HOSTNAME_MAX};
use crate::objects::ProcessRef;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsActivity {
    pub actor: ProcessRef,
    /// `query.hostname`.
    pub hostname: String,
    /// `query.type`: numeric RR type (28 = AAAA).
    pub query_type: u16,
    pub action: DnsAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DnsAction {
    Response {
        /// DNS response code, when the sensor could map the platform status.
        rcode: Option<u16>,
        /// Raw platform status (Windows: Win32/DNS status from event 3008).
        platform_status: Option<u32>,
        answers: Vec<DnsAnswer>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsAnswer {
    /// Numeric RR type.
    pub rr_type: u16,
    pub data: String,
}

impl From<DnsActivity> for wire::DnsActivity {
    fn from(v: DnsActivity) -> Self {
        let activity = match v.action {
            DnsAction::Response { rcode, platform_status, answers } => W::Response(wire::DnsResponse {
                rcode: rcode.map(u32::from),
                platform_status,
                answers: answers
                    .into_iter()
                    .map(|a| wire::DnsAnswer { r#type: u32::from(a.rr_type), data: a.data })
                    .collect(),
            }),
        };
        Self {
            actor: Some(v.actor.into()),
            hostname: v.hostname,
            query_type: u32::from(v.query_type),
            activity: Some(activity),
        }
    }
}

impl DnsActivity {
    pub(crate) fn from_wire(w: wire::DnsActivity) -> Result<Self> {
        Ok(Self {
            actor: ProcessRef::required(w.actor, "", "actor.process")?,
            hostname: bounded(w.hostname, DNS_HOSTNAME_MAX, "", "query.hostname")?,
            query_type: u16_field(w.query_type, "", "query.type")?,
            action: match require(w.activity, "", "activity")? {
                W::Response(r) => {
                    if r.answers.len() > DNS_ANSWERS_MAX {
                        return err("", "answers", SchemaErrorKind::TooLarge);
                    }
                    let rcode = match r.rcode {
                        Some(c) => Some(u16_field(c, "", "rcode")?),
                        None => None,
                    };
                    let answers = r
                        .answers
                        .into_iter()
                        .enumerate()
                        .map(|(i, a)| {
                            let path = format!("answers[{i}]");
                            Ok(DnsAnswer {
                                rr_type: u16_field(a.r#type, &path, "type")?,
                                data: bounded(a.data, DNS_ANSWER_DATA_MAX, &path, "data")?,
                            })
                        })
                        .collect::<Result<Vec<_>>>()?;
                    DnsAction::Response { rcode, platform_status: r.platform_status, answers }
                }
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::test_support::proc_ref;

    fn response(answers: Vec<DnsAnswer>) -> DnsActivity {
        DnsActivity {
            actor: proc_ref(),
            hostname: "example.com".into(),
            query_type: 1,
            action: DnsAction::Response { rcode: Some(3), platform_status: Some(9003), answers },
        }
    }

    fn answer(data: &str) -> DnsAnswer {
        DnsAnswer { rr_type: 1, data: data.into() }
    }

    #[test]
    fn response_round_trips() {
        let a = response(vec![answer("93.184.216.34"), answer("93.184.216.35")]);
        assert_eq!(DnsActivity::from_wire(a.clone().into()).unwrap(), a);
    }

    #[test]
    fn too_many_answers_is_too_large() {
        let e = DnsActivity::from_wire(response(vec![answer("x"); DNS_ANSWERS_MAX + 1]).into()).unwrap_err();
        assert_eq!((e.field_path.as_str(), e.kind), ("answers", SchemaErrorKind::TooLarge));
    }

    #[test]
    fn answer_errors_carry_their_index() {
        let long = "a".repeat(DNS_ANSWER_DATA_MAX + 1);
        let e = DnsActivity::from_wire(response(vec![answer("ok"), answer(&long)]).into()).unwrap_err();
        assert_eq!((e.field_path.as_str(), e.kind), ("answers[1].data", SchemaErrorKind::TooLarge));
    }

    #[test]
    fn query_type_must_fit_u16() {
        let w = wire::DnsActivity { query_type: 65_536, ..response(vec![]).into() };
        let e = DnsActivity::from_wire(w).unwrap_err();
        assert_eq!((e.field_path.as_str(), e.kind), ("query.type", SchemaErrorKind::Malformed));
    }
}
