//! ETW sessions on Windows (sensor spec §4.1, §9.1): start, enable, query and
//! stop them, and consume their events in real time. All of the sensor's ETW
//! `unsafe` code lives in this module, behind a safe API. Starting a session
//! needs administrator rights; the TDH manifest queries do not.

mod consumer;
mod gate;
pub mod tdh;

pub use consumer::{Consumer, EventRecord, consume};
pub use gate::{Verdict, VersionGate, verdict};

use crate::Provider;
use crate::providers::{Enable, LEVEL};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, ERROR_WMI_INSTANCE_NOT_FOUND, WIN32_ERROR};
use windows::Win32::System::Diagnostics::Etw::*;
use windows::core::{GUID, PCWSTR};

/// Session A: the manifest providers (§4.1).
pub const SESSION_A: &str = "Atlas-Sensor";
/// Session B: the system logger, process events only (§4.1).
pub const SESSION_B: &str = "Atlas-Process";

/// A failed ETW call: which function, and the Win32 error it returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EtwError {
    pub op: &'static str,
    pub code: u32,
}

impl std::fmt::Display for EtwError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = windows::core::HRESULT::from_win32(self.code).message();
        write!(f, "{}: error {} ({})", self.op, self.code, msg.trim_end())
    }
}

impl std::error::Error for EtwError {}

fn check(op: &'static str, st: WIN32_ERROR) -> Result<(), EtwError> {
    if st == ERROR_SUCCESS { Ok(()) } else { Err(EtwError { op, code: st.0 }) }
}

/// `EVENT_TRACE_USE_MS_FLUSH_TIMER` (evntrace.h); missing from the `windows` crate.
const EVENT_TRACE_USE_MS_FLUSH_TIMER: u32 = 0x10;

/// Which kind of session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Real-time; providers are enabled with [`Session::enable`].
    Manifest,
    /// Real-time system logger with `EVENT_TRACE_FLAG_PROCESS` only (Session B).
    ProcessLogger,
}

/// Session settings (sensor spec §4.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub name: String,
    pub kind: Kind,
    pub buffer_kb: u32,
    pub min_buffers: u32,
    pub max_buffers: u32,
    pub flush_timer_ms: u32,
}

impl Config {
    /// Session A's settings for this machine: 64 KB buffers, min 2 × CPUs,
    /// max max(256, 4 × CPUs), 250 ms flush.
    pub fn session_a(name: &str) -> Config {
        let cpus = std::thread::available_parallelism().map_or(4, |n| n.get() as u32);
        Config {
            name: name.into(),
            kind: Kind::Manifest,
            buffer_kb: 64,
            min_buffers: 2 * cpus,
            max_buffers: (4 * cpus).max(256),
            flush_timer_ms: 250,
        }
    }

    /// Session B's settings: 64 KB buffers, min 4, max 16, 250 ms flush.
    pub fn session_b(name: &str) -> Config {
        Config {
            name: name.into(),
            kind: Kind::ProcessLogger,
            buffer_kb: 64,
            min_buffers: 4,
            max_buffers: 16,
            flush_timer_ms: 250,
        }
    }
}

/// A session's state, from `ControlTraceW` (query or stop).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionInfo {
    /// Identifies this instance of the session; a session recreated under the
    /// same name gets a new one (§9.1).
    pub logger_id: u16,
    pub events_lost: u32,
    pub realtime_buffers_lost: u32,
    pub buffers_written: u32,
    pub buffers: u32,
    pub free_buffers: u32,
    pub log_file_mode: u32,
}

/// `EVENT_TRACE_PROPERTIES` followed by room for the logger name, 8-byte aligned.
struct Props(Vec<u64>);

impl Props {
    fn new() -> Props {
        let mut p = Props(vec![0u64; (size_of::<EVENT_TRACE_PROPERTIES>() + 2 * 1024).div_ceil(8)]);
        let len = (p.0.len() * 8) as u32;
        let h = p.get_mut();
        h.Wnode.BufferSize = len;
        h.LoggerNameOffset = size_of::<EVENT_TRACE_PROPERTIES>() as u32;
        p
    }

    fn get_mut(&mut self) -> &mut EVENT_TRACE_PROPERTIES {
        // SAFETY: the buffer is larger than EVENT_TRACE_PROPERTIES and 8-byte aligned;
        // all-zero bytes are a valid value of the struct.
        unsafe { &mut *self.0.as_mut_ptr().cast::<EVENT_TRACE_PROPERTIES>() }
    }

    fn ptr(&mut self) -> *mut EVENT_TRACE_PROPERTIES {
        self.0.as_mut_ptr().cast()
    }

    fn info(&mut self) -> SessionInfo {
        let p = self.get_mut();
        SessionInfo {
            // SAFETY: after a query, HistoricalContext holds the session handle,
            // whose low 16 bits are the LoggerId.
            logger_id: unsafe { p.Wnode.Anonymous1.HistoricalContext } as u16,
            events_lost: p.EventsLost,
            realtime_buffers_lost: p.RealTimeBuffersLost,
            buffers_written: p.BuffersWritten,
            buffers: p.NumberOfBuffers,
            free_buffers: p.FreeBuffers,
            log_file_mode: p.LogFileMode,
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// Queries, flushes or stops a session by name. Returns `None` if no session
/// has that name.
fn control_by_name(name: &str, code: EVENT_TRACE_CONTROL, op: &'static str) -> Result<Option<SessionInfo>, EtwError> {
    let name = wide(name);
    let mut props = Props::new();
    // SAFETY: `props` is a properties buffer with room for the name; `name` is NUL-terminated.
    let st = unsafe { ControlTraceW(CONTROLTRACE_HANDLE { Value: 0 }, PCWSTR(name.as_ptr()), props.ptr(), code) };
    if st == ERROR_WMI_INSTANCE_NOT_FOUND {
        return Ok(None);
    }
    check(op, st)?;
    Ok(Some(props.info()))
}

/// The session's state, or `None` if no session has that name (§9.1).
pub fn query_by_name(name: &str) -> Result<Option<SessionInfo>, EtwError> {
    control_by_name(name, EVENT_TRACE_CONTROL_QUERY, "ControlTraceW(query)")
}

/// Stops a session by name (a leftover from a crash, or a blinding test).
/// Returns its final state, or `None` if it did not exist.
pub fn stop_by_name(name: &str) -> Result<Option<SessionInfo>, EtwError> {
    control_by_name(name, EVENT_TRACE_CONTROL_STOP, "ControlTraceW(stop)")
}

fn event_id_filter(ids: &[u16]) -> Vec<u8> {
    // EVENT_FILTER_EVENT_ID: FilterIn (BOOLEAN), Reserved (UCHAR), Count (USHORT), Events[Count].
    let mut f = vec![1u8, 0];
    f.extend((ids.len() as u16).to_le_bytes());
    for id in ids {
        f.extend(id.to_le_bytes());
    }
    f
}

/// Enables `guid` with the start-key property and, if `ids` is non-empty, an
/// event-ID allow-list.
fn enable_raw(handle: CONTROLTRACE_HANDLE, guid: u128, keywords: u64, ids: &[u16]) -> Result<(), EtwError> {
    let filter = event_id_filter(ids);
    let mut desc = EVENT_FILTER_DESCRIPTOR {
        Ptr: filter.as_ptr() as u64,
        Size: filter.len() as u32,
        Type: EVENT_FILTER_TYPE_EVENT_ID,
    };
    let params = ENABLE_TRACE_PARAMETERS {
        Version: ENABLE_TRACE_PARAMETERS_VERSION_2,
        EnableProperty: EVENT_ENABLE_PROPERTY_PROCESS_START_KEY,
        EnableFilterDesc: if ids.is_empty() { std::ptr::null_mut() } else { &mut desc },
        FilterDescCount: u32::from(!ids.is_empty()),
        ..Default::default()
    };
    let guid = GUID::from_u128(guid);
    // SAFETY: `params` and the filter it points to outlive the call.
    let st = unsafe {
        EnableTraceEx2(handle, &guid, EVENT_CONTROL_CODE_ENABLE_PROVIDER.0, LEVEL, keywords, 0, 0, Some(&params))
    };
    check("EnableTraceEx2(enable)", st)
}

fn disable_raw(handle: CONTROLTRACE_HANDLE, provider: Provider) -> Result<(), EtwError> {
    let guid = GUID::from_u128(provider.guid());
    // SAFETY: plain values; no parameters.
    let st = unsafe { EnableTraceEx2(handle, &guid, EVENT_CONTROL_CODE_DISABLE_PROVIDER.0, 0, 0, 0, 0, None) };
    check("EnableTraceEx2(disable)", st)
}

/// `ERROR_WMI_INSTANCE_NOT_FOUND`: the code a stale [`Session`] reports.
pub const STALE: u32 = 4201;

/// A running session. Dropping it stops the session: ETW sessions outlive the
/// process that started them.
///
/// The handle is the session's LoggerId, a small number Windows reuses. If the
/// session was stopped from outside (a crash recovery, a blinding attack, §9.1),
/// another session may hold that number by now. So every call first checks that
/// the session with our name still has our LoggerId, and otherwise does nothing
/// and returns [`STALE`]; dropping a stale session stops nothing. A replacement
/// is a new `Session`; the old one should be [`Session::abandon`]ed.
pub struct Session {
    name: String,
    handle: CONTROLTRACE_HANDLE,
    logger_id: u16,
    stopped: bool,
}

impl Session {
    /// Starts a session. A leftover session with the same name (from a crash)
    /// is stopped first (§4.1).
    pub fn start(cfg: &Config) -> Result<Session, EtwError> {
        stop_by_name(&cfg.name)?;
        let name = wide(&cfg.name);
        let mut props = Props::new();
        let p = props.get_mut();
        p.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
        p.Wnode.ClientContext = 1; // raw QPC timestamps (§3.3)
        p.Wnode.Guid = GUID::new().map_err(|e| EtwError { op: "CoCreateGuid", code: e.code().0 as u32 })?;
        p.BufferSize = cfg.buffer_kb;
        p.MinimumBuffers = cfg.min_buffers;
        p.MaximumBuffers = cfg.max_buffers;
        p.FlushTimer = cfg.flush_timer_ms;
        p.LogFileMode = EVENT_TRACE_REAL_TIME_MODE | EVENT_TRACE_USE_MS_FLUSH_TIMER;
        if cfg.kind == Kind::ProcessLogger {
            p.LogFileMode |= EVENT_TRACE_SYSTEM_LOGGER_MODE;
            p.EnableFlags = EVENT_TRACE_FLAG_PROCESS;
        }
        let mut handle = CONTROLTRACE_HANDLE::default();
        // SAFETY: `props` is a properties buffer with room for the name; `name` is NUL-terminated.
        check("StartTraceW", unsafe { StartTraceW(&mut handle, PCWSTR(name.as_ptr()), props.ptr()) })?;
        let logger_id = handle.Value as u16;
        Ok(Session { name: cfg.name.clone(), handle, logger_id, stopped: false })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The LoggerId this session got at creation (§9.1).
    pub fn logger_id(&self) -> u16 {
        self.logger_id
    }

    /// Enables a provider with its keywords, the event-ID allow-list and the
    /// process start key (§4.2). Enabling again replaces the previous settings.
    pub fn enable(&self, e: &Enable) -> Result<(), EtwError> {
        self.check_current("EnableTraceEx2(enable)")?;
        enable_raw(self.handle, e.provider.guid(), e.keywords, &e.event_ids)
    }

    /// Enables a provider outside the sensor's set, every event ID (diagnostics
    /// and tests).
    pub fn enable_guid(&self, guid: u128, keywords: u64) -> Result<(), EtwError> {
        self.check_current("EnableTraceEx2(enable)")?;
        enable_raw(self.handle, guid, keywords, &[])
    }

    pub fn disable(&self, provider: Provider) -> Result<(), EtwError> {
        self.check_current("EnableTraceEx2(disable)")?;
        disable_raw(self.handle, provider)
    }

    /// Whether the session with our name is still the one we started.
    pub fn is_current(&self) -> bool {
        query_by_name(&self.name).is_ok_and(|i| i.is_some_and(|i| i.logger_id == self.logger_id))
    }

    fn check_current(&self, op: &'static str) -> Result<(), EtwError> {
        if self.is_current() { Ok(()) } else { Err(EtwError { op, code: STALE }) }
    }

    /// Forgets a session that was stopped or replaced from outside, without
    /// touching whatever now holds its LoggerId.
    pub fn abandon(mut self) {
        self.stopped = true;
    }

    /// The session's current state and loss counters.
    pub fn query(&self) -> Result<SessionInfo, EtwError> {
        self.control(EVENT_TRACE_CONTROL_QUERY, "ControlTraceW(query)")
    }

    /// Delivers the buffers' contents now instead of at the next flush tick.
    pub fn flush(&self) -> Result<SessionInfo, EtwError> {
        self.control(EVENT_TRACE_CONTROL_FLUSH, "ControlTraceW(flush)")
    }

    /// Stops the session and returns its final counters. Consumers receive the
    /// remaining events, then `ProcessTrace` returns.
    pub fn stop(mut self) -> Result<SessionInfo, EtwError> {
        self.stopped = true;
        self.control(EVENT_TRACE_CONTROL_STOP, "ControlTraceW(stop)")
    }

    fn control(&self, code: EVENT_TRACE_CONTROL, op: &'static str) -> Result<SessionInfo, EtwError> {
        self.check_current(op)?;
        let mut props = Props::new();
        // SAFETY: `props` is a properties buffer; the handle came from StartTraceW.
        check(op, unsafe { ControlTraceW(self.handle, PCWSTR::null(), props.ptr(), code) })?;
        Ok(props.info())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if !self.stopped {
            let _ = self.control(EVENT_TRACE_CONTROL_STOP, "ControlTraceW(stop)");
        }
    }
}

/// How one session has a provider enabled, from `EnumerateTraceGuidsEx`.
/// The event-ID filter cannot be read back: Windows has no query for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderEnable {
    /// The registered provider instance (0 for kernel providers).
    pub pid: u32,
    pub logger_id: u16,
    pub level: u8,
    pub match_any_keyword: u64,
    pub match_all_keyword: u64,
    pub enable_property: u32,
}

/// Every session's enablement of `provider` (`TraceGuidQueryInfo`, §9.1).
/// Empty if the provider is not registered.
pub fn provider_state(provider: Provider) -> Result<Vec<ProviderEnable>, EtwError> {
    let guid = GUID::from_u128(provider.guid());
    let mut buf = vec![0u64; 64];
    loop {
        let mut needed = 0u32;
        // SAFETY: in = one GUID; out = `buf`, whose byte size is passed.
        let st = unsafe {
            EnumerateTraceGuidsEx(
                TraceGuidQueryInfo,
                Some((&raw const guid).cast()),
                size_of::<GUID>() as u32,
                Some(buf.as_mut_ptr().cast()),
                (buf.len() * 8) as u32,
                &mut needed,
            )
        };
        match st {
            s if s == ERROR_SUCCESS => {
                // SAFETY: viewing initialised u64s as bytes.
                let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), buf.len() * 8) };
                return Ok(parse_guid_info(&bytes[..(needed as usize).min(bytes.len())]));
            }
            s if s == ERROR_INSUFFICIENT_BUFFER => buf = vec![0u64; (needed as usize).div_ceil(8)],
            // ERROR_WMI_GUID_NOT_FOUND: no instance of the provider is registered.
            s if s.0 == 4200 => return Ok(Vec::new()),
            s => return Err(EtwError { op: "EnumerateTraceGuidsEx", code: s.0 }),
        }
    }
}

fn le_u16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}
fn le_u32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
fn le_u64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// Parses `TRACE_GUID_INFO` → `TRACE_PROVIDER_INSTANCE_INFO`s, each followed by
/// its `TRACE_ENABLE_INFO`s, with bounds checks instead of pointer casts.
fn parse_guid_info(b: &[u8]) -> Vec<ProviderEnable> {
    const GUID_INFO: usize = 8; // InstanceCount, Reserved
    const INSTANCE: usize = 16; // NextOffset, EnableCount, Pid, Flags
    const ENABLE: usize = 32; // IsEnabled, Level, Reserved1, LoggerId, EnableProperty, Reserved2, MatchAny, MatchAll
    let mut out = Vec::new();
    let Some(instances) = le_u32(b, 0) else { return out };
    let mut at = GUID_INFO;
    for _ in 0..instances {
        let (Some(next), Some(count), Some(pid)) = (le_u32(b, at), le_u32(b, at + 4), le_u32(b, at + 8)) else {
            break;
        };
        for i in 0..count as usize {
            let e = at + INSTANCE + i * ENABLE;
            let (Some(level), Some(logger_id), Some(prop), Some(any), Some(all)) =
                (b.get(e + 4).copied(), le_u16(b, e + 6), le_u32(b, e + 8), le_u64(b, e + 16), le_u64(b, e + 24))
            else {
                break;
            };
            out.push(ProviderEnable {
                pid,
                logger_id,
                level,
                match_any_keyword: any,
                match_all_keyword: all,
                enable_property: prop,
            });
        }
        if next == 0 {
            break;
        }
        at += next as usize;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_info_parses_instances_and_enables() {
        let mut b = Vec::new();
        b.extend(2u32.to_le_bytes()); // two instances
        b.extend(0u32.to_le_bytes());
        // Instance 1: pid 0, one enable, NextOffset = 16 + 32.
        b.extend(48u32.to_le_bytes());
        b.extend(1u32.to_le_bytes());
        b.extend(0u32.to_le_bytes());
        b.extend(0u32.to_le_bytes());
        b.extend(1u32.to_le_bytes()); // IsEnabled
        b.extend([5u8, 0]); // Level, Reserved1
        b.extend(42u16.to_le_bytes()); // LoggerId
        b.extend(0x40u32.to_le_bytes()); // EnableProperty
        b.extend(0u32.to_le_bytes());
        b.extend(0x1EE0u64.to_le_bytes());
        b.extend(0u64.to_le_bytes());
        // Instance 2: pid 1234, no enables, last.
        b.extend(0u32.to_le_bytes());
        b.extend(0u32.to_le_bytes());
        b.extend(1234u32.to_le_bytes());
        b.extend(0u32.to_le_bytes());
        assert_eq!(
            parse_guid_info(&b),
            vec![ProviderEnable {
                pid: 0,
                logger_id: 42,
                level: 5,
                match_any_keyword: 0x1EE0,
                match_all_keyword: 0,
                enable_property: 0x40
            }]
        );
        // Truncated input yields what was whole, never a panic.
        for cut in 0..b.len() {
            let _ = parse_guid_info(&b[..cut]);
        }
    }

    #[test]
    fn event_id_filter_layout() {
        assert_eq!(event_id_filter(&[12, 3008]), vec![1, 0, 2, 0, 12, 0, 0xC0, 0x0B]);
    }
}
