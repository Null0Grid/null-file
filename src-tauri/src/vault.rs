//! The Vault: an encrypted folder that needs a PIN to open.
//!
//! Everything lives in `~/.local/share/dev.nullgrid.file/vault/`:
//!
//! - `vault.json`: KDF settings and the data key, wrapped twice:
//!   - KEK_pin = Argon2id(PIN, salt, 64 MiB, t=3, p=1)
//!   - KEK_rec = SHA-256("null-file vault recovery v1\0" || 32-byte recovery entropy),
//!     shown to the user once as 24 BIP-39 words
//!   - wrap = nonce(24) | XChaCha20-Poly1305(KEK, data key, aad)
//! - `index.bin`: `NFVIDX1\n` | nonce | XChaCha20-Poly1305(data key, JSON index).
//!   The index holds every name, folder, size and date, so nothing about the
//!   contents is readable on disk.
//! - `blobs/<random id>`: one per file, `NFVBLB1\n` | stream nonce(19) | chunks.
//!   Chunks are 1 MiB of plaintext sealed with the STREAM construction
//!   (XChaCha20-Poly1305, big-endian 32-bit counter, last-chunk flag), with the
//!   blob id as associated data so blobs can't be swapped or truncated.
//! - `attempts.json`: wrong-PIN counter for the in-app delay (plain, not secret).
//!
//! Vault paths in the UI are `vault:/<relative path>`.

use crate::fsops::{self, kind_of, natural_cmp, Entry, Listing};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chacha20poly1305::aead::generic_array::GenericArray;
use chacha20poly1305::aead::{rand_core::RngCore, stream, Aead, AeadCore, KeyInit, OsRng, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use zeroize::Zeroizing;

pub const PREFIX: &str = "vault:/";
const IDX_MAGIC: &[u8; 8] = b"NFVIDX1\n";
const BLOB_MAGIC: &[u8; 8] = b"NFVBLB1\n";
const AAD_PIN: &[u8] = b"null-file vault data key v1/pin";
const AAD_REC: &[u8] = b"null-file vault data key v1/recovery";
const CHUNK: usize = 1 << 20;
const TAG: usize = 16;
const STREAM_NONCE: usize = 19;
pub const PIN_MIN: usize = 3;
pub const PIN_MAX: usize = 300;

pub type Key = Zeroizing<[u8; 32]>;
pub type R<T> = Result<T, String>;
pub type Shared = Arc<Mutex<Option<Open>>>;

/* ---------------- paths ---------------- */

pub fn is_vault(p: &str) -> bool {
    p == "vault:" || p.starts_with(PREFIX)
}

/// "vault:/a/b/" → "a/b". Rejects "." and ".." parts.
pub fn rel(p: &str) -> R<String> {
    let s = p.strip_prefix(PREFIX).or_else(|| p.strip_prefix("vault:")).unwrap_or(p);
    let s = s.trim_matches('/');
    if s.is_empty() {
        return Ok(String::new());
    }
    for part in s.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(format!("\"{p}\" isn't a valid vault path."));
        }
    }
    Ok(s.to_string())
}

pub fn vpath(rel: &str) -> String {
    format!("{PREFIX}{rel}")
}

fn parent(rel: &str) -> &str {
    rel.rfind('/').map(|i| &rel[..i]).unwrap_or("")
}

fn name(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

fn join(dir: &str, n: &str) -> String {
    if dir.is_empty() {
        n.to_string()
    } else {
        format!("{dir}/{n}")
    }
}

fn under(rel: &str, dir: &str) -> bool {
    rel.len() > dir.len() + 1 && rel.starts_with(dir) && rel.as_bytes()[dir.len()] == b'/'
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/* ---------------- on-disk formats ---------------- */

#[derive(Serialize, Deserialize, Clone)]
struct Header {
    version: u32,
    kdf: String,
    m_kib: u32,
    t: u32,
    p: u32,
    salt: String,
    wrap_pin: String,
    wrap_rec: String,
    created: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Node {
    pub dir: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub blob: String,
    pub size: u64,
    pub modified: i64,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Index {
    pub version: u32,
    pub nodes: BTreeMap<String, Node>,
}

#[derive(Serialize, Deserialize, Default)]
struct Attempts {
    failures: u32,
    until: i64,
}

/// Where the vault is and how hard the PIN key is to derive.
#[derive(Clone)]
pub struct Store {
    pub root: PathBuf,
    pub m_kib: u32,
    pub t: u32,
}

impl Store {
    pub fn at(root: PathBuf) -> Store {
        Store { root, m_kib: 64 * 1024, t: 3 }
    }
    pub fn default_root() -> PathBuf {
        let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| fsops::home().join(".local/share"));
        data.join("dev.nullgrid.file/vault")
    }
    fn header(&self) -> PathBuf {
        self.root.join("vault.json")
    }
    fn index(&self) -> PathBuf {
        self.root.join("index.bin")
    }
    pub fn blobs(&self) -> PathBuf {
        self.root.join("blobs")
    }
    fn attempts(&self) -> PathBuf {
        self.root.join("attempts.json")
    }
    pub fn exists(&self) -> bool {
        self.header().is_file()
    }
}

fn write_private(p: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = p.with_extension("tmp");
    {
        let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, p)
}

fn mkdir_private(p: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    fs::DirBuilder::new().recursive(true).mode(0o700).create(p)?;
    fs::set_permissions(p, fs::Permissions::from_mode(0o700))
}

/* ---------------- small crypto helpers ---------------- */

pub fn check_pin(pin: &str) -> R<()> {
    if pin.len() < PIN_MIN || pin.len() > PIN_MAX {
        return Err(format!("The PIN must be {PIN_MIN} to {PIN_MAX} digits."));
    }
    if !pin.bytes().all(|b| b.is_ascii_digit()) {
        return Err("The PIN can only contain digits.".into());
    }
    Ok(())
}

fn derive_pin(pin: &str, salt: &[u8], m_kib: u32, t: u32) -> R<Key> {
    let params = argon2::Params::new(m_kib, t, 1, Some(32)).map_err(|e| format!("Bad key settings: {e}"))?;
    let a = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut out = Zeroizing::new([0u8; 32]);
    a.hash_password_into(pin.as_bytes(), salt, out.as_mut()).map_err(|e| format!("Couldn't derive the key: {e}"))?;
    Ok(out)
}

fn rec_kek(entropy: &[u8]) -> Key {
    let mut h = Sha256::new();
    h.update(b"null-file vault recovery v1\0");
    h.update(entropy);
    let mut out = Zeroizing::new([0u8; 32]);
    out.copy_from_slice(&h.finalize());
    out
}

fn seal(key: &[u8; 32], plain: &[u8], aad: &[u8]) -> Vec<u8> {
    let c = XChaCha20Poly1305::new(key.into());
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ct = c.encrypt(&nonce, Payload { msg: plain, aad }).expect("encryption can't fail for in-memory data");
    let mut out = Vec::with_capacity(24 + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    out
}

fn unseal(key: &[u8; 32], blob: &[u8], aad: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    if blob.len() < 24 + TAG {
        return None;
    }
    let c = XChaCha20Poly1305::new(key.into());
    c.decrypt(XNonce::from_slice(&blob[..24]), Payload { msg: &blob[24..], aad }).ok().map(Zeroizing::new)
}

fn random_id() -> String {
    let mut b = [0u8; 16];
    OsRng.fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// 24 recovery words → 32 bytes of entropy.
fn parse_recovery(words: &str) -> R<Zeroizing<Vec<u8>>> {
    let norm = words.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    let m = bip39::Mnemonic::parse_normalized(&norm).map_err(|_| "Those words aren't a valid recovery key. Check the spelling and the order.".to_string())?;
    let e = Zeroizing::new(m.to_entropy());
    if e.len() != 32 {
        return Err("A recovery key is 24 words.".into());
    }
    Ok(e)
}

/* ---------------- blobs (streamed, chunked) ---------------- */

fn read_full(r: &mut impl Read, buf: &mut Vec<u8>, want: usize) -> io::Result<()> {
    buf.clear();
    buf.resize(want, 0);
    let mut n = 0;
    while n < want {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    buf.truncate(n);
    Ok(())
}

fn damaged() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "this vault file is damaged or was changed outside Null File")
}

/// Encrypt `src` into `dst`. `on(bytes)` reports progress and can return an
/// error (Interrupted) to cancel.
pub fn encrypt_stream(key: &[u8; 32], id: &str, src: &mut impl Read, dst: &mut impl Write, on: &mut dyn FnMut(u64) -> io::Result<()>) -> io::Result<u64> {
    let mut nonce = [0u8; STREAM_NONCE];
    OsRng.fill_bytes(&mut nonce);
    dst.write_all(BLOB_MAGIC)?;
    dst.write_all(&nonce)?;
    let mut enc = stream::EncryptorBE32::from_aead(XChaCha20Poly1305::new(key.into()), GenericArray::from_slice(&nonce));
    let aad = id.as_bytes();
    let (mut cur, mut next) = (Zeroizing::new(Vec::with_capacity(CHUNK)), Zeroizing::new(Vec::with_capacity(CHUNK)));
    read_full(src, &mut cur, CHUNK)?;
    let mut total = 0u64;
    loop {
        read_full(src, &mut next, CHUNK)?;
        total += cur.len() as u64;
        if next.is_empty() {
            let ct = enc.encrypt_last(Payload { msg: &cur, aad }).map_err(|_| damaged())?;
            dst.write_all(&ct)?;
            on(cur.len() as u64)?;
            break;
        }
        let ct = enc.encrypt_next(Payload { msg: &cur, aad }).map_err(|_| damaged())?;
        dst.write_all(&ct)?;
        on(cur.len() as u64)?;
        std::mem::swap(&mut cur, &mut next);
    }
    dst.flush()?;
    Ok(total)
}

pub fn decrypt_stream(key: &[u8; 32], id: &str, src: &mut impl Read, dst: &mut impl Write, on: &mut dyn FnMut(u64) -> io::Result<()>) -> io::Result<u64> {
    let mut head = [0u8; 8 + STREAM_NONCE];
    src.read_exact(&mut head).map_err(|_| damaged())?;
    if &head[..8] != BLOB_MAGIC {
        return Err(damaged());
    }
    let mut dec = stream::DecryptorBE32::from_aead(XChaCha20Poly1305::new(key.into()), GenericArray::from_slice(&head[8..]));
    let aad = id.as_bytes();
    let (mut cur, mut next) = (Vec::with_capacity(CHUNK + TAG), Vec::with_capacity(CHUNK + TAG));
    read_full(src, &mut cur, CHUNK + TAG)?;
    let mut total = 0u64;
    loop {
        read_full(src, &mut next, CHUNK + TAG)?;
        if next.is_empty() {
            let pt = Zeroizing::new(dec.decrypt_last(Payload { msg: &cur, aad }).map_err(|_| damaged())?);
            dst.write_all(&pt)?;
            total += pt.len() as u64;
            on(pt.len() as u64)?;
            break;
        }
        let pt = Zeroizing::new(dec.decrypt_next(Payload { msg: &cur, aad }).map_err(|_| damaged())?);
        dst.write_all(&pt)?;
        total += pt.len() as u64;
        on(pt.len() as u64)?;
        std::mem::swap(&mut cur, &mut next);
    }
    dst.flush()?;
    Ok(total)
}

/// Encrypt a real file into a new blob. Returns (blob id, plaintext size).
pub fn encrypt_file(key: &[u8; 32], blobs: &Path, src: &Path, on: &mut dyn FnMut(u64) -> io::Result<()>) -> io::Result<(String, u64)> {
    let mut r = io::BufReader::new(fs::File::open(src)?);
    encrypt_reader(key, blobs, &mut r, on)
}

fn encrypt_reader(key: &[u8; 32], blobs: &Path, r: &mut impl Read, on: &mut dyn FnMut(u64) -> io::Result<()>) -> io::Result<(String, u64)> {
    use std::os::unix::fs::OpenOptionsExt;
    let id = random_id();
    let tmp = blobs.join(format!("{id}.part"));
    let res = (|| {
        let f = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        let mut w = io::BufWriter::new(f);
        let n = encrypt_stream(key, &id, r, &mut w, on)?;
        w.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        Ok(n)
    })();
    match res {
        Ok(n) => {
            fs::rename(&tmp, blobs.join(&id))?;
            Ok((id, n))
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Decrypt a blob to a new real file (never overwrites; removes a partial file on error).
pub fn decrypt_to_file(key: &[u8; 32], blobs: &Path, id: &str, dst: &Path, mode: u32, on: &mut dyn FnMut(u64) -> io::Result<()>) -> io::Result<u64> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut r = io::BufReader::new(fs::File::open(blobs.join(id))?);
    let f = fs::OpenOptions::new().write(true).create_new(true).mode(mode).open(dst)?;
    let mut w = io::BufWriter::new(f);
    match decrypt_stream(key, id, &mut r, &mut w, on) {
        Ok(n) => {
            w.flush()?;
            Ok(n)
        }
        Err(e) => {
            drop(w);
            let _ = fs::remove_file(dst);
            Err(e)
        }
    }
}

/* ---------------- an unlocked vault ---------------- */

/// A decrypted copy handed to another app, kept in RAM-backed storage.
pub struct TmpFile {
    pub rel: String,
    pub path: PathBuf,
    pub modified: SystemTime,
    pub len: u64,
}

pub struct Open {
    pub store: Store,
    pub key: Key,
    pub index: Index,
    pub tmp: Vec<TmpFile>,
    tmp_dir: Option<PathBuf>,
}

#[derive(Serialize)]
pub struct Status {
    pub exists: bool,
    pub unlocked: bool,
    pub wait: i64,
    pub failures: u32,
}

fn read_attempts(s: &Store) -> Attempts {
    fs::read(s.attempts()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn status(s: &Store, unlocked: bool) -> Status {
    let a = read_attempts(s);
    Status { exists: s.exists(), unlocked, wait: (a.until - now()).max(0), failures: a.failures }
}

/// Seconds to wait after `failures` wrong PINs: none for the first 3, then 5 s doubling, at most 15 min.
pub fn delay_for(failures: u32) -> i64 {
    if failures < 3 {
        0
    } else {
        (5i64 << (failures - 3).min(10)).min(900)
    }
}

fn record_failure(s: &Store) -> i64 {
    let mut a = read_attempts(s);
    a.failures += 1;
    let d = delay_for(a.failures);
    a.until = now() + d;
    let _ = write_private(&s.attempts(), serde_json::to_string(&a).unwrap().as_bytes());
    d
}

fn clear_failures(s: &Store) {
    let _ = fs::remove_file(s.attempts());
}

fn read_header(s: &Store) -> R<Header> {
    let b = fs::read(s.header()).map_err(|e| format!("Couldn't read the vault: {e}"))?;
    serde_json::from_slice(&b).map_err(|_| "The vault's key file is damaged.".to_string())
}

fn write_header(s: &Store, h: &Header) -> R<()> {
    write_private(&s.header(), serde_json::to_string_pretty(h).unwrap().as_bytes()).map_err(|e| format!("Couldn't save the vault: {e}"))
}

fn wrap_with_pin(s: &Store, pin: &str, key: &[u8; 32]) -> R<(String, String)> {
    let mut salt = [0u8; 16];
    OsRng.fill_bytes(&mut salt);
    let kek = derive_pin(pin, &salt, s.m_kib, s.t)?;
    Ok((B64.encode(salt), B64.encode(seal(&kek, &key[..], AAD_PIN))))
}

/// Create a new vault. Returns it unlocked, and the 24 recovery words (shown once).
pub fn setup(s: &Store, pin: &str) -> R<(Open, String)> {
    check_pin(pin)?;
    if s.exists() {
        return Err("A vault already exists.".into());
    }
    mkdir_private(&s.root).map_err(|e| format!("Couldn't create the vault folder: {e}"))?;
    mkdir_private(&s.blobs()).map_err(|e| format!("Couldn't create the vault folder: {e}"))?;
    let mut key = Zeroizing::new([0u8; 32]);
    OsRng.fill_bytes(&mut key[..]);
    let mut entropy = Zeroizing::new([0u8; 32]);
    OsRng.fill_bytes(&mut entropy[..]);
    let words = bip39::Mnemonic::from_entropy(&entropy[..]).map_err(|e| e.to_string())?.to_string();
    let (salt, wrap_pin) = wrap_with_pin(s, pin, &key)?;
    let wrap_rec = B64.encode(seal(&rec_kek(&entropy[..]), &key[..], AAD_REC));
    let h = Header { version: 1, kdf: "argon2id".into(), m_kib: s.m_kib, t: s.t, p: 1, salt, wrap_pin, wrap_rec, created: now() };
    let open = Open { store: s.clone(), key, index: Index { version: 1, nodes: BTreeMap::new() }, tmp: vec![], tmp_dir: None };
    open.save_index()?;
    write_header(s, &h)?;
    clear_failures(s);
    Ok((open, words))
}

fn load_index(s: &Store, key: &[u8; 32]) -> R<Index> {
    let b = match fs::read(s.index()) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Index { version: 1, nodes: BTreeMap::new() }),
        Err(e) => return Err(format!("Couldn't read the vault: {e}")),
    };
    if b.len() < 8 || &b[..8] != IDX_MAGIC {
        return Err("The vault's index is damaged.".into());
    }
    let pt = unseal(key, &b[8..], IDX_MAGIC).ok_or("The vault's index is damaged.")?;
    serde_json::from_slice(&pt).map_err(|_| "The vault's index is damaged.".to_string())
}

fn finish_unlock(s: &Store, key: Key) -> R<Open> {
    let index = load_index(s, &key)?;
    let mut o = Open { store: s.clone(), key, index, tmp: vec![], tmp_dir: None };
    o.remove_orphans();
    Ok(o)
}

pub fn unlock(s: &Store, pin: &str) -> R<Open> {
    let a = read_attempts(s);
    let wait = a.until - now();
    if wait > 0 {
        return Err(format!("WAIT:{wait}"));
    }
    let h = read_header(s)?;
    let salt = B64.decode(&h.salt).map_err(|_| "The vault's key file is damaged.")?;
    let wrapped = B64.decode(&h.wrap_pin).map_err(|_| "The vault's key file is damaged.")?;
    let kek = derive_pin(pin, &salt, h.m_kib, h.t)?;
    let Some(k) = unseal(&kek, &wrapped, AAD_PIN) else {
        let d = record_failure(s);
        return Err(if d > 0 { format!("WRONG:{d}") } else { "WRONG:0".into() });
    };
    clear_failures(s);
    let mut key = Zeroizing::new([0u8; 32]);
    key.copy_from_slice(&k);
    finish_unlock(s, key)
}

/// Open with the recovery words and set a new PIN.
pub fn recover(s: &Store, words: &str, new_pin: &str) -> R<Open> {
    check_pin(new_pin)?;
    let e = parse_recovery(words)?;
    let mut h = read_header(s)?;
    let wrapped = B64.decode(&h.wrap_rec).map_err(|_| "The vault's key file is damaged.")?;
    let k = unseal(&rec_kek(&e), &wrapped, AAD_REC).ok_or("That recovery key doesn't belong to this vault.")?;
    let mut key = Zeroizing::new([0u8; 32]);
    key.copy_from_slice(&k);
    let st = Store { m_kib: h.m_kib, t: h.t, ..s.clone() };
    let (salt, wrap_pin) = wrap_with_pin(&st, new_pin, &key)?;
    h.salt = salt;
    h.wrap_pin = wrap_pin;
    write_header(s, &h)?;
    clear_failures(s);
    finish_unlock(s, key)
}

impl Open {
    pub fn change_pin(&self, new_pin: &str) -> R<()> {
        check_pin(new_pin)?;
        let mut h = read_header(&self.store)?;
        let (salt, wrap_pin) = wrap_with_pin(&self.store, new_pin, &self.key)?;
        h.salt = salt;
        h.wrap_pin = wrap_pin;
        h.m_kib = self.store.m_kib;
        h.t = self.store.t;
        write_header(&self.store, &h)
    }

    pub fn save_index(&self) -> R<()> {
        let json = Zeroizing::new(serde_json::to_vec(&self.index).unwrap());
        let mut out = IDX_MAGIC.to_vec();
        out.extend(seal(&self.key, &json, IDX_MAGIC));
        write_private(&self.store.index(), &out).map_err(|e| format!("Couldn't save the vault: {e}"))
    }

    /// Blobs left behind by an interrupted job, and stray .part files.
    fn remove_orphans(&mut self) {
        let used: std::collections::HashSet<&str> = self.index.nodes.values().filter(|n| !n.dir).map(|n| n.blob.as_str()).collect();
        if let Ok(rd) = fs::read_dir(self.store.blobs()) {
            for de in rd.flatten() {
                let n = de.file_name().to_string_lossy().into_owned();
                if !used.contains(n.as_str()) {
                    let _ = fs::remove_file(de.path());
                }
            }
        }
    }

    pub fn exists(&self, rel: &str) -> bool {
        rel.is_empty() || self.index.nodes.contains_key(rel)
    }

    pub fn node(&self, rel: &str) -> R<&Node> {
        self.index.nodes.get(rel).ok_or_else(|| format!("\"{}\" isn't in the vault any more.", name(rel)))
    }

    fn is_dir(&self, rel: &str) -> bool {
        rel.is_empty() || self.index.nodes.get(rel).map(|n| n.dir).unwrap_or(false)
    }

    fn children(&self, dir: &str) -> impl Iterator<Item = (&String, &Node)> {
        let d = dir.to_string();
        self.index.nodes.iter().filter(move |(k, _)| parent(k) == d && k.as_str() != d)
    }

    pub fn entry(&self, rel: &str) -> R<Entry> {
        if rel.is_empty() {
            return Ok(Entry {
                name: "Vault".into(),
                path: vpath(""),
                dir: true,
                link: false,
                hidden: false,
                size: 0,
                modified: 0,
                kind: "dir",
                ext: String::new(),
                mode: 0o700,
                items: Some(self.children("").count() as u32),
            });
        }
        let n = self.node(rel)?;
        let nm = name(rel).to_string();
        Ok(Entry {
            hidden: nm.starts_with('.'),
            ext: if n.dir { String::new() } else { fsops::ext_of(&nm) },
            kind: kind_of(&nm, n.dir),
            name: nm,
            path: vpath(rel),
            dir: n.dir,
            link: false,
            size: if n.dir { 0 } else { n.size },
            modified: n.modified,
            mode: if n.dir { 0o700 } else { 0o600 },
            items: if n.dir { Some(self.children(rel).count() as u32) } else { None },
        })
    }

    pub fn list(&self, dir: &str, hidden: bool) -> R<Listing> {
        if !self.is_dir(dir) {
            return Err(format!("\"{}\" isn't a folder in the vault.", name(dir)));
        }
        let mut entries: Vec<Entry> = self
            .children(dir)
            .filter(|(k, _)| hidden || !name(k).starts_with('.'))
            .filter_map(|(k, _)| self.entry(k).ok())
            .collect();
        entries.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| natural_cmp(&a.name, &b.name)));
        Ok(Listing {
            path: vpath(dir),
            parent: if dir.is_empty() { None } else { Some(vpath(parent(dir))) },
            entries,
            writable: true,
        })
    }

    /// Total plaintext bytes and item count under `rel`.
    pub fn measure(&self, rel: &str) -> (u64, u64) {
        let mut b = 0;
        let mut n = 0;
        for (k, v) in &self.index.nodes {
            if k == rel || under(k, rel) || rel.is_empty() {
                b += v.size;
                n += 1;
            }
        }
        (b, n.max(1))
    }

    pub fn unique(&self, dir: &str, nm: &str) -> String {
        let first = join(dir, nm);
        if !self.exists(&first) {
            return first;
        }
        let (stem, ext) = match nm.rfind('.') {
            Some(i) if i > 0 => (&nm[..i], &nm[i..]),
            _ => (nm, ""),
        };
        let mut n = 2;
        loop {
            let c = join(dir, &format!("{stem} ({n}){ext}"));
            if !self.exists(&c) {
                return c;
            }
            n += 1;
        }
    }

    fn ensure_dir_parent(&self, rel: &str) -> R<()> {
        if !self.is_dir(parent(rel)) {
            return Err("That folder isn't in the vault any more.".into());
        }
        Ok(())
    }

    pub fn mkdir(&mut self, dir: &str, nm: &str) -> R<String> {
        fsops::check_name(nm)?;
        let rel = join(dir, nm.trim());
        self.ensure_dir_parent(&rel)?;
        if self.exists(&rel) {
            return Err(format!("\"{}\" already exists here.", nm.trim()));
        }
        self.index.nodes.insert(rel.clone(), Node { dir: true, blob: String::new(), size: 0, modified: now() });
        self.save_index()?;
        Ok(vpath(&rel))
    }

    pub fn mkfile(&mut self, dir: &str, nm: &str) -> R<String> {
        fsops::check_name(nm)?;
        let rel = join(dir, nm.trim());
        self.ensure_dir_parent(&rel)?;
        if self.exists(&rel) {
            return Err(format!("\"{}\" already exists here.", nm.trim()));
        }
        let (id, _) = encrypt_reader(&self.key, &self.store.blobs(), &mut io::empty(), &mut |_| Ok(())).map_err(|e| e.to_string())?;
        self.index.nodes.insert(rel.clone(), Node { dir: false, blob: id, size: 0, modified: now() });
        self.save_index()?;
        Ok(vpath(&rel))
    }

    /// Move `from` (file or folder, with everything under it) to `to`.
    fn relocate(&mut self, from: &str, to: &str) {
        let keys: Vec<String> = self.index.nodes.keys().filter(|k| *k == from || under(k, from)).cloned().collect();
        for k in keys {
            let n = self.index.nodes.remove(&k).unwrap();
            let nk = format!("{to}{}", &k[from.len()..]);
            self.index.nodes.insert(nk, n);
        }
    }

    pub fn rename(&mut self, rel: &str, new_name: &str) -> R<String> {
        fsops::check_name(new_name)?;
        self.node(rel)?;
        let to = join(parent(rel), new_name.trim());
        if to == rel {
            return Ok(vpath(rel));
        }
        if self.exists(&to) {
            return Err(format!("\"{}\" already exists here.", new_name.trim()));
        }
        self.relocate(rel, &to);
        self.save_index()?;
        Ok(vpath(&to))
    }

    /// Delete for good: removes the entries and their encrypted blobs.
    pub fn delete(&mut self, rels: &[String]) -> R<usize> {
        let mut n = 0;
        for rel in rels {
            if rel.is_empty() {
                return Err("The vault itself can't be deleted from here.".into());
            }
            let keys: Vec<String> = self.index.nodes.keys().filter(|k| *k == rel || under(k, rel)).cloned().collect();
            for k in keys {
                if let Some(node) = self.index.nodes.remove(&k) {
                    if !node.dir {
                        let _ = fs::remove_file(self.store.blobs().join(&node.blob));
                    }
                    n += 1;
                }
            }
        }
        self.save_index()?;
        Ok(n)
    }

    /// Decrypt a whole file into memory (previews), up to `max` bytes.
    pub fn read_all(&self, rel: &str, max: u64) -> R<Zeroizing<Vec<u8>>> {
        let n = self.node(rel)?;
        if n.dir {
            return Err("That's a folder.".into());
        }
        if n.size > max {
            return Err("TOO_BIG".into());
        }
        read_blob(&self.key, &self.store.blobs(), &n.blob)
    }

    pub fn find(&self, dir: &str, query: &str, hidden: bool, limit: usize) -> Vec<Entry> {
        let terms: Vec<String> = query.to_lowercase().split_whitespace().map(String::from).collect();
        if terms.is_empty() {
            return vec![];
        }
        let mut hits: Vec<(usize, Entry)> = vec![];
        for k in self.index.nodes.keys() {
            if !(dir.is_empty() || under(k, dir)) {
                continue;
            }
            if !hidden && k.split('/').any(|p| p.starts_with('.')) {
                continue;
            }
            let ln = name(k).to_lowercase();
            if terms.iter().all(|t| ln.contains(t.as_str())) {
                if let Ok(e) = self.entry(k) {
                    hits.push((k.matches('/').count(), e));
                }
            }
        }
        hits.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| natural_cmp(&a.1.name, &b.1.name)));
        hits.into_iter().take(limit).map(|(_, e)| e).collect()
    }

    pub fn tree(&self, dir: &str, depth: u32, hidden: bool, max_nodes: usize) -> R<crate::search::Graph> {
        use crate::search::{GEdge, GNode, Graph};
        if !self.is_dir(dir) {
            return Err("That isn't a folder in the vault.".into());
        }
        let base_depth = if dir.is_empty() { 0 } else { dir.matches('/').count() as u32 + 1 };
        let mut nodes = vec![GNode {
            id: vpath(dir),
            name: if dir.is_empty() { "Vault".into() } else { name(dir).into() },
            kind: "dir",
            dir: true,
            size: 0,
            depth: 0,
            children: self.children(dir).count() as u32,
            more: false,
        }];
        let mut edges = vec![];
        let mut truncated = false;
        let mut all: Vec<(&String, &Node)> = self
            .index
            .nodes
            .iter()
            .filter(|(k, _)| dir.is_empty() || under(k, dir))
            .filter(|(k, _)| hidden || !k.split('/').any(|p| p.starts_with('.')))
            .collect();
        all.sort_by_key(|(k, _)| (k.matches('/').count(), (*k).clone()));
        for (k, n) in all {
            let d = k.matches('/').count() as u32 + 1 - base_depth;
            if d > depth {
                if let Some(p) = nodes.iter_mut().find(|g| g.id == vpath(parent(k))) {
                    p.more = true;
                }
                continue;
            }
            if nodes.len() >= max_nodes {
                truncated = true;
                if let Some(p) = nodes.iter_mut().find(|g| g.id == vpath(parent(k))) {
                    p.more = true;
                }
                continue;
            }
            let nm = name(k);
            edges.push(GEdge { source: vpath(parent(k)), target: vpath(k) });
            nodes.push(GNode {
                id: vpath(k),
                name: nm.into(),
                kind: kind_of(nm, n.dir),
                dir: n.dir,
                size: n.size,
                depth: d,
                children: if n.dir { self.children(k).count() as u32 } else { 0 },
                more: false,
            });
        }
        Ok(Graph { root: vpath(dir), nodes, edges, truncated })
    }

    /* ---------- opening in other apps ---------- */

    fn private_tmp(&mut self) -> R<PathBuf> {
        if let Some(d) = &self.tmp_dir {
            return Ok(d.clone());
        }
        let run = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .ok_or("There's no private RAM folder ($XDG_RUNTIME_DIR), so vault files can't be opened in other apps safely.")?;
        let d = run.join(format!("null-file-vault-{}", random_id()));
        mkdir_private(&d).map_err(|e| format!("Couldn't prepare a private folder: {e}"))?;
        self.tmp_dir = Some(d.clone());
        Ok(d)
    }

    /// Decrypt `rel` into RAM-backed storage for another app. Returns the path.
    pub fn open_copy(&mut self, rel: &str) -> R<PathBuf> {
        if let Some(t) = self.tmp.iter().find(|t| t.rel == rel && t.path.exists()) {
            return Ok(t.path.clone());
        }
        let n = self.node(rel)?.clone();
        if n.dir {
            return Err("Folders in the vault can't be opened in other apps.".into());
        }
        let dir = self.private_tmp()?.join(random_id());
        mkdir_private(&dir).map_err(|e| e.to_string())?;
        let p = dir.join(name(rel));
        decrypt_to_file(&self.key, &self.store.blobs(), &n.blob, &p, 0o600, &mut |_| Ok(())).map_err(|e| format!("Couldn't open it: {e}"))?;
        let md = fs::metadata(&p).map_err(|e| e.to_string())?;
        self.tmp.push(TmpFile { rel: rel.into(), path: p.clone(), modified: md.modified().unwrap_or(UNIX_EPOCH), len: md.len() });
        Ok(p)
    }

    /// Save edits made in other apps back into the vault. Returns how many files changed.
    pub fn sync_back(&mut self) -> R<usize> {
        let mut changed = 0;
        let mut i = 0;
        while i < self.tmp.len() {
            let (rel, p, m0, l0) = {
                let t = &self.tmp[i];
                (t.rel.clone(), t.path.clone(), t.modified, t.len)
            };
            let Ok(md) = fs::metadata(&p) else {
                i += 1;
                continue;
            };
            let m = md.modified().unwrap_or(UNIX_EPOCH);
            if (m != m0 || md.len() != l0) && self.index.nodes.get(&rel).map(|n| !n.dir).unwrap_or(false) {
                let (id, size) = encrypt_file(&self.key, &self.store.blobs(), &p, &mut |_| Ok(())).map_err(|e| format!("Couldn't save \"{}\" back: {e}", name(&rel)))?;
                let node = self.index.nodes.get_mut(&rel).unwrap();
                let old = std::mem::replace(&mut node.blob, id);
                node.size = size;
                node.modified = now();
                let _ = fs::remove_file(self.store.blobs().join(old));
                self.tmp[i].modified = m;
                self.tmp[i].len = md.len();
                changed += 1;
            }
            i += 1;
        }
        if changed > 0 {
            self.save_index()?;
        }
        Ok(changed)
    }

    /// Save edits back, then erase every decrypted copy.
    pub fn close(mut self) -> R<usize> {
        let r = self.sync_back();
        if let Some(d) = self.tmp_dir.take() {
            wipe_dir(&d);
        }
        r
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        if let Some(d) = self.tmp_dir.take() {
            wipe_dir(&d);
        }
    }
}

/// Overwrite files with zeros before removing them (they live in tmpfs, but
/// this keeps the plaintext out of any swap or page that outlives the unlink).
fn wipe_dir(d: &Path) {
    fn walk(p: &Path) {
        if let Ok(rd) = fs::read_dir(p) {
            for de in rd.flatten() {
                let q = de.path();
                if q.is_dir() {
                    walk(&q);
                } else if let Ok(md) = fs::metadata(&q) {
                    if let Ok(mut f) = fs::OpenOptions::new().write(true).open(&q) {
                        let zero = vec![0u8; 64 * 1024];
                        let mut left = md.len();
                        while left > 0 {
                            let k = left.min(zero.len() as u64) as usize;
                            if f.write_all(&zero[..k]).is_err() {
                                break;
                            }
                            left -= k as u64;
                        }
                        let _ = f.sync_all();
                    }
                }
            }
        }
    }
    walk(d);
    let _ = fs::remove_dir_all(d);
}

pub fn read_blob(key: &[u8; 32], blobs: &Path, id: &str) -> R<Zeroizing<Vec<u8>>> {
    let mut r = io::BufReader::new(fs::File::open(blobs.join(id)).map_err(|e| format!("Couldn't read it: {e}"))?);
    let mut out = Zeroizing::new(Vec::new());
    decrypt_stream(key, id, &mut r, &mut *out, &mut |_| Ok(())).map_err(|e| format!("Couldn't read it: {e}"))?;
    Ok(out)
}

/* ---------------- copy / move jobs that touch the vault ---------------- */

pub enum Side {
    Real(PathBuf),
    Vault(String),
}

impl Side {
    pub fn parse(p: &str) -> R<Side> {
        if is_vault(p) {
            Ok(Side::Vault(rel(p)?))
        } else {
            Ok(Side::Real(fsops::abs(p)?))
        }
    }
}

pub struct JobCtx<'a> {
    pub cancel: &'a AtomicBool,
    pub on: &'a mut dyn FnMut(u64, bool, &str),
}

fn locked() -> String {
    "The vault was locked, so the rest was stopped.".into()
}

fn check_cancel(c: &AtomicBool) -> io::Result<()> {
    if c.load(Ordering::Relaxed) {
        Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"))
    } else {
        Ok(())
    }
}

/// Grab the key and blob folder without holding the lock during long crypto.
fn key_and_blobs(v: &Shared) -> R<(Key, PathBuf)> {
    let g = v.lock().unwrap();
    let o = g.as_ref().ok_or_else(locked)?;
    Ok((o.key.clone(), o.store.blobs()))
}

pub fn measure(v: &Shared, s: &Side) -> (u64, u64) {
    match s {
        Side::Real(p) => fsops::measure(p),
        Side::Vault(r) => v.lock().unwrap().as_ref().map(|o| o.measure(r)).unwrap_or((0, 1)),
    }
}

/// Where to put `nm` in vault folder `dir` under the conflict rule. None = skip.
fn vault_target(v: &Shared, dir: &str, nm: &str, conflict: &str) -> R<Option<String>> {
    let mut g = v.lock().unwrap();
    let o = g.as_mut().ok_or_else(locked)?;
    if !o.is_dir(dir) {
        return Err("The vault folder isn't there any more.".into());
    }
    let want = join(dir, nm);
    if !o.exists(&want) {
        return Ok(Some(want));
    }
    match conflict {
        "skip" => Ok(None),
        "replace" => {
            o.delete(std::slice::from_ref(&want))?;
            Ok(Some(want))
        }
        _ => Ok(Some(o.unique(dir, nm))),
    }
}

fn insert(v: &Shared, rel: &str, node: Node, dirty: &mut Instant) -> R<()> {
    let mut g = v.lock().unwrap();
    let o = g.as_mut().ok_or_else(locked)?;
    o.index.nodes.insert(rel.to_string(), node);
    if dirty.elapsed().as_secs() >= 3 {
        o.save_index()?;
        *dirty = Instant::now();
    }
    Ok(())
}

fn real_mtime(p: &Path) -> i64 {
    fs::metadata(p).ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or_else(now)
}

/// Real file or folder → vault path `dst` (already conflict-free).
fn import(v: &Shared, src: &Path, dst: &str, cx: &mut JobCtx, saved: &mut Instant) -> R<()> {
    check_cancel(cx.cancel).map_err(|_| "cancelled".to_string())?;
    let md = fs::symlink_metadata(src).map_err(|e| fsops::err(e, "Can't read", src))?;
    if md.file_type().is_symlink() {
        // Store what a link points to only if it's a file; never follow folder links.
        match fs::metadata(src) {
            Ok(m) if m.is_file() => {}
            _ => {
                (cx.on)(0, true, &name_of(src));
                return Ok(());
            }
        }
    }
    if fs::metadata(src).map(|m| m.is_dir()).unwrap_or(false) {
        insert(v, dst, Node { dir: true, blob: String::new(), size: 0, modified: real_mtime(src) }, saved)?;
        let mut kids: Vec<_> = fs::read_dir(src).map_err(|e| fsops::err(e, "Can't open folder", src))?.flatten().collect();
        kids.sort_by_key(|d| d.file_name());
        for de in kids {
            let n = de.file_name().to_string_lossy().into_owned();
            import(v, &de.path(), &join(dst, &n), cx, saved)?;
        }
        (cx.on)(0, true, &name_of(src));
        return Ok(());
    }
    let (key, blobs) = key_and_blobs(v)?;
    let label = name_of(src);
    let cancel = cx.cancel;
    let on = &mut cx.on;
    let (id, size) = encrypt_file(&key, &blobs, src, &mut |b| {
        on(b, false, &label);
        check_cancel(cancel)
    })
    .map_err(|e| if e.kind() == io::ErrorKind::Interrupted { "cancelled".to_string() } else { fsops::err(e, "Can't add to vault", src) })?;
    if let Err(e) = insert(v, dst, Node { dir: false, blob: id.clone(), size, modified: real_mtime(src) }, saved) {
        let _ = fs::remove_file(blobs.join(&id));
        return Err(e);
    }
    (cx.on)(0, true, &label);
    Ok(())
}

/// Vault path `src` → real path `dst` (which must not exist yet).
fn export(v: &Shared, src: &str, dst: &Path, cx: &mut JobCtx) -> R<()> {
    check_cancel(cx.cancel).map_err(|_| "cancelled".to_string())?;
    let (node, kids) = {
        let g = v.lock().unwrap();
        let o = g.as_ref().ok_or_else(locked)?;
        let n = o.node(src)?.clone();
        let kids: Vec<String> = if n.dir { o.children(src).map(|(k, _)| k.clone()).collect() } else { vec![] };
        (n, kids)
    };
    let label = name(src).to_string();
    if node.dir {
        fs::create_dir(dst).map_err(|e| fsops::err(e, "Can't create folder", dst))?;
        for k in kids {
            export(v, &k, &dst.join(name(&k)), cx)?;
        }
        (cx.on)(0, true, &label);
        return Ok(());
    }
    let (key, blobs) = key_and_blobs(v)?;
    let cancel = cx.cancel;
    let on = &mut cx.on;
    decrypt_to_file(&key, &blobs, &node.blob, dst, 0o644, &mut |b| {
        on(b, false, &label);
        check_cancel(cancel)
    })
    .map_err(|e| if e.kind() == io::ErrorKind::Interrupted { "cancelled".to_string() } else { fsops::err(e, "Can't take out of vault", dst) })?;
    let _ = filetime::set_file_mtime(dst, filetime::FileTime::from_unix_time(node.modified, 0));
    (cx.on)(0, true, &label);
    Ok(())
}

/// Copy inside the vault: decrypt and re-encrypt each file into a fresh blob.
fn copy_inside(v: &Shared, src: &str, dst: &str, cx: &mut JobCtx, saved: &mut Instant) -> R<()> {
    check_cancel(cx.cancel).map_err(|_| "cancelled".to_string())?;
    let (node, kids) = {
        let g = v.lock().unwrap();
        let o = g.as_ref().ok_or_else(locked)?;
        let n = o.node(src)?.clone();
        let kids: Vec<String> = if n.dir { o.children(src).map(|(k, _)| k.clone()).collect() } else { vec![] };
        (n, kids)
    };
    let label = name(src).to_string();
    if node.dir {
        insert(v, dst, Node { dir: true, blob: String::new(), size: 0, modified: now() }, saved)?;
        for k in kids {
            copy_inside(v, &k, &join(dst, name(&k)), cx, saved)?;
        }
        (cx.on)(0, true, &label);
        return Ok(());
    }
    let (key, blobs) = key_and_blobs(v)?;
    // Small and medium files go through memory; that keeps plaintext off the disk.
    let plain = read_blob(&key, &blobs, &node.blob)?;
    let (id, size) = encrypt_reader(&key, &blobs, &mut io::Cursor::new(&plain[..]), &mut |b| {
        (cx.on)(b, false, &label);
        check_cancel(cx.cancel)
    })
    .map_err(|e| if e.kind() == io::ErrorKind::Interrupted { "cancelled".to_string() } else { e.to_string() })?;
    if let Err(e) = insert(v, dst, Node { dir: false, blob: id.clone(), size, modified: node.modified }, saved) {
        let _ = fs::remove_file(blobs.join(&id));
        return Err(e);
    }
    (cx.on)(0, true, &label);
    Ok(())
}

fn name_of(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Remove a real file or folder for good (after it was moved into the vault).
fn remove_real(p: &Path) -> io::Result<()> {
    let md = fs::symlink_metadata(p)?;
    if md.is_dir() {
        fs::remove_dir_all(p)
    } else {
        fs::remove_file(p)
    }
}

/// A copy or move where the source or the destination is in the vault.
/// Returns the paths created (UI paths) and any errors.
pub fn transfer(v: &Shared, is_move: bool, srcs: &[String], dest: &str, conflict: &str, cx: &mut JobCtx) -> (Vec<String>, Vec<String>) {
    let mut created = vec![];
    let mut errors = vec![];
    let mut saved = Instant::now();
    let dest_side = match Side::parse(dest) {
        Ok(d) => d,
        Err(e) => return (created, vec![e]),
    };
    for s in srcs {
        if cx.cancel.load(Ordering::Relaxed) {
            break;
        }
        let src = match Side::parse(s) {
            Ok(x) => x,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        let res: R<Option<String>> = (|| match (&src, &dest_side) {
            (Side::Real(sp), Side::Vault(dd)) => {
                let Some(target) = vault_target(v, dd, &name_of(sp), conflict)? else { return Ok(None) };
                import(v, sp, &target, cx, &mut saved)?;
                {
                    let g = v.lock().unwrap();
                    g.as_ref().ok_or_else(locked)?.save_index()?;
                }
                if is_move {
                    remove_real(sp).map_err(|e| fsops::err(e, "Added to the vault, but couldn't remove the original", sp))?;
                }
                Ok(Some(vpath(&target)))
            }
            (Side::Vault(sr), Side::Real(dd)) => {
                if sr.is_empty() {
                    return Err("The whole vault can't be moved out at once.".into());
                }
                let nm = name(sr).to_string();
                let mut target = dd.join(&nm);
                if fs::symlink_metadata(&target).is_ok() {
                    match conflict {
                        "skip" => return Ok(None),
                        "replace" => fsops::trash_paths(&[target.clone()])?,
                        _ => target = fsops::unique_in(dd, &nm),
                    }
                }
                export(v, sr, &target, cx)?;
                if is_move {
                    let mut g = v.lock().unwrap();
                    g.as_mut().ok_or_else(locked)?.delete(std::slice::from_ref(sr))?;
                }
                Ok(Some(target.display().to_string()))
            }
            (Side::Vault(sr), Side::Vault(dd)) => {
                if sr.is_empty() {
                    return Err("The vault can't be copied into itself.".into());
                }
                if dd == sr || under(dd, sr) {
                    return Err(format!("Can't put \"{}\" inside itself.", name(sr)));
                }
                if is_move && parent(sr) == dd {
                    return Ok(None);
                }
                let Some(target) = vault_target(v, dd, name(sr), conflict)? else { return Ok(None) };
                if is_move {
                    let mut g = v.lock().unwrap();
                    let o = g.as_mut().ok_or_else(locked)?;
                    o.relocate(sr, &target);
                    o.save_index()?;
                    let (b, _) = o.measure(&target);
                    drop(g);
                    (cx.on)(b, true, name(sr));
                } else {
                    copy_inside(v, sr, &target, cx, &mut saved)?;
                    let g = v.lock().unwrap();
                    g.as_ref().ok_or_else(locked)?.save_index()?;
                }
                Ok(Some(vpath(&target)))
            }
            (Side::Real(_), Side::Real(_)) => Err("Not a vault transfer.".into()),
        })();
        match res {
            Ok(Some(p)) => created.push(p),
            Ok(None) => {}
            Err(e) if e == "cancelled" => break,
            Err(e) => errors.push(e),
        }
    }
    if let Some(o) = v.lock().unwrap().as_ref() {
        if let Err(e) = o.save_index() {
            errors.push(e);
        }
    }
    (created, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(t: &tempfile::TempDir) -> Store {
        Store { root: t.path().join("vault"), m_kib: 64, t: 1 }
    }

    fn shared(o: Open) -> Shared {
        Arc::new(Mutex::new(Some(o)))
    }

    fn job<'a>(c: &'a AtomicBool, on: &'a mut dyn FnMut(u64, bool, &str)) -> JobCtx<'a> {
        JobCtx { cancel: c, on }
    }

    #[test]
    fn pins_are_digits_3_to_300() {
        assert!(check_pin("123").is_ok());
        assert!(check_pin(&"9".repeat(300)).is_ok());
        assert!(check_pin("12").is_err());
        assert!(check_pin(&"9".repeat(301)).is_err());
        assert!(check_pin("12a4").is_err());
        assert!(check_pin("12 34").is_err());
    }

    #[test]
    fn rel_paths() {
        assert_eq!(rel("vault:/").unwrap(), "");
        assert_eq!(rel("vault:/a/b/").unwrap(), "a/b");
        assert!(rel("vault:/a/../b").is_err());
        assert!(is_vault("vault:/x") && !is_vault("/home/x"));
        assert!(under("a/b", "a") && !under("ab", "a") && !under("a", "a"));
    }

    #[test]
    fn stream_roundtrip_at_chunk_edges() {
        let key = [7u8; 32];
        for len in [0usize, 1, CHUNK - 1, CHUNK, CHUNK + 1, 3 * CHUNK + 5] {
            let data: Vec<u8> = (0..len).map(|i| (i * 31 % 251) as u8).collect();
            let mut enc = vec![];
            let n = encrypt_stream(&key, "id1", &mut &data[..], &mut enc, &mut |_| Ok(())).unwrap();
            assert_eq!(n, len as u64);
            let mut dec = vec![];
            decrypt_stream(&key, "id1", &mut &enc[..], &mut dec, &mut |_| Ok(())).unwrap();
            assert_eq!(dec, data, "len {len}");
            // wrong id (swapped blob) fails
            assert!(decrypt_stream(&key, "id2", &mut &enc[..], &mut vec![], &mut |_| Ok(())).is_err());
            // truncation at a chunk boundary fails (last-chunk flag)
            if len > CHUNK {
                let cut = &enc[..8 + STREAM_NONCE + CHUNK + TAG];
                assert!(decrypt_stream(&key, "id1", &mut &cut[..], &mut vec![], &mut |_| Ok(())).is_err());
            }
        }
    }

    #[test]
    fn setup_unlock_wrong_pin_and_backoff() {
        let t = tempfile::tempdir().unwrap();
        let s = store(&t);
        let (o, words) = setup(&s, "4821").unwrap();
        assert_eq!(words.split(' ').count(), 24);
        drop(o);
        assert!(setup(&s, "4821").is_err());
        assert!(unlock(&s, "4821").is_ok());
        assert_eq!(unlock(&s, "0000").err().unwrap(), "WRONG:0");
        assert_eq!(unlock(&s, "0000").err().unwrap(), "WRONG:0");
        assert_eq!(unlock(&s, "0000").err().unwrap(), "WRONG:5");
        let e = unlock(&s, "4821").err().unwrap();
        assert!(e.starts_with("WAIT:"), "{e}");
        assert_eq!(status(&s, false).failures, 3);
        // clear the wait and the right PIN works and resets the counter
        let _ = fs::remove_file(s.attempts());
        assert!(unlock(&s, "4821").is_ok());
        assert_eq!(status(&s, false).failures, 0);
        assert_eq!(delay_for(4), 10);
        assert_eq!(delay_for(40), 900);
    }

    #[test]
    fn recovery_and_change_pin() {
        let t = tempfile::tempdir().unwrap();
        let s = store(&t);
        let (mut o, words) = setup(&s, "111").unwrap();
        o.mkdir("", "Taxes").unwrap();
        drop(o);
        assert!(recover(&s, "abandon ".repeat(23).trim(), "222").is_err());
        let shouty = words.to_uppercase().replace(' ', "   ");
        let o = recover(&s, &shouty, "222222").unwrap();
        assert!(o.exists("Taxes"));
        drop(o);
        assert!(unlock(&s, "111").is_err());
        let _ = fs::remove_file(s.attempts());
        let o = unlock(&s, "222222").unwrap();
        o.change_pin("98765").unwrap();
        drop(o);
        assert!(unlock(&s, "98765").is_ok());
        // the recovery key still works after a PIN change
        assert!(recover(&s, &words, "333").is_ok());
    }

    #[test]
    fn nothing_readable_on_disk() {
        let t = tempfile::tempdir().unwrap();
        let s = store(&t);
        let src = t.path().join("bank-statement-secret.txt");
        fs::write(&src, "ACCOUNT 1234 balance hello-plaintext").unwrap();
        let (o, _) = setup(&s, "5555").unwrap();
        let v = shared(o);
        let c = AtomicBool::new(false);
        let mut on = |_: u64, _: bool, _: &str| {};
        let (made, errs) = transfer(&v, false, &[src.display().to_string()], "vault:/", "keep-both", &mut job(&c, &mut on));
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(made, vec!["vault:/bank-statement-secret.txt"]);
        fn scan(p: &Path, out: &mut Vec<u8>) {
            for de in fs::read_dir(p).unwrap().flatten() {
                let q = de.path();
                out.extend(q.file_name().unwrap().to_string_lossy().as_bytes());
                if q.is_dir() { scan(&q, out) } else { out.extend(fs::read(&q).unwrap()) }
            }
        }
        let mut all = vec![];
        scan(&s.root, &mut all);
        let hay = String::from_utf8_lossy(&all);
        assert!(!hay.contains("bank-statement"));
        assert!(!hay.contains("hello-plaintext"));
        assert!(!hay.contains("ACCOUNT"));
    }

    #[test]
    fn import_export_tree_and_move_semantics() {
        let t = tempfile::tempdir().unwrap();
        let s = store(&t);
        let real = t.path().join("Photos");
        fs::create_dir_all(real.join("2026/Trip")).unwrap();
        fs::write(real.join("2026/Trip/a.jpg"), vec![1u8; CHUNK + 10]).unwrap();
        fs::write(real.join("notes.md"), "# hi").unwrap();
        let (o, _) = setup(&s, "1234").unwrap();
        let v = shared(o);
        let c = AtomicBool::new(false);
        let mut bytes = 0u64;
        let mut on = |b: u64, _: bool, _: &str| bytes += b;
        // move in: original is gone afterwards
        let (made, errs) = transfer(&v, true, &[real.display().to_string()], "vault:/", "keep-both", &mut job(&c, &mut on));
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(made, vec!["vault:/Photos"]);
        assert!(!real.exists());
        assert_eq!(bytes, (CHUNK + 10 + 4) as u64);
        {
            let g = v.lock().unwrap();
            let o = g.as_ref().unwrap();
            let l = o.list("Photos", false).unwrap();
            assert_eq!(l.entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["2026", "notes.md"]);
            assert_eq!(o.list("", false).unwrap().entries[0].items, Some(2));
            assert_eq!(o.find("", "A.JPG", false, 10).len(), 1);
            let g = o.tree("", 9, false, 100).unwrap();
            assert_eq!(g.nodes.len(), g.edges.len() + 1);
            assert_eq!(&*o.read_all("Photos/notes.md", 100).unwrap(), b"# hi");
            assert_eq!(o.read_all("Photos/2026/Trip/a.jpg", 100).err().unwrap(), "TOO_BIG");
        }
        // copy inside, then move out
        let mut on = |_: u64, _: bool, _: &str| {};
        let (made, _) = transfer(&v, false, &["vault:/Photos".into()], "vault:/", "keep-both", &mut job(&c, &mut on));
        assert_eq!(made, vec!["vault:/Photos (2)"]);
        let out = t.path().join("out");
        fs::create_dir(&out).unwrap();
        let (made, errs) = transfer(&v, true, &["vault:/Photos (2)".into()], &out.display().to_string(), "keep-both", &mut job(&c, &mut on));
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(made, vec![out.join("Photos (2)").display().to_string()]);
        assert_eq!(fs::read(out.join("Photos (2)/2026/Trip/a.jpg")).unwrap(), vec![1u8; CHUNK + 10]);
        let g = v.lock().unwrap();
        let o = g.as_ref().unwrap();
        assert!(!o.exists("Photos (2)"));
        assert!(o.exists("Photos/2026/Trip/a.jpg"));
        // blobs on disk match the files in the index exactly
        let files = o.index.nodes.values().filter(|n| !n.dir).count();
        assert_eq!(fs::read_dir(s.blobs()).unwrap().count(), files);
    }

    #[test]
    fn rename_delete_and_move_within() {
        let t = tempfile::tempdir().unwrap();
        let s = store(&t);
        let (mut o, _) = setup(&s, "777").unwrap();
        o.mkdir("", "A").unwrap();
        o.mkdir("A", "B").unwrap();
        o.mkfile("A/B", "x.txt").unwrap();
        assert!(o.mkdir("", "A").is_err());
        assert!(o.mkdir("Nope", "C").is_err());
        assert_eq!(o.rename("A", "Z").unwrap(), "vault:/Z");
        assert!(o.exists("Z/B/x.txt") && !o.exists("A"));
        let v = shared(o);
        let c = AtomicBool::new(false);
        let mut on = |_: u64, _: bool, _: &str| {};
        let (_, errs) = transfer(&v, true, &["vault:/Z".into()], "vault:/Z/B", "keep-both", &mut job(&c, &mut on));
        assert!(!errs.is_empty()); // into itself
        transfer(&v, true, &["vault:/Z/B/x.txt".into()], "vault:/", "keep-both", &mut job(&c, &mut on));
        let mut g = v.lock().unwrap();
        let o = g.as_mut().unwrap();
        assert!(o.exists("x.txt") && !o.exists("Z/B/x.txt"));
        assert_eq!(o.delete(&["Z".into(), "x.txt".into()]).unwrap(), 3);
        assert!(o.index.nodes.is_empty());
        assert_eq!(fs::read_dir(s.blobs()).unwrap().count(), 0);
        drop(g);
        // the index survives a re-open
        let mut o = unlock(&s, "777").unwrap();
        assert!(o.index.nodes.is_empty());
        o.mkdir("", "Keep").unwrap();
        drop(o);
        assert!(unlock(&s, "777").unwrap().exists("Keep"));
    }

    #[test]
    fn open_copy_syncs_back_and_wipes() {
        let t = tempfile::tempdir().unwrap();
        let run = t.path().join("run");
        fs::create_dir(&run).unwrap();
        std::env::set_var("XDG_RUNTIME_DIR", &run);
        let s = store(&t);
        let (mut o, _) = setup(&s, "4444").unwrap();
        o.mkfile("", "draft.txt").unwrap();
        let p = o.open_copy("draft.txt").unwrap();
        assert!(p.starts_with(&run));
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&p, "edited outside").unwrap();
        assert_eq!(o.close().unwrap(), 1);
        assert_eq!(fs::read_dir(&run).unwrap().count(), 0, "decrypted copies are wiped on lock");
        let o = unlock(&s, "4444").unwrap();
        assert_eq!(&*o.read_all("draft.txt", 100).unwrap(), b"edited outside");
    }

    #[test]
    fn tampered_index_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let s = store(&t);
        let (mut o, _) = setup(&s, "1212").unwrap();
        o.mkdir("", "X").unwrap();
        drop(o);
        let mut b = fs::read(s.index()).unwrap();
        let n = b.len();
        b[n - 3] ^= 1;
        fs::write(s.index(), b).unwrap();
        assert!(unlock(&s, "1212").err().unwrap().contains("damaged"));
    }
}
