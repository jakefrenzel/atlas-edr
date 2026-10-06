//! The real-time consumer: `OpenTraceW` + `ProcessTrace` on a dedicated thread,
//! calling a Rust closure for every event (sensor spec §3.2 [1]).

use super::EtwError;
use crate::Provider;
use crate::parse::{EventMeta, PointerSize};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;
use windows::Win32::Foundation::GetLastError;
use windows::Win32::System::Diagnostics::Etw::*;
use windows::core::PWSTR;

/// One event, valid only inside the callback.
pub struct EventRecord<'a> {
    rec: &'a EVENT_RECORD,
}

impl<'a> EventRecord<'a> {
    /// Wraps a record built by a test (the gate's tests rebuild records offline).
    #[cfg(test)]
    pub(crate) fn from_raw(rec: &'a EVENT_RECORD) -> Self {
        EventRecord { rec }
    }

    pub(crate) fn raw(&self) -> *const EVENT_RECORD {
        self.rec
    }

    pub fn provider_guid(&self) -> u128 {
        self.rec.EventHeader.ProviderId.to_u128()
    }

    /// The provider, if it is one we parse.
    pub fn provider(&self) -> Option<Provider> {
        Provider::from_guid(self.provider_guid())
    }

    pub fn id(&self) -> u16 {
        self.rec.EventHeader.EventDescriptor.Id
    }

    pub fn version(&self) -> u8 {
        self.rec.EventHeader.EventDescriptor.Version
    }

    pub fn opcode(&self) -> u8 {
        self.rec.EventHeader.EventDescriptor.Opcode
    }

    /// `EVENT_HEADER.Flags`.
    pub fn flags(&self) -> u16 {
        self.rec.EventHeader.Flags
    }

    /// The logging process (for network events this is not the owner, §5.3).
    pub fn pid(&self) -> u32 {
        self.rec.EventHeader.ProcessId
    }

    pub fn tid(&self) -> u32 {
        self.rec.EventHeader.ThreadId
    }

    /// Raw QPC ticks: sessions use `ClientContext = 1` and the consumer asks
    /// for raw timestamps (§3.3).
    pub fn timestamp(&self) -> i64 {
        self.rec.EventHeader.TimeStamp
    }

    pub fn pointer_size(&self) -> PointerSize {
        if u32::from(self.flags()) & EVENT_HEADER_FLAG_32_BIT_HEADER != 0 { PointerSize::P32 } else { PointerSize::P64 }
    }

    pub fn payload(&self) -> &'a [u8] {
        if self.rec.UserData.is_null() || self.rec.UserDataLength == 0 {
            return &[];
        }
        // SAFETY: ETW guarantees UserData points to UserDataLength bytes for the
        // duration of the callback, which bounds 'a.
        unsafe { std::slice::from_raw_parts(self.rec.UserData.cast::<u8>(), usize::from(self.rec.UserDataLength)) }
    }

    /// The process start key from the extended data, when the provider was
    /// enabled with `EVENT_ENABLE_PROPERTY_PROCESS_START_KEY` (§5.2).
    pub fn start_key(&self) -> Option<u64> {
        let n = usize::from(self.rec.ExtendedDataCount);
        if n == 0 || self.rec.ExtendedData.is_null() {
            return None;
        }
        // SAFETY: ExtendedData points to ExtendedDataCount items during the callback.
        let items = unsafe { std::slice::from_raw_parts(self.rec.ExtendedData, n) };
        items.iter().find_map(|item| {
            (u32::from(item.ExtType) == EVENT_HEADER_EXT_TYPE_PROCESS_START_KEY
                && item.DataSize >= 8
                && item.DataPtr != 0)
                // SAFETY: the item's DataPtr holds DataSize (≥ 8) bytes; read unaligned.
                .then(|| unsafe { (item.DataPtr as *const u64).read_unaligned() })
        })
    }

    /// What the parsers need, if this is a provider we parse. Classic events
    /// are identified by opcode (their ID is 0).
    pub fn meta(&self) -> Option<EventMeta> {
        let provider = self.provider()?;
        let id = if provider == Provider::ClassicProcess { u16::from(self.opcode()) } else { self.id() };
        Some(EventMeta { provider, id, version: self.version(), pointer_size: self.pointer_size() })
    }
}

type Callback = Box<dyn FnMut(&EventRecord) + Send>;

/// A consumer thread. Close it after stopping its session; dropping it closes it too.
pub struct Consumer {
    handle: PROCESSTRACE_HANDLE,
    thread: Option<JoinHandle<u32>>,
    panics: Arc<AtomicU64>,
    qpc_frequency: i64,
    closed: bool,
}

/// `INVALID_PROCESSTRACE_HANDLE`.
const INVALID: u64 = u64::MAX;

struct Context {
    callback: Callback,
    panics: Arc<AtomicU64>,
}

unsafe extern "system" fn trampoline(rec: *mut EVENT_RECORD) {
    // A panic must not unwind into ETW: catch it and count it.
    // SAFETY: ETW passes a valid record whose UserContext is the `Context` we
    // registered, alive until ProcessTrace returns.
    unsafe {
        let rec = &*rec;
        let ctx = &mut *rec.UserContext.cast::<Context>();
        let cb = &mut ctx.callback;
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cb(&EventRecord { rec }))).is_err() {
            ctx.panics.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Opens a real-time consumer on the session `name` and runs `ProcessTrace` on
/// a new thread, calling `callback` for every event, the session's own header
/// event included (its `meta()` is `None`). The callback must stay fast (§3.2).
pub fn consume(name: &str, callback: impl FnMut(&EventRecord) + Send + 'static) -> Result<Consumer, EtwError> {
    let panics = Arc::new(AtomicU64::new(0));
    let ctx = Box::into_raw(Box::new(Context { callback: Box::new(callback), panics: panics.clone() }));
    let mut name_w: Vec<u16> = name.encode_utf16().chain([0]).collect();
    // SAFETY: an all-zero EVENT_TRACE_LOGFILEW is valid; the fields set below are
    // the ones a real-time EVENT_RECORD consumer needs.
    let mut lf: EVENT_TRACE_LOGFILEW = unsafe { std::mem::zeroed() };
    lf.LoggerName = PWSTR(name_w.as_mut_ptr());
    lf.Anonymous1.ProcessTraceMode =
        PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD | PROCESS_TRACE_MODE_RAW_TIMESTAMP;
    lf.Anonymous2.EventRecordCallback = Some(trampoline);
    lf.Context = ctx.cast();
    // SAFETY: `lf` is initialised as above; OpenTraceW copies what it keeps.
    let handle = unsafe { OpenTraceW(&mut lf) };
    if handle.Value == INVALID {
        // SAFETY: reclaims the context we leaked above; nothing else holds it.
        drop(unsafe { Box::from_raw(ctx) });
        // SAFETY: reads the calling thread's last error.
        return Err(EtwError { op: "OpenTraceW", code: unsafe { GetLastError() }.0 });
    }
    let qpc_frequency = lf.LogfileHeader.PerfFreq;
    let ctx_addr = ctx as usize;
    let thread = std::thread::Builder::new()
        .name(format!("etw-{name}"))
        .spawn(move || {
            // SAFETY: the handle is open; ProcessTrace calls the trampoline on this thread.
            let st = unsafe { ProcessTrace(&[handle], None, None) };
            // SAFETY: ProcessTrace has returned, so no callback can still use the context.
            drop(unsafe { Box::from_raw(ctx_addr as *mut Context) });
            st.0
        })
        .map_err(|_| {
            // SAFETY: the thread never started, so the handle and context are ours to release.
            unsafe {
                let _ = CloseTrace(handle);
                drop(Box::from_raw(ctx_addr as *mut Context));
            }
            EtwError { op: "spawn consumer thread", code: 0 }
        })?;
    Ok(Consumer { handle, thread: Some(thread), panics, qpc_frequency, closed: false })
}

impl Consumer {
    /// The QPC frequency from the session's log file header, for converting
    /// timestamps to wall time (§3.3).
    pub fn qpc_frequency(&self) -> i64 {
        self.qpc_frequency
    }

    /// Callbacks that panicked (each was caught and the event skipped).
    pub fn panics(&self) -> u64 {
        self.panics.load(Ordering::Relaxed)
    }

    /// `ProcessTrace` has returned: the session stopped or was destroyed (§9.1).
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Closes the trace and waits for the thread. Returns `ProcessTrace`'s status.
    /// Called after the session stopped, every event has been delivered first.
    pub fn close(mut self) -> u32 {
        self.close_inner()
    }

    fn close_inner(&mut self) -> u32 {
        if !self.closed {
            self.closed = true;
            // SAFETY: the handle came from OpenTraceW and is closed exactly once.
            let _ = unsafe { CloseTrace(self.handle) };
        }
        self.thread.take().map_or(0, |t| t.join().unwrap_or(u32::MAX))
    }
}

impl Drop for Consumer {
    fn drop(&mut self) {
        self.close_inner();
    }
}
