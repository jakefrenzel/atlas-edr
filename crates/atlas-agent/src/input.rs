//! What the ETW callbacks hand to the pipeline: a parsed event and the parts of
//! its header the pipeline needs (sensor spec §3.2 [1]).

use atlas_etw::parse::RawEvent;

/// Which session delivered the event (§4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Session {
    /// Session A: the manifest providers.
    Sensor,
    /// Session B: the system logger's classic process events.
    Process,
}

/// The event header, as far as the pipeline uses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub session: Session,
    /// The logging process. For network events this is not the owner (§5.3).
    pub pid: u32,
    pub tid: u32,
    /// Raw QPC (§3.3).
    pub ts: i64,
    /// From the extended data, when the provider was enabled with it (Session A).
    pub start_key: Option<u64>,
}

/// One event on its way from a callback to the pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incoming {
    pub header: Header,
    pub event: RawEvent,
}
