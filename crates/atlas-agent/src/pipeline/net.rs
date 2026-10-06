//! Network and DNS (sensor spec §5.4, §7.3).
//!
//! Which end is which (plan 1b-2, F5): `saddr` is the local end for TCP connect,
//! TCP accept and UDP send; for UDP receive it is the remote sender.
//! OCSF `src_endpoint` is the initiator: the local end for outbound, the remote
//! end for inbound.

use std::collections::HashMap;
use std::net::IpAddr;

use atlas_etw::parse::{DnsAnswer as RawAnswer, DnsQuery, NetEvent, parse_query_results};
use atlas_schema::classes::dns::{DnsAction, DnsActivity, DnsAnswer};
use atlas_schema::classes::network::{NetworkAction, NetworkActivity, NetworkDirection, NetworkProtocol};
use atlas_schema::limits::{DNS_ANSWER_DATA_MAX, DNS_HOSTNAME_MAX, truncate_utf8};
use atlas_schema::{Event, EventKind, NetworkEndpoint, ProcessRef, ProcessUid};

use super::{IdGen, Pipeline, make_event};
use crate::completion::Completion;
use crate::counters::{Class, Counters};
use crate::input::Header;
use crate::process::Identity;
use crate::services::Lookups;
use crate::time::Clock;

pub(super) enum Tcp {
    Connect,
    Accept,
    Disconnect,
}

type Ep = (IpAddr, u16);

#[derive(Debug, Clone)]
struct Flow {
    actor: ProcessRef,
    local: Ep,
    remote: Ep,
    direction: NetworkDirection,
    last: i64,
}

pub struct Network {
    /// TCP connections opened while we watched: their direction, for the Close.
    tcp: HashMap<(u32, Ep, Ep), NetworkDirection>,
    /// UDP flows (§7.3), keyed by (actor, local, remote).
    flows: HashMap<(ProcessUid, Ep, Ep), Flow>,
    cap: usize,
    idle: i64,
    evictions: u64,
}

fn ep((ip, port): Ep) -> NetworkEndpoint {
    NetworkEndpoint { ip, port }
}

fn activity(
    actor: ProcessRef,
    local: Ep,
    remote: Ep,
    protocol: NetworkProtocol,
    direction: NetworkDirection,
    action: NetworkAction,
) -> EventKind {
    let (src, dst) = match direction {
        NetworkDirection::Outbound => (local, remote),
        NetworkDirection::Inbound => (remote, local),
    };
    EventKind::Network(NetworkActivity {
        actor,
        src_endpoint: ep(src),
        dst_endpoint: ep(dst),
        protocol,
        direction,
        action,
    })
}

const CLOSE: NetworkAction = NetworkAction::Close { bytes_in: None, bytes_out: None };

impl Network {
    pub fn new(cap: usize, idle_ticks: i64) -> Self {
        Network { tcp: HashMap::new(), flows: HashMap::new(), cap, idle: idle_ticks, evictions: 0 }
    }

    pub fn evictions(&self) -> u64 {
        self.evictions
    }

    fn close_flow(f: Flow, out: &mut Completion<Event>, clock: &Clock, ids: &mut IdGen, id: &Identity) {
        // A UDP Close is timestamped at the flow's last datagram (§3.3).
        let kind = activity(f.actor, f.local, f.remote, NetworkProtocol::Udp, f.direction, CLOSE);
        out.push(make_event(clock, ids, id, f.last, kind));
    }

    /// Closes UDP flows idle for longer than the idle timeout at stream time `now`.
    pub(super) fn expire(
        &mut self,
        now: i64,
        out: &mut Completion<Event>,
        _c: &mut Counters,
        clock: &Clock,
        ids: &mut IdGen,
        id: &Identity,
    ) {
        let idle = self.idle;
        let mut gone: Vec<Flow> = Vec::new();
        self.flows.retain(|_, f| {
            let keep = f.last.saturating_add(idle) >= now;
            if !keep {
                gone.push(f.clone());
            }
            keep
        });
        gone.sort_by_key(|f| f.last);
        for f in gone {
            Self::close_flow(f, out, clock, ids, id);
        }
    }

    /// A clean stop closes every open flow.
    pub(super) fn close_all(
        &mut self,
        out: &mut Completion<Event>,
        c: &mut Counters,
        clock: &Clock,
        ids: &mut IdGen,
        id: &Identity,
    ) {
        self.expire(i64::MAX, out, c, clock, ids, id);
    }
}

impl<L: Lookups> Pipeline<L> {
    pub(super) fn on_tcp(&mut self, h: &Header, n: NetEvent, kind: Tcp) {
        let Some(actor) = self.actor_payload(n.pid, h.ts, Class::Network) else { return };
        let (local, remote) = ((n.saddr, n.sport), (n.daddr, n.dport));
        let key = (n.pid, local, remote);
        let (direction, action) = match kind {
            Tcp::Connect => (NetworkDirection::Outbound, NetworkAction::Open),
            Tcp::Accept => (NetworkDirection::Inbound, NetworkAction::Open),
            Tcp::Disconnect => {
                // A connection opened before we watched has no known direction:
                // the end with the lower port is taken to be the server.
                let d = self.net.tcp.remove(&key).unwrap_or(if local.1 < remote.1 {
                    NetworkDirection::Inbound
                } else {
                    NetworkDirection::Outbound
                });
                (d, CLOSE)
            }
        };
        if matches!(action, NetworkAction::Open) {
            if self.net.tcp.len() >= self.net.cap {
                self.net.tcp.clear();
                self.net.evictions += 1;
            }
            self.net.tcp.insert(key, direction);
        }
        let ev = self.event(h.ts, activity(actor, local, remote, NetworkProtocol::Tcp, direction, action));
        self.completion.push(ev);
    }

    pub(super) fn on_udp(&mut self, h: &Header, n: NetEvent, send: bool) {
        if !self.cfg.network_udp {
            return;
        }
        let Some(actor) = self.actor_payload(n.pid, h.ts, Class::Network) else { return };
        let (local, remote) =
            if send { ((n.saddr, n.sport), (n.daddr, n.dport)) } else { ((n.daddr, n.dport), (n.saddr, n.sport)) };
        let key = (actor.uid, local, remote);
        if let Some(f) = self.net.flows.get_mut(&key) {
            f.last = f.last.max(h.ts);
            return;
        }
        let direction = if send { NetworkDirection::Outbound } else { NetworkDirection::Inbound };
        let ev = self
            .event(h.ts, activity(actor.clone(), local, remote, NetworkProtocol::Udp, direction, NetworkAction::Open));
        self.completion.push(ev);
        if self.net.flows.len() >= self.net.cap {
            // The table is full: the least recently active flows are closed (§7.3).
            let ages = self.net.flows.iter().map(|(k, f)| (*k, f.last));
            let mut victims: Vec<Flow> = crate::evict::oldest(ages, self.net.flows.len())
                .into_iter()
                .filter_map(|k| self.net.flows.remove(&k))
                .collect();
            victims.sort_by_key(|f| f.last);
            for f in victims {
                self.net.evictions += 1;
                Network::close_flow(f, &mut self.completion, &self.clock, &mut self.ids, &self.id);
            }
        }
        self.net.flows.insert(key, Flow { actor, local, remote, direction, last: h.ts });
    }

    pub(super) fn on_dns(&mut self, h: &Header, q: DnsQuery) {
        let Some(actor) = self.actor_sync(h, Class::Dns) else { return };
        let name = q.query_name.to_string_lossy();
        let hostname = truncate_utf8(&name, DNS_HOSTNAME_MAX).0.to_string();
        let parsed = parse_query_results(q.query_results.as_units());
        let answers = parsed
            .answers
            .into_iter()
            .map(|a| {
                let (rr_type, data) = match a {
                    RawAnswer::Address(ip @ IpAddr::V4(_)) => (1, ip.to_string()),
                    RawAnswer::Address(ip @ IpAddr::V6(_)) => (28, ip.to_string()),
                    RawAnswer::Record { rtype, data } => (rtype, data),
                    // Neither form: kept, with type 0 (plan 1b-3a clarification 9).
                    RawAnswer::Unrecognized(s) => (0, s),
                };
                DnsAnswer { rr_type, data: truncate_utf8(&data, DNS_ANSWER_DATA_MAX).0.to_string() }
            })
            .collect();
        let action =
            DnsAction::Response { rcode: rcode(q.query_status), platform_status: Some(q.query_status), answers };
        let query_type = u16::try_from(q.query_type).unwrap_or(u16::MAX);
        let ev = self.event(h.ts, EventKind::Dns(DnsActivity { actor, hostname, query_type, action }));
        self.completion.push(ev);
    }
}

/// DNS response code from the DNS-Client status (§5.4).
pub(crate) fn rcode(status: u32) -> Option<u16> {
    Some(match status {
        0 | 9501 => 0,
        9001 => 1,
        9002 => 2,
        9003 => 3,
        9004 => 4,
        9005 => 5,
        _ => return None,
    })
}
