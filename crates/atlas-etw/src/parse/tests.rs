use super::*;
use crate::layout::{InType, LAYOUTS};
use proptest::prelude::*;

/// Builds payloads field by field, little-endian, as ETW logs them.
#[derive(Default)]
struct B(Vec<u8>);

impl B {
    fn u16(mut self, v: u16) -> Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    fn u16_be(mut self, v: u16) -> Self {
        self.0.extend(v.to_be_bytes());
        self
    }
    fn u32(mut self, v: u32) -> Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    fn u64(mut self, v: u64) -> Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    fn ptr(self, p: PointerSize, v: u64) -> Self {
        match p {
            PointerSize::P32 => self.u32(v as u32),
            PointerSize::P64 => self.u64(v),
        }
    }
    fn raw(mut self, b: &[u8]) -> Self {
        self.0.extend(b);
        self
    }
    fn wstr(mut self, s: &str) -> Self {
        for u in s.encode_utf16().chain([0]) {
            self.0.extend(u.to_le_bytes());
        }
        self
    }
    fn astr(mut self, s: &[u8]) -> Self {
        self.0.extend(s);
        self.0.push(0);
        self
    }
    /// S-1-<authority>-<subs...>
    fn sid(mut self, authority: u8, subs: &[u32]) -> Self {
        self.0.extend([1, subs.len() as u8, 0, 0, 0, 0, 0, authority]);
        for s in subs {
            self.0.extend(s.to_le_bytes());
        }
        self
    }
}

fn meta(provider: Provider, id: u16, version: u8, pointer_size: PointerSize) -> EventMeta {
    EventMeta { provider, id, version, pointer_size }
}

/// A valid payload for any layout, driven by the layout table alone.
fn payload_for(fields: &[layout::Field], p: PointerSize) -> Vec<u8> {
    let mut b = B::default();
    let mut last_u16 = None;
    for (name, ty) in fields {
        b = match ty {
            InType::UInt16 => {
                // A size field (CapturedDataSize) is followed by that many bytes.
                let v = if name.ends_with("Size") { 3 } else { 0x1234 };
                last_u16 = Some(v);
                b.u16(v)
            }
            InType::UInt32 | InType::Int32 | InType::HexInt32 => b.u32(0x0102_0304),
            InType::UInt64 | InType::HexInt64 | InType::FileTime => b.u64(0x0102_0304_0506_0708),
            InType::Pointer => b.ptr(p, 0xffff_8000_1234_5678),
            InType::UnicodeString => b.wstr("ab"),
            InType::AnsiString => b.astr(b"ab"),
            InType::Sid => b.sid(16, &[8192]),
            InType::WbemSid => b.ptr(p, 0xffff_8000_0000_0001).ptr(p, 0).sid(5, &[18]),
            InType::Binary => match last_u16.take() {
                Some(n) => b.raw(&vec![0xAB; usize::from(n)]),
                None => b.raw(&[0xFE; 16]), // an IPv6 address
            },
        };
    }
    b.0
}

#[test]
fn every_layout_parses_and_every_strict_prefix_fails() {
    for l in LAYOUTS {
        for p in [PointerSize::P32, PointerSize::P64] {
            let m = meta(l.provider, l.id, l.version, p);
            let full = payload_for(l.fields, p);
            assert!(parse(&m, &full).is_ok(), "{l:?} {p:?}: {:?}", parse(&m, &full));
            for cut in 0..full.len() {
                assert!(parse(&m, &full[..cut]).is_err(), "{l:?} {p:?} parsed with {cut} of {} bytes", full.len());
            }
            // Trailing bytes from a longer, newer layout are ignored.
            let mut longer = full.clone();
            longer.extend([9; 7]);
            assert_eq!(parse(&m, &longer), parse(&m, &full), "{l:?}");
        }
    }
}

#[test]
fn process_start_v4_and_v3() {
    let body = |b: B| {
        b.u32(4242)
            .u64(17)
            .u64(133_000_000_000_000_000)
            .u32(1000)
            .u64(9)
            .u32(1)
            .u32(0)
            .u32(3)
            .u32(1)
            .sid(16, &[12288])
            .wstr(r"\Device\HarddiskVolume3\Windows\System32\cmd.exe")
            .u32(0xAABB)
            .u32(0x5F00_0000)
            .wstr("")
            .wstr("")
    };
    let v4 = body(B::default()).u32(0x10).0;
    let RawEvent::ProcessStart(s) = parse(&meta(Provider::KernelProcess, 1, 4, PointerSize::P64), &v4).unwrap() else {
        panic!()
    };
    assert_eq!((s.pid, s.sequence_number, s.parent_pid, s.parent_sequence_number), (4242, 17, 1000, 9));
    assert_eq!(s.create_time, 133_000_000_000_000_000);
    assert_eq!(s.mandatory_label.to_string(), "S-1-16-12288");
    assert_eq!(s.image_name.to_string_lossy(), r"\Device\HarddiskVolume3\Windows\System32\cmd.exe");
    assert_eq!(s.security_mitigations, Some(0x10));

    let v3 = body(B::default()).0;
    let RawEvent::ProcessStart(s) = parse(&meta(Provider::KernelProcess, 1, 3, PointerSize::P64), &v3).unwrap() else {
        panic!()
    };
    assert_eq!(s.security_mitigations, None);
    assert_eq!(s.time_date_stamp, 0x5F00_0000);
}

#[test]
fn process_stop_skips_the_counters() {
    let mut b = B::default().u32(77).u64(5).u64(1).u64(2).u32(0xC000_0005);
    for _ in 0..2 {
        b = b.u32(0xEE);
    }
    for _ in 0..3 {
        b = b.u64(0xEE);
    }
    for _ in 0..5 {
        b = b.u32(0xEE);
    }
    let b = b.astr(b"notepad.exe");
    let RawEvent::ProcessStop(s) = parse(&meta(Provider::KernelProcess, 2, 2, PointerSize::P64), &b.0).unwrap() else {
        panic!()
    };
    assert_eq!((s.pid, s.sequence_number, s.create_time, s.exit_time), (77, 5, 1, 2));
    assert_eq!(s.exit_code, 0xC000_0005);
    assert_eq!(&*s.image_name, b"notepad.exe");
}

#[test]
fn image_load_with_32_bit_pointers() {
    let p = PointerSize::P32;
    let b = B::default().ptr(p, 0x7700_0000).ptr(p, 0x1000).u32(99).u32(1).u32(2).ptr(p, 0x7700_0000).wstr(r"\x.dll");
    let RawEvent::ImageLoad(i) = parse(&meta(Provider::KernelProcess, 5, 0, p), &b.0).unwrap() else { panic!() };
    assert_eq!((i.image_base, i.image_size, i.pid, i.default_base), (0x7700_0000, 0x1000, 99, 0x7700_0000));
    assert_eq!(i.image_name.to_string_lossy(), r"\x.dll");
}

#[test]
fn file_create_and_delete_on_close() {
    let p = PointerSize::P64;
    let b = B::default().ptr(p, 1).ptr(p, 0xffff_a001).u32(12).u32(0x0100_1000).u32(0x80).u32(7).wstr(r"\a\b.txt");
    for (id, wrap) in [(12, RawEvent::FileCreate as fn(_) -> _), (30, RawEvent::FileCreateNew)] {
        let e = parse(&meta(Provider::KernelFile, id, 1, p), &b.0).unwrap();
        let c = FileCreate {
            irp: 1,
            file_object: 0xffff_a001,
            issuing_tid: 12,
            create_options: 0x0100_1000,
            create_attributes: 0x80,
            share_access: 7,
            file_name: r"\a\b.txt".into(),
        };
        assert!(c.delete_on_close());
        assert_eq!(e, wrap(c));
    }
}

#[test]
fn operation_end_failure_means_error_severity_only() {
    let op = |status| FileOpEnd { irp: 0, extra_information: 0, status };
    assert!(op(0xC000_0035).failed()); // STATUS_OBJECT_NAME_COLLISION
    assert!(op(0xC000_0121).failed()); // STATUS_CANNOT_DELETE
    assert!(!op(0).failed());
    assert!(!op(0x104).failed()); // STATUS_REPARSE
    assert!(!op(0x108).failed()); // STATUS_OPLOCK_BREAK_IN_PROGRESS
    assert!(!op(0x8000_0005).failed()); // STATUS_BUFFER_OVERFLOW: a warning
}

#[test]
fn rename_path_carries_the_new_name() {
    let p = PointerSize::P64;
    let b = B::default().ptr(p, 1).ptr(p, 2).ptr(p, 3).ptr(p, 4).u32(5).u32(65).wstr(r"\new.txt");
    let RawEvent::FileRenamePath(r) = parse(&meta(Provider::KernelFile, 27, 1, p), &b.0).unwrap() else { panic!() };
    assert_eq!((r.irp, r.file_object, r.file_key, r.extra_information, r.issuing_tid), (1, 2, 3, 4, 5));
    assert_eq!((r.info_class, r.file_path.to_string_lossy()), (65, r"\new.txt".to_string()));
}

#[test]
fn set_value_reads_both_captured_buffers() {
    let p = PointerSize::P64;
    let b = B::default()
        .ptr(p, 0xffff_c001)
        .u32(0)
        .u32(4)
        .u32(4)
        .wstr("")
        .wstr("Run")
        .u16(4)
        .raw(&[1, 0, 0, 0])
        .u32(1)
        .u32(6)
        .u16(2)
        .raw(&[b'h', 0]);
    let RawEvent::RegSetValue(v) = parse(&meta(Provider::KernelRegistry, 5, 0, p), &b.0).unwrap() else { panic!() };
    assert_eq!((v.key_object, v.status, v.value_type, v.data_size), (0xffff_c001, 0, 4, 4));
    assert_eq!(v.value_name.to_string_lossy(), "Run");
    assert_eq!((&*v.captured_data, v.previous_data_type, v.previous_data_size), (&[1u8, 0, 0, 0][..], 1, 6));
    assert_eq!(&*v.previous_data, &[b'h', 0]);
}

#[test]
fn a_captured_size_beyond_the_payload_is_truncated() {
    let p = PointerSize::P64;
    let b = B::default().ptr(p, 1).u32(0).u32(1).u32(1).wstr("").wstr("v").u16(0xFFFF).raw(&[1, 2]);
    assert_eq!(
        parse(&meta(Provider::KernelRegistry, 5, 0, p), &b.0),
        // KeyObject 8, Status + Type + DataSize 12, "" 2, "v" 4, CapturedDataSize 2.
        Err(ParseError::Truncated { field: "CapturedData", offset: 28 })
    );
}

#[test]
fn set_value_with_an_embedded_nul_in_the_value_name() {
    // Recorded on the host (plan 1b-2 live run): NtSetValueKey with the counted
    // name "a", NUL, "b". The kernel logs all three units, then the terminator.
    let raw = "e099d2f00e9affff00000000010000000400000000006100000062000000000000000000000000000000";
    let b: Vec<u8> = (0..raw.len()).step_by(2).map(|i| u8::from_str_radix(&raw[i..i + 2], 16).unwrap()).collect();
    let RawEvent::RegSetValue(v) = parse(&meta(Provider::KernelRegistry, 5, 0, PointerSize::P64), &b).unwrap() else {
        panic!()
    };
    assert_eq!(v.value_name.as_units(), &[u16::from(b'a'), 0, u16::from(b'b')]);
    assert_eq!((v.value_type, v.data_size, v.captured_data.len(), v.previous_data.len()), (1, 4, 0, 0));
    assert!(!v.value_name_ambiguous);
    // A newer version whose layout only starts with ours may append fields, so
    // its names stop at the first NUL; one whose layout equals ours stays exact.
    let newer = meta(Provider::KernelRegistry, 5, 1, PointerSize::P64);
    let prefix = parse_as(&newer, 0, &b, false);
    assert!(
        prefix.is_err() || matches!(prefix, Ok(RawEvent::RegSetValue(ref v)) if v.value_name.to_string_lossy() == "a")
    );
    assert!(
        matches!(parse_as(&newer, 0, &b, true), Ok(RawEvent::RegSetValue(ref v)) if v.value_name.as_units().len() == 3)
    );
}

#[test]
fn set_value_flags_a_name_that_could_end_in_two_places() {
    // Previous data that ends in zeros lets a NUL inside it end the name too:
    // "v", terminator, CapturedDataSize 0, PreviousDataType 3, PreviousDataSize 16,
    // PreviousDataCapturedSize 16, then 16 zero bytes.
    let p = PointerSize::P64;
    let b = B::default().ptr(p, 1).u32(0).u32(3).u32(4).wstr("").wstr("v").u16(0).u32(3).u32(16).u16(16).raw(&[0; 16]);
    let RawEvent::RegSetValue(v) = parse(&meta(Provider::KernelRegistry, 5, 0, p), &b.0).unwrap() else { panic!() };
    assert!(v.value_name_ambiguous);
}

#[test]
fn last_field_names_keep_embedded_nuls() {
    let p = PointerSize::P64;
    let units = |s: &str| s.encode_utf16().collect::<Vec<_>>();
    let del = B::default().ptr(p, 1).u32(0).wstr("").wstr("a\0b");
    let RawEvent::RegDeleteValue(d) = parse(&meta(Provider::KernelRegistry, 6, 0, p), &del.0).unwrap() else {
        panic!()
    };
    assert_eq!(d.value_name.as_units(), &units("a\0b")[..]);
    let open = B::default().ptr(p, 0).ptr(p, 2).u32(0).u32(1).wstr("").wstr("k\0x");
    let RawEvent::RegCreateKey(o) = parse(&meta(Provider::KernelRegistry, 1, 0, p), &open.0).unwrap() else { panic!() };
    assert_eq!(o.relative_name.as_units(), &units("k\0x")[..]);
}

#[test]
fn registry_open_close_and_delete_value() {
    let p = PointerSize::P64;
    let open = B::default().ptr(p, 0).ptr(p, 0xffff_d001).u32(0).u32(2).wstr("").wstr(r"\REGISTRY\MACHINE\SOFTWARE");
    let RawEvent::RegOpenKey(o) = parse(&meta(Provider::KernelRegistry, 2, 0, p), &open.0).unwrap() else { panic!() };
    assert_eq!((o.base_object, o.key_object, o.status, o.disposition), (0, 0xffff_d001, 0, 2));
    assert_eq!(o.relative_name.to_string_lossy(), r"\REGISTRY\MACHINE\SOFTWARE");

    let close = B::default().ptr(p, 0xffff_d001).u32(0).wstr("");
    assert_eq!(
        parse(&meta(Provider::KernelRegistry, 13, 0, p), &close.0),
        Ok(RawEvent::RegCloseKey(RegKey { key_object: 0xffff_d001, status: 0, key_name: WStr::default() }))
    );

    let del = B::default().ptr(p, 0xffff_d001).u32(0xC000_0034).wstr("").wstr("Gone");
    let RawEvent::RegDeleteValue(d) = parse(&meta(Provider::KernelRegistry, 6, 0, p), &del.0).unwrap() else {
        panic!()
    };
    assert_eq!((d.status, d.value_name.to_string_lossy()), (0xC000_0034, "Gone".to_string()));
}

#[test]
fn network_ports_are_big_endian_and_v6_addresses_are_16_bytes() {
    let p = PointerSize::P64;
    // TCP connect over IPv4: daddr 93.184.216.34:443 from 10.0.0.5:50000.
    let v4 = B::default()
        .u32(321)
        .u32(0)
        .raw(&[93, 184, 216, 34])
        .raw(&[10, 0, 0, 5])
        .u16_be(443)
        .u16_be(50000)
        .raw(&[0; 16])
        .u32(7)
        .u32(8);
    let RawEvent::TcpConnect(n) = parse(&meta(Provider::KernelNetwork, 12, 0, p), &v4.0).unwrap() else { panic!() };
    assert_eq!(n.pid, 321);
    assert_eq!((n.daddr, n.dport), ("93.184.216.34".parse().unwrap(), 443));
    assert_eq!((n.saddr, n.sport), ("10.0.0.5".parse().unwrap(), 50000));
    assert_eq!((n.seqnum, n.connid), (7, 8));

    // UDP receive over IPv6 (no TCP options).
    let lo = std::net::Ipv6Addr::LOCALHOST.octets();
    let v6 = B::default().u32(55).u32(1200).raw(&lo).raw(&lo).u16_be(53).u16_be(60000).u32(0).u32(0);
    let RawEvent::UdpRecv(n) = parse(&meta(Provider::KernelNetwork, 59, 0, p), &v6.0).unwrap() else { panic!() };
    assert_eq!((n.pid, n.size, n.daddr, n.dport, n.sport), (55, 1200, "::1".parse().unwrap(), 53, 60000));
}

#[test]
fn dns_query_completed() {
    let b = B::default().wstr("example.com").u32(28).u64(0x4000_0000).u32(9003).wstr("");
    let RawEvent::DnsQuery(q) = parse(&meta(Provider::DnsClient, 3008, 0, PointerSize::P32), &b.0).unwrap() else {
        panic!()
    };
    assert_eq!((q.query_name.to_string_lossy(), q.query_type, q.query_status), ("example.com".to_string(), 28, 9003));
    assert!(q.query_results.is_empty());
}

fn classic(p: PointerSize, sid: impl FnOnce(B) -> B) -> Vec<u8> {
    let b = B::default().ptr(p, 0xffff_e001).u32(4242).u32(1000).u32(1).u32(0x103).ptr(p, 0x1aa000).u32(0);
    sid(b).astr(b"cmd.exe").wstr(r#""C:\Windows\system32\cmd.exe" /c echo hi"#).wstr("").wstr("").0
}

#[test]
fn classic_process_with_a_user_sid_both_pointer_sizes() {
    for p in [PointerSize::P32, PointerSize::P64] {
        let b = classic(p, |b| b.ptr(p, 0xffff_9000).ptr(p, 0).sid(5, &[21, 1, 2, 3, 1001]));
        let RawEvent::ClassicProcess(c) = parse(&meta(Provider::ClassicProcess, 1, 4, p), &b).unwrap() else {
            panic!()
        };
        assert_eq!(c.kind, ClassicKind::Start);
        assert_eq!((c.pid, c.parent_pid, c.session_id, c.exit_status), (4242, 1000, 1, 0x103));
        assert_eq!(c.user_sid.unwrap().to_string(), "S-1-5-21-1-2-3-1001");
        assert_eq!(&*c.image_file_name, b"cmd.exe");
        assert_eq!(c.command_line.to_string_lossy(), r#""C:\Windows\system32\cmd.exe" /c echo hi"#);
    }
}

#[test]
fn classic_process_with_a_null_sid() {
    let b = classic(PointerSize::P64, |b| b.u32(0));
    let RawEvent::ClassicProcess(c) = parse(&meta(Provider::ClassicProcess, 3, 4, PointerSize::P64), &b).unwrap()
    else {
        panic!()
    };
    assert_eq!((c.kind, c.user_sid), (ClassicKind::DcStart, None));
    assert_eq!(&*c.image_file_name, b"cmd.exe");
}

#[test]
fn versions_we_do_not_know() {
    let m = |id, v| meta(Provider::KernelProcess, id, v, PointerSize::P64);
    let v4 = payload_for(layout::find(Provider::KernelProcess, 1, 4).unwrap().fields, PointerSize::P64);
    assert_eq!(parse(&m(1, 5), &v4), Err(ParseError::NewerVersion { version: 5, newest: 4 }));
    assert_eq!(parse(&m(1, 2), &v4), Err(ParseError::UnsupportedVersion { version: 2 }));
    assert_eq!(parse(&m(9, 0), &v4), Err(ParseError::UnknownEvent));
    assert_eq!(parse(&meta(Provider::ClassicProcess, 39, 5, PointerSize::P64), &v4), Err(ParseError::UnknownEvent));
    // After a prefix check, a v5 parses with the v4 layout.
    assert!(matches!(parse_as(&m(1, 5), 4, &v4, false), Ok(RawEvent::ProcessStart(_))));
    assert_eq!(parse_as(&m(1, 5), 9, &v4, false), Err(ParseError::UnsupportedVersion { version: 9 }));
}

fn any_meta() -> impl Strategy<Value = EventMeta> {
    (0..LAYOUTS.len(), 0u8..3, any::<bool>()).prop_map(|(i, bump, p64)| {
        let l = &LAYOUTS[i];
        let pointer_size = if p64 { PointerSize::P64 } else { PointerSize::P32 };
        EventMeta { provider: l.provider, id: l.id, version: l.version.saturating_add(bump), pointer_size }
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4096))]

    /// Arbitrary bytes never panic, for every event and both pointer sizes.
    #[test]
    fn arbitrary_payloads_never_panic(m in any_meta(), bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = parse(&m, &bytes);
        let _ = parse_as(&m, m.version, &bytes, true);
        let _ = parse_as(&m, m.version, &bytes, false);
    }

    /// Flipping bytes of a valid payload never panics either (reaches deeper fields).
    #[test]
    fn mutated_payloads_never_panic(m in any_meta(), flips in proptest::collection::vec((any::<usize>(), any::<u8>()), 1..8)) {
        let l = layout::newest(m.provider, m.id).unwrap();
        let mut bytes = payload_for(l.fields, m.pointer_size);
        for (i, v) in flips {
            let n = bytes.len();
            bytes[i % n] = v;
        }
        let _ = parse_as(&m, l.version, &bytes, true);
    }
}
