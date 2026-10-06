//! TDH (Trace Data Helper), used only as an oracle (sensor spec §4.3): once
//! per new (provider, event, version) for the version check, and in tests.
//! Never per event in the agent.

use super::{EtwError, EventRecord};
use crate::Provider;
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS};
use windows::Win32::System::Diagnostics::Etw::*;
use windows::core::{GUID, PWSTR};

/// A field as TDH describes it: name and in-type.
pub type TdhField = (String, u16);

/// A `TRACE_EVENT_INFO` in an 8-byte-aligned buffer.
struct Info(Vec<u64>);

impl Info {
    /// Calls `f(buffer, size)` until the buffer is large enough.
    fn fetch(
        op: &'static str,
        mut f: impl FnMut(Option<*mut TRACE_EVENT_INFO>, &mut u32) -> u32,
    ) -> Result<Info, EtwError> {
        let mut size = 0u32;
        let st = f(None, &mut size);
        if st != ERROR_INSUFFICIENT_BUFFER.0 {
            return Err(EtwError { op, code: st });
        }
        loop {
            let mut buf = vec![0u64; (size as usize).div_ceil(8)];
            let st = f(Some(buf.as_mut_ptr().cast()), &mut size);
            match st {
                s if s == ERROR_SUCCESS.0 => return Ok(Info(buf)),
                s if s == ERROR_INSUFFICIENT_BUFFER.0 => continue,
                s => return Err(EtwError { op, code: s }),
            }
        }
    }

    fn header(&self) -> &TRACE_EVENT_INFO {
        // SAFETY: TDH filled the buffer with a TRACE_EVENT_INFO at its start, and
        // the buffer is 8-byte aligned (Vec<u64>).
        unsafe { &*self.0.as_ptr().cast::<TRACE_EVENT_INFO>() }
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: viewing initialised u64s as bytes.
        unsafe { std::slice::from_raw_parts(self.0.as_ptr().cast::<u8>(), self.0.len() * 8) }
    }

    fn props(&self) -> &[EVENT_PROPERTY_INFO] {
        let count = self.header().PropertyCount as usize;
        trailing_array(&self.0, std::mem::offset_of!(TRACE_EVENT_INFO, EventPropertyInfoArray), count)
    }

    /// The NUL-terminated UTF-16 string at byte offset `off` (0 = none).
    fn string_at(&self, off: u32) -> String {
        let b = self.bytes();
        let start = off as usize;
        if off == 0 || start >= b.len() {
            return String::new();
        }
        let units: Vec<u16> =
            b[start..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
        String::from_utf16_lossy(&units)
    }

    /// Top-level fields with their in-types. Struct fields are reported with
    /// in-type 0 (none of our events has one).
    fn fields(&self) -> Vec<TdhField> {
        let h = self.header();
        self.props()
            .iter()
            .take(h.TopLevelPropertyCount as usize)
            .map(|p| {
                let in_type = if p.Flags.0 & PropertyStruct.0 != 0 {
                    0
                } else {
                    // SAFETY: nonStructType is the active variant when PropertyStruct is clear.
                    unsafe { p.Anonymous1.nonStructType.InType }
                };
                (self.string_at(p.NameOffset), in_type)
            })
            .collect()
    }
}

/// The `count` entries of a variable-length array that starts `offset` bytes into
/// `buf` (a C struct's trailing `T[1]`), clamped to what the buffer holds. The
/// pointer is derived from the whole buffer, not from a reference to the struct's
/// one-element array, so the slice stays in bounds of its provenance.
fn trailing_array<T>(buf: &[u64], offset: usize, count: usize) -> &[T] {
    let bytes = buf.len() * 8;
    let fits = bytes.saturating_sub(offset) / size_of::<T>();
    debug_assert!(offset.is_multiple_of(align_of::<T>()) && align_of::<T>() <= 8);
    // SAFETY: `buf` is 8-byte aligned and `offset` is a field offset of a struct
    // that TDH wrote at its start, so the address is aligned for T; at most `fits`
    // entries lie inside `buf`, and TDH initialised them (all-zero is valid too).
    unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>().add(offset).cast::<T>(), count.min(fits)) }
}

/// The installed manifest's layout of (provider, event, version), or `None`
/// if the manifest has no such event. Needs no elevation.
pub fn manifest_layout(provider: Provider, id: u16, version: u8) -> Result<Option<Vec<TdhField>>, EtwError> {
    let guid = GUID::from_u128(provider.guid());
    // TdhGetManifestEventInformation wants the full descriptor, so find it first.
    let mut size = 0u32;
    // SAFETY: a size query with no buffer.
    let st = unsafe { TdhEnumerateManifestProviderEvents(&guid, None, &mut size) };
    if st != ERROR_INSUFFICIENT_BUFFER.0 {
        return Err(EtwError { op: "TdhEnumerateManifestProviderEvents", code: st });
    }
    let mut buf = vec![0u64; (size as usize).div_ceil(8)];
    let pe = buf.as_mut_ptr().cast::<PROVIDER_EVENT_INFO>();
    // SAFETY: the buffer holds `size` bytes, 8-byte aligned.
    let st = unsafe { TdhEnumerateManifestProviderEvents(&guid, Some(pe), &mut size) };
    if st != ERROR_SUCCESS.0 {
        return Err(EtwError { op: "TdhEnumerateManifestProviderEvents", code: st });
    }
    // SAFETY: TDH wrote a PROVIDER_EVENT_INFO at the start of the buffer.
    let count = unsafe { (*pe).NumberOfEvents } as usize;
    let descs: &[EVENT_DESCRIPTOR] =
        trailing_array(&buf, std::mem::offset_of!(PROVIDER_EVENT_INFO, EventDescriptorsArray), count);
    let Some(d) = descs.iter().find(|d| d.Id == id && d.Version == version) else { return Ok(None) };
    let info = Info::fetch("TdhGetManifestEventInformation", |b, s| {
        // SAFETY: `b` is None or a buffer of `*s` bytes.
        unsafe { TdhGetManifestEventInformation(&guid, d, b, s) }
    })?;
    Ok(Some(info.fields()))
}

/// TDH's layout of a classic (MOF) event of Session B's process class, from a
/// record built here: TDH needs only the class GUID, opcode, version and header
/// flags to find the MOF description, so this needs no session and no elevation.
pub fn classic_layout(opcode: u8, version: u8) -> Result<Vec<TdhField>, EtwError> {
    // SAFETY: an all-zero EVENT_RECORD is valid; TDH reads only the header fields set below.
    let mut rec: EVENT_RECORD = unsafe { std::mem::zeroed() };
    rec.EventHeader.ProviderId = GUID::from_u128(Provider::ClassicProcess.guid());
    rec.EventHeader.EventDescriptor.Opcode = opcode;
    rec.EventHeader.EventDescriptor.Version = version;
    rec.EventHeader.Flags = (EVENT_HEADER_FLAG_CLASSIC_HEADER | EVENT_HEADER_FLAG_64_BIT_HEADER) as u16;
    let info = Info::fetch("TdhGetEventInformation", |b, s| {
        // SAFETY: `rec` lives for the call; `b` is None or a buffer of `*s` bytes.
        unsafe { TdhGetEventInformation(&rec, None, b, s) }
    })?;
    Ok(info.fields())
}

fn event_info(rec: &EventRecord) -> Result<Info, EtwError> {
    Info::fetch("TdhGetEventInformation", |b, s| {
        // SAFETY: the record is valid for the duration of the callback that holds it.
        unsafe { TdhGetEventInformation(rec.raw(), None, b, s) }
    })
}

/// TDH's layout of a live event (manifest or classic MOF).
pub fn event_layout(rec: &EventRecord) -> Result<Vec<TdhField>, EtwError> {
    event_info(rec).map(|i| i.fields())
}

/// Decodes a live event into (field name, TDH's text) pairs, in payload order.
/// For tests and fixture recording only: it costs microseconds per event.
pub fn decode(rec: &EventRecord) -> Result<Vec<(String, String)>, EtwError> {
    let info = event_info(rec)?;
    let data = rec.payload();
    let ptr_size: u32 = match rec.pointer_size() {
        crate::parse::PointerSize::P32 => 4,
        crate::parse::PointerSize::P64 => 8,
    };
    let props = info.props();
    let mut ints: Vec<Option<u64>> = vec![None; props.len()];
    let mut offset = 0usize;
    let mut out = Vec::new();
    for (i, p) in props.iter().enumerate().take(info.header().TopLevelPropertyCount as usize) {
        let name = info.string_at(p.NameOffset);
        let flags = p.Flags.0;
        if flags & (PropertyStruct.0 | PropertyParamCount.0 | PropertyParamFixedCount.0) != 0 {
            return Err(EtwError { op: "decode: struct or array field", code: 0 });
        }
        // SAFETY: the non-struct variants are active (checked above).
        let (in_type, out_type, length) = unsafe {
            let t = p.Anonymous1.nonStructType;
            let len = if flags & PropertyParamLength.0 != 0 {
                ints.get(p.Anonymous3.lengthPropertyIndex as usize).copied().flatten().unwrap_or(0) as u16
            } else {
                p.Anonymous3.length
            };
            (t.InType, t.OutType, len)
        };
        if flags & PropertyParamLength.0 != 0 && length == 0 {
            out.push((name, String::new()));
            continue;
        }
        let rest = &data[offset.min(data.len())..];
        ints[i] = read_int(rest, in_type, ptr_size);
        let mut text = vec![0u16; 256];
        loop {
            let mut size_bytes = (text.len() * 2) as u32;
            let mut consumed = 0u16;
            // SAFETY: `info` is the TRACE_EVENT_INFO for this event; `rest` and `text`
            // are valid buffers of the sizes passed.
            let st = unsafe {
                TdhFormatProperty(
                    info.0.as_ptr().cast(),
                    None,
                    ptr_size,
                    in_type,
                    out_type,
                    length,
                    &rest[..rest.len().min(u16::MAX as usize)],
                    &mut size_bytes,
                    Some(PWSTR(text.as_mut_ptr())),
                    &mut consumed,
                )
            };
            if st == ERROR_INSUFFICIENT_BUFFER.0 {
                text = vec![0u16; (size_bytes as usize).div_ceil(2)];
                continue;
            }
            if st != ERROR_SUCCESS.0 {
                return Err(EtwError { op: "TdhFormatProperty", code: st });
            }
            offset += usize::from(consumed);
            let n = text.iter().position(|&c| c == 0).unwrap_or(text.len());
            out.push((name, String::from_utf16_lossy(&text[..n])));
            break;
        }
    }
    Ok(out)
}

/// The integer at the start of `b`, for fields that size later ones.
fn read_int(b: &[u8], in_type: u16, ptr_size: u32) -> Option<u64> {
    let n = match i32::from(in_type) {
        x if x == TDH_INTYPE_INT8.0 || x == TDH_INTYPE_UINT8.0 => 1,
        x if x == TDH_INTYPE_INT16.0 || x == TDH_INTYPE_UINT16.0 => 2,
        x if x == TDH_INTYPE_INT32.0 || x == TDH_INTYPE_UINT32.0 || x == TDH_INTYPE_HEXINT32.0 => 4,
        x if x == TDH_INTYPE_INT64.0 || x == TDH_INTYPE_UINT64.0 || x == TDH_INTYPE_HEXINT64.0 => 8,
        x if x == TDH_INTYPE_POINTER.0 || x == TDH_INTYPE_SIZET.0 => ptr_size as usize,
        _ => return None,
    };
    let mut v = [0u8; 8];
    v[..n].copy_from_slice(b.get(..n)?);
    Some(u64::from_le_bytes(v))
}
