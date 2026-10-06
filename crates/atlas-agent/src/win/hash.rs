//! SHA-256 and Authenticode for Launch and Module Load images (sensor spec §6.3).
//!
//! One handle per file serves the cache key (volume serial, 128-bit file id,
//! USN), the bytes hashed and the signature check, so the three cannot come
//! from different files (plan 1b-3b, D5).

use std::collections::HashMap;
use std::sync::Mutex;

use atlas_schema::{Hashes, Signature, SignatureStatus};
use sha2::{Digest, Sha256};
use windows::Win32::Foundation::{ERROR_SHARING_VIOLATION, HANDLE, HWND};
use windows::Win32::Security::Cryptography::Catalog::{
    CATALOG_INFO, CryptCATAdminAcquireContext2, CryptCATAdminCalcHashFromFileHandle2, CryptCATAdminEnumCatalogFromHash,
    CryptCATAdminReleaseCatalogContext, CryptCATAdminReleaseContext, CryptCATCatalogInfoFromContext,
};
use windows::Win32::Security::Cryptography::{CERT_NAME_ATTR_TYPE, CertGetNameStringW, szOID_COMMON_NAME};
use windows::Win32::Security::WinTrust::{
    DRIVER_ACTION_VERIFY, WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_CATALOG_INFO, WINTRUST_DATA, WINTRUST_FILE_INFO,
    WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_CATALOG, WTD_CHOICE_FILE, WTD_REVOCATION_CHECK_NONE, WTD_REVOKE_NONE,
    WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE, WTHelperGetProvSignerFromChain,
    WTHelperProvDataFromStateData, WinVerifyTrust,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_BEGIN, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_SEQUENTIAL_SCAN, FILE_GENERIC_READ, FILE_ID_INFO,
    FILE_READ_DATA, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileIdInfo, GetFileInformationByHandleEx,
    GetFileSizeEx, OPEN_EXISTING, ReadFile, SetFilePointerEx,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{FSCTL_READ_FILE_USN_DATA, READ_FILE_USN_DATA};
use windows::core::{GUID, PCWSTR, w};

use super::util::{Owned, aligned, as_bytes, u32_at, u64_at, wide};

/// Cache key: which file, in which version (§6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct FileKey {
    volume: u64,
    id: [u8; 16],
    usn: i64,
}

/// One file's result, as `Reply::Enriched` carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Enriched {
    pub hashes: Option<Hashes>,
    pub signature: Option<Signature>,
    /// An operational error: the signature (and maybe the hash) is absent (§6.3).
    pub error: bool,
}

impl Enriched {
    fn failed() -> Self {
        Enriched { hashes: None, signature: None, error: true }
    }
}

/// Results by file version, shared by the two workers. Errors are not cached.
pub(crate) struct HashCache {
    map: HashMap<FileKey, Enriched>,
    /// Lowercased NT path → the keys stored under it, for `InvalidateHash`.
    by_path: HashMap<String, Vec<FileKey>>,
    cap: usize,
    pub evictions: u64,
}

impl HashCache {
    pub(crate) fn new(cap: usize) -> Self {
        HashCache { map: HashMap::new(), by_path: HashMap::new(), cap, evictions: 0 }
    }

    fn get(&self, key: &FileKey) -> Option<Enriched> {
        self.map.get(key).cloned()
    }

    fn insert(&mut self, nt_path: &str, key: FileKey, value: Enriched) {
        if self.map.len() >= self.cap {
            // Rare and cheap to rebuild: start over rather than track recency.
            self.evictions += self.map.len() as u64;
            self.map.clear();
            self.by_path.clear();
        }
        self.map.insert(key, value);
        self.by_path.entry(nt_path.to_lowercase()).or_default().push(key);
    }

    /// Forgets results for a path that changed (§6.3). The USN in the key
    /// already separates versions; this keeps the cache from holding them.
    pub(crate) fn invalidate(&mut self, nt_path: &str) {
        for key in self.by_path.remove(&nt_path.to_lowercase()).unwrap_or_default() {
            self.map.remove(&key);
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.map.len()
    }
}

/// What a signature check concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Valid,
    /// No embedded signature: try the catalogs.
    NotEmbedded,
    Invalid,
    /// An operational error: signature absent (§6.3).
    Error,
}

const TRUST_E_PROVIDER_UNKNOWN: u32 = 0x800B_0001;
const TRUST_E_ACTION_UNKNOWN: u32 = 0x800B_0002;
const TRUST_E_SUBJECT_FORM_UNKNOWN: u32 = 0x800B_0003;
const TRUST_E_NOSIGNATURE: u32 = 0x800B_0100;
const TRUST_E_SYSTEM_ERROR: u32 = 0x8009_6001;
const CRYPT_E_REVOKED: u32 = 0x8009_2010;
const CRYPT_E_SECURITY_SETTINGS: u32 = 0x8009_2026;

/// `WinVerifyTrust`'s result → verdict (§6.3). Failures of the signature or
/// its chain are `Invalid`: the certificate facility (`0x800B….`: untrusted
/// root, expired, revoked, explicit distrust, chaining), the trust errors
/// `0x80096002`–`0x800960FF` (bad digest, no signer certificate, bad
/// certificate signature), revocation, admin policy, and malformed ASN.1
/// (`0x80093xxx`). Anything else, such as a file or RPC error, is `Error`.
fn classify(hr: i32) -> Verdict {
    let hr = hr as u32;
    match hr {
        0 => Verdict::Valid,
        TRUST_E_NOSIGNATURE | TRUST_E_SUBJECT_FORM_UNKNOWN => Verdict::NotEmbedded,
        TRUST_E_PROVIDER_UNKNOWN | TRUST_E_ACTION_UNKNOWN | TRUST_E_SYSTEM_ERROR => Verdict::Error,
        CRYPT_E_REVOKED | CRYPT_E_SECURITY_SETTINGS => Verdict::Invalid,
        _ if hr >> 16 == 0x800B => Verdict::Invalid,
        0x8009_6002..=0x8009_60FF => Verdict::Invalid,
        0x8009_3000..=0x8009_31FF => Verdict::Invalid,
        _ => Verdict::Error,
    }
}

/// Hashes and checks files; one per worker thread (catalog contexts are per thread).
pub(crate) struct Enricher {
    /// Catalog contexts, SHA-256 first, then SHA-1 for files that older
    /// catalogs list only by SHA-1 (R-m2).
    cat_admins: Vec<isize>,
    size_cap: u64,
}

impl Enricher {
    pub(crate) fn new(size_cap: u64) -> Self {
        let cat_admins = [w!("SHA256"), w!("SHA1")]
            .into_iter()
            .filter_map(|alg| {
                let mut admin = 0isize;
                // SAFETY: writes the context handle; released in Drop.
                unsafe { CryptCATAdminAcquireContext2(&mut admin, Some(&DRIVER_ACTION_VERIFY), alg, None, None) }
                    .is_ok()
                    .then_some(admin)
            })
            .collect();
        Enricher { cat_admins, size_cap }
    }

    /// Hash and signature of the file at `nt_path`, from the cache when this
    /// version of the file was seen before.
    pub(crate) fn enrich(&self, nt_path: &str, cache: &Mutex<HashCache>) -> Enriched {
        let path = format!(r"\\?\GLOBALROOT{nt_path}");
        let Some(file) = open(&path) else { return Enriched::failed() };
        // A handle with write access can change the file without changing its
        // USN (R-M1): then the cache is neither read nor written.
        let key = if writer_open(&path) { None } else { file_key(file.raw()) };
        if let Some(hit) = key.and_then(|k| cache.lock().expect("hash cache").get(&k)) {
            return hit;
        }
        let Some(size) = size(file.raw()) else { return Enriched::failed() };
        let hashes = if size <= self.size_cap {
            match sha256(file.raw()) {
                Some(h) => Some(Hashes { sha256: Some(h) }),
                None => return Enriched::failed(),
            }
        } else {
            None
        };
        let (signature, error) = match self.signature(&path, file.raw()) {
            Some(s) => (Some(s), false),
            None => (None, true),
        };
        let out = Enriched { hashes, signature, error };
        // Cache only a version that stayed the same while it was read.
        if let Some(k) = key
            && !error
            && file_key(file.raw()) == Some(k)
        {
            cache.lock().expect("hash cache").insert(nt_path, k, out.clone());
        }
        out
    }

    /// Embedded signature first, then the catalogs; `None` on an operational error.
    fn signature(&self, path: &str, file: HANDLE) -> Option<Signature> {
        let path_w = wide(path);
        let mut info = WINTRUST_FILE_INFO {
            cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
            pcwszFilePath: PCWSTR(path_w.as_ptr()),
            hFile: file,
            ..Default::default()
        };
        let mut data = trust_data();
        data.dwUnionChoice = WTD_CHOICE_FILE;
        data.Anonymous.pFile = &mut info;
        let (hr, signer) = verify(&mut data);
        match classify(hr) {
            Verdict::Valid => Some(Signature { signer, status: SignatureStatus::Valid }),
            Verdict::Invalid => Some(Signature { signer, status: SignatureStatus::Invalid }),
            Verdict::Error => None,
            Verdict::NotEmbedded => self.catalog(&path_w, file),
        }
    }

    /// The catalogs, by each hash algorithm in turn: `Unsigned` when none
    /// lists the file, `None` on an operational error.
    fn catalog(&self, path_w: &[u16], file: HANDLE) -> Option<Signature> {
        if self.cat_admins.is_empty() {
            return None;
        }
        for &admin in &self.cat_admins {
            if let Some(found) = catalog_with(admin, path_w, file) {
                return found;
            }
        }
        Some(Signature { signer: None, status: SignatureStatus::Unsigned })
    }
}

/// `None` when no catalog of this context's algorithm lists the file;
/// otherwise the verdict (itself `None` on an operational error).
fn catalog_with(admin: isize, path_w: &[u16], file: HANDLE) -> Option<Option<Signature>> {
    if rewind(file).is_none() {
        return Some(None);
    }
    let mut len = 0u32;
    // SAFETY: a size query (no buffer); fails with a size when the buffer is too small.
    let _ = unsafe { CryptCATAdminCalcHashFromFileHandle2(admin, file, &mut len, None, None) };
    if len == 0 || len > 64 {
        return Some(None);
    }
    let mut hash = vec![0u8; len as usize];
    // SAFETY: `hash` is writable for `len` bytes.
    if unsafe { CryptCATAdminCalcHashFromFileHandle2(admin, file, &mut len, Some(hash.as_mut_ptr()), None) }.is_err() {
        return Some(None);
    }
    // SAFETY: `hash` is valid; the returned context is released below.
    let cat = unsafe { CryptCATAdminEnumCatalogFromHash(admin, &hash, None, None) };
    if cat == 0 {
        return None;
    }
    let mut ci = CATALOG_INFO { cbStruct: size_of::<CATALOG_INFO>() as u32, ..Default::default() };
    // SAFETY: `cat` is a live catalog context.
    let out = match unsafe { CryptCATCatalogInfoFromContext(cat, &mut ci, 0) } {
        Err(_) => None,
        Ok(()) => {
            let tag = wide(&hash.iter().map(|b| format!("{b:02X}")).collect::<String>());
            let mut member = WINTRUST_CATALOG_INFO {
                cbStruct: size_of::<WINTRUST_CATALOG_INFO>() as u32,
                pcwszCatalogFilePath: PCWSTR(ci.wszCatalogFile.as_ptr()),
                pcwszMemberTag: PCWSTR(tag.as_ptr()),
                pcwszMemberFilePath: PCWSTR(path_w.as_ptr()),
                hMemberFile: file,
                pbCalculatedFileHash: hash.as_mut_ptr(),
                cbCalculatedFileHash: len,
                hCatAdmin: admin,
                ..Default::default()
            };
            let mut data = trust_data();
            data.dwUnionChoice = WTD_CHOICE_CATALOG;
            data.Anonymous.pCatalog = &mut member;
            let (hr, signer) = verify(&mut data);
            match classify(hr) {
                Verdict::Valid => Some(Signature { signer, status: SignatureStatus::Valid }),
                Verdict::Invalid => Some(Signature { signer, status: SignatureStatus::Invalid }),
                Verdict::NotEmbedded => Some(Signature { signer: None, status: SignatureStatus::Unsigned }),
                Verdict::Error => None,
            }
        }
    };
    // SAFETY: releases the context enumerated above.
    unsafe {
        let _ = CryptCATAdminReleaseCatalogContext(admin, cat, 0);
    }
    Some(out)
}

impl Drop for Enricher {
    fn drop(&mut self) {
        for &admin in &self.cat_admins {
            // SAFETY: acquired in `new`.
            unsafe {
                let _ = CryptCATAdminReleaseContext(admin, 0);
            }
        }
    }
}

/// No UI, no revocation checks and no network: the sensor never fetches (§6.3).
fn trust_data() -> WINTRUST_DATA {
    WINTRUST_DATA {
        cbStruct: size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL | WTD_REVOCATION_CHECK_NONE,
        ..Default::default()
    }
}

/// Runs the check and reads the leaf certificate's subject CN before closing
/// the state.
fn verify(data: &mut WINTRUST_DATA) -> (i32, Option<String>) {
    let mut action: GUID = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    data.dwStateAction = WTD_STATEACTION_VERIFY;
    // SAFETY: `data` and what it points to outlive both calls.
    let hr = unsafe { WinVerifyTrust(HWND::default(), &mut action, (data as *mut WINTRUST_DATA).cast()) };
    let signer = signer_cn(data.hWVTStateData);
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    // SAFETY: as above; closes the state opened by the first call.
    unsafe { WinVerifyTrust(HWND::default(), &mut action, (data as *mut WINTRUST_DATA).cast()) };
    (hr, signer)
}

fn signer_cn(state: HANDLE) -> Option<String> {
    if state.is_invalid() || state.0.is_null() {
        return None;
    }
    // SAFETY: `state` is open until WTD_STATEACTION_CLOSE; every pointer read
    // here belongs to it and is checked for null and count first.
    unsafe {
        let prov = WTHelperProvDataFromStateData(state);
        if prov.is_null() {
            return None;
        }
        let sgnr = WTHelperGetProvSignerFromChain(prov, 0, false, 0);
        if sgnr.is_null() || (*sgnr).csCertChain == 0 || (*sgnr).pasCertChain.is_null() {
            return None;
        }
        let cert = (*(*sgnr).pasCertChain).pCert;
        if cert.is_null() {
            return None;
        }
        let mut buf = [0u16; 256];
        let n = CertGetNameStringW(cert, CERT_NAME_ATTR_TYPE, 0, Some(szOID_COMMON_NAME.0.cast()), Some(&mut buf));
        let s = String::from_utf16_lossy(&buf[..(n as usize).saturating_sub(1).min(buf.len())]);
        (!s.is_empty()).then_some(s)
    }
}

/// Opens for reading with full sharing, so the agent never blocks the file's
/// users (§6.3). Backup semantics: SYSTEM with `SeBackupPrivilege` can read
/// any file.
fn open(path: &str) -> Option<Owned> {
    let p = wide(path);
    // SAFETY: `p` is NUL-terminated; the handle is owned by `Owned`.
    let h = unsafe {
        CreateFileW(
            PCWSTR(p.as_ptr()),
            FILE_GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_SEQUENTIAL_SCAN,
            None,
        )
    };
    h.ok().map(Owned)
}

/// Whether some handle to the file has write access: an open for reading
/// that refuses write sharing then fails with a sharing violation. Our own
/// handle has no write access, so it never counts.
fn writer_open(path: &str) -> bool {
    let p = wide(path);
    // SAFETY: `p` is NUL-terminated; the handle, if any, is closed at once.
    let h = unsafe {
        CreateFileW(
            PCWSTR(p.as_ptr()),
            FILE_READ_DATA.0,
            FILE_SHARE_READ | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    };
    match h {
        Ok(h) => {
            drop(Owned(h));
            false
        }
        Err(e) => e.code() == ERROR_SHARING_VIOLATION.to_hresult(),
    }
}

/// The cache key, or `None` when the volume keeps no USNs (not cached, §6.3).
fn file_key(h: HANDLE) -> Option<FileKey> {
    let mut id = FILE_ID_INFO::default();
    // SAFETY: `id` is writable for its size.
    unsafe {
        GetFileInformationByHandleEx(
            h,
            FileIdInfo,
            (&mut id as *mut FILE_ID_INFO).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    }
    .ok()?;
    let usn = usn(h)?;
    Some(FileKey { volume: id.VolumeSerialNumber, id: id.FileId.Identifier, usn })
}

/// The file's USN from its USN_RECORD (V2 or V3). Zero means the journal has
/// not recorded the file: treated as no USN.
fn usn(h: HANDLE) -> Option<i64> {
    let input = READ_FILE_USN_DATA { MinMajorVersion: 2, MaxMajorVersion: 3 };
    let mut out = aligned(1024);
    let mut ret = 0u32;
    // SAFETY: input and output buffers are valid for the sizes given.
    unsafe {
        DeviceIoControl(
            h,
            FSCTL_READ_FILE_USN_DATA,
            Some((&input as *const READ_FILE_USN_DATA).cast()),
            size_of::<READ_FILE_USN_DATA>() as u32,
            Some(out.as_mut_ptr().cast()),
            (out.len() * 8) as u32,
            Some(&mut ret),
            None,
        )
    }
    .ok()?;
    let b = as_bytes(&out);
    let major = u32_at(b, 4)? & 0xFFFF;
    let off = match major {
        2 => 24,
        3 => 40,
        _ => return None,
    };
    Some(u64_at(b, off)? as i64).filter(|u| *u > 0)
}

fn size(h: HANDLE) -> Option<u64> {
    let mut n = 0i64;
    // SAFETY: writes one i64.
    unsafe { GetFileSizeEx(h, &mut n) }.ok()?;
    u64::try_from(n).ok()
}

fn rewind(h: HANDLE) -> Option<()> {
    // SAFETY: moves the file pointer of a synchronous handle.
    unsafe { SetFilePointerEx(h, 0, None, FILE_BEGIN) }.ok()
}

fn sha256(h: HANDLE) -> Option<[u8; 32]> {
    rewind(h)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let mut n = 0u32;
        // SAFETY: `buf` is writable; synchronous read.
        unsafe { ReadFile(h, Some(&mut buf), Some(&mut n), None) }.ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n as usize]);
    }
    Some(hasher.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nt(dos: &str) -> String {
        let devices = super::super::lookups::query_drives();
        let (dev, drive) = devices.iter().find(|(_, d)| dos[..2].eq_ignore_ascii_case(d)).expect("drive mapped");
        format!("{dev}{}", &dos[drive.len()..])
    }

    /// A file path in a directory of its own, removed with the directory on drop.
    struct Temp(std::path::PathBuf);

    impl std::ops::Deref for Temp {
        type Target = std::path::Path;
        fn deref(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl AsRef<std::path::Path> for Temp {
        fn as_ref(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            if let Some(dir) = self.0.parent() {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }

    fn temp(name: &str) -> Temp {
        let dir = std::env::temp_dir().join(format!("atlas-hash-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir.join(name))
    }

    #[test]
    fn classification() {
        assert_eq!(classify(0), Verdict::Valid);
        for nosig in [0x800B_0100u32, 0x800B_0003] {
            assert_eq!(classify(nosig as i32), Verdict::NotEmbedded);
        }
        // Bad digest, untrusted root, revoked (cert and CRYPT_E), distrust, expired, bad ASN.1.
        for bad in [0x8009_6010u32, 0x800B_0109, 0x800B_010C, 0x8009_2010, 0x800B_0111, 0x800B_0101, 0x8009_310B] {
            assert_eq!(classify(bad as i32), Verdict::Invalid, "{bad:#x}");
        }
        // Sharing violation, RPC server unavailable, trust system error, unknown provider.
        for err in [0x8007_0020u32, 0x8007_06BA, 0x8009_6001, 0x800B_0001] {
            assert_eq!(classify(err as i32), Verdict::Error, "{err:#x}");
        }
    }

    #[test]
    fn hashes_a_file_and_caches_by_version() {
        let p = temp("hello.bin");
        std::fs::write(&p, b"hello").unwrap();
        let path = nt(&p.to_string_lossy());
        let cache = Mutex::new(HashCache::new(100));
        let e = Enricher::new(100 << 20);
        let r = e.enrich(&path, &cache);
        let want: [u8; 32] = Sha256::digest(b"hello").into();
        assert_eq!(r.hashes, Some(Hashes { sha256: Some(want) }));
        assert_eq!(r.signature, Some(Signature { signer: None, status: SignatureStatus::Unsigned }));
        assert!(!r.error);
        assert_eq!(cache.lock().unwrap().len(), 1, "cached under its USN");

        // A new version has a new USN: a miss, and the new content.
        std::fs::write(&p, b"world").unwrap();
        let r2 = e.enrich(&path, &cache);
        assert_eq!(r2.hashes.unwrap().sha256, Some(Sha256::digest(b"world").into()));
        assert_eq!(cache.lock().unwrap().len(), 2);
        cache.lock().unwrap().invalidate(&path.to_uppercase());
        assert_eq!(cache.lock().unwrap().len(), 0, "invalidation ignores case");
    }

    /// R-M1: writes through a handle that stays open keep the USN, so while a
    /// writer is open nothing is cached and every request hashes the bytes.
    #[test]
    fn files_open_for_writing_are_never_served_from_the_cache() {
        use std::io::{Seek, SeekFrom, Write};
        let p = temp("writer.bin");
        let mut w = std::fs::OpenOptions::new().create(true).truncate(true).read(true).write(true).open(&p).unwrap();
        w.write_all(b"benign").unwrap();
        w.flush().unwrap();
        let path = nt(&p.to_string_lossy());
        let cache = Mutex::new(HashCache::new(100));
        let e = Enricher::new(1 << 20);
        let first = e.enrich(&path, &cache).hashes.unwrap().sha256.unwrap();
        assert_eq!(first, <[u8; 32]>::from(Sha256::digest(b"benign")));
        assert_eq!(cache.lock().unwrap().len(), 0, "a writer is open: not cached");
        w.seek(SeekFrom::Start(0)).unwrap();
        w.write_all(b"malice").unwrap();
        w.flush().unwrap();
        let second = e.enrich(&path, &cache).hashes.unwrap().sha256.unwrap();
        assert_eq!(second, <[u8; 32]>::from(Sha256::digest(b"malice")), "the new bytes, not a cached hash");
        drop(w);
        e.enrich(&path, &cache);
        assert_eq!(cache.lock().unwrap().len(), 1, "cached once the writer closed");
    }

    #[test]
    fn size_cap_skips_the_hash_only() {
        let p = temp("big.bin");
        std::fs::write(&p, vec![0u8; 4096]).unwrap();
        let r = Enricher::new(4095).enrich(&nt(&p.to_string_lossy()), &Mutex::new(HashCache::new(10)));
        assert_eq!(r.hashes, None);
        assert_eq!(r.signature.map(|s| s.status), Some(SignatureStatus::Unsigned));
    }

    #[test]
    fn missing_files_are_errors_and_not_cached() {
        let cache = Mutex::new(HashCache::new(10));
        let r = Enricher::new(1 << 20).enrich(&nt(&temp("absent.exe").to_string_lossy()), &cache);
        assert_eq!(r, Enriched::failed());
        assert_eq!(cache.lock().unwrap().len(), 0);
    }

    #[test]
    fn catalog_signed_os_file() {
        let notepad = format!(r"{}\System32\notepad.exe", std::env::var("SystemRoot").unwrap());
        let r = Enricher::new(100 << 20).enrich(&nt(&notepad), &Mutex::new(HashCache::new(10)));
        let s = r.signature.expect("signature");
        assert_eq!(s.status, SignatureStatus::Valid);
        assert_eq!(s.signer.as_deref(), Some("Microsoft Windows"));
    }

    /// A binary with an embedded signature, present on the host and the CI runner.
    fn embedded_signed() -> String {
        let root = std::env::var("SystemRoot").unwrap();
        let pf = std::env::var("ProgramFiles").unwrap();
        let candidates = [
            format!(r"{pf}\Windows Defender\MpCmdRun.exe"),
            format!(r"{root}\System32\SecurityHealthService.exe"),
            format!(r"{root}\System32\drivers\WdFilter.sys"),
            format!(r"{pf}\Git\cmd\git.exe"),
        ];
        candidates.into_iter().find(|c| std::path::Path::new(c).exists()).expect("an embedded-signed binary")
    }

    #[test]
    fn embedded_signature_and_tampering() {
        let e = Enricher::new(100 << 20);
        let cache = Mutex::new(HashCache::new(10));
        let original = embedded_signed();
        let r = e.enrich(&nt(&original), &cache);
        let s = r.signature.expect("signature");
        assert_eq!(s.status, SignatureStatus::Valid, "{original}");
        assert!(s.signer.is_some_and(|n| !n.is_empty()));

        // One flipped byte in the middle: the embedded digest no longer matches.
        let mut bytes = std::fs::read(&original).unwrap();
        let mid = bytes.len() / 2;
        bytes[mid] ^= 0xFF;
        let copy = temp("tampered.exe");
        std::fs::write(&copy, &bytes).unwrap();
        let r = e.enrich(&nt(&copy.to_string_lossy()), &cache);
        assert_eq!(r.signature.map(|s| s.status), Some(SignatureStatus::Invalid));
        assert!(r.hashes.is_some() && !r.error);
    }
}
