//! Argon2id password hashing with a pepper, and the HMAC lookup index for
//! API keys.

use crate::error::{AppError, Result};
use argon2::{
    password_hash::{rand_core::OsRng, Output, ParamsString, PasswordHash, SaltString},
    Algorithm, Argon2, Block, Params, Version,
};
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use zeroize::Zeroize;

pub const PEPPER_SIZE: usize = 32;

/// OWASP baseline for Argon2id. Parameters are stored in the PHC string, so
/// verification of older hashes keeps working if these change.
fn argon2() -> Result<Argon2<'static>> {
    ARGON2_RUNS.with(|n| n.set(n.get() + 1));
    let params = Params::new(19456, 2, 1, None)
        .map_err(|e| AppError::Internal(format!("argon2 params: {}", e)))?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

/// The password with the pepper appended, as the web hashes it. Zeroed when
/// dropped: it holds both secrets in the clear.
fn peppered(pepper: &[u8], password: &str) -> zeroize::Zeroizing<String> {
    let pepper_b64 =
        zeroize::Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(pepper));
    let mut s = zeroize::Zeroizing::new(String::with_capacity(password.len() + pepper_b64.len()));
    s.push_str(password);
    s.push_str(&pepper_b64);
    s
}

/// Largest parameters a stored hash may ask for: 256 MiB, 16 passes and 16
/// lanes (the account's own are 19 MiB, 2 and 1). A hash asking for more is
/// refused before anything is allocated, never run.
const MAX_M_COST_KIB: u32 = 256 * 1024;
const MAX_T_COST: u32 = 16;
const MAX_P_COST: u32 = 16;

/// One Argon2 run's working memory (19 MiB with the account's parameters),
/// zeroed when the run ends. On Linux with glibc it is an anonymous map of
/// its own, unmapped when the run ends: taken from the allocator, a freed
/// block stayed in the thread's malloc arena once the first one had raised
/// glibc's mmap threshold, so every thread that ever hashed held on to up to
/// ~19 MiB, some several (weekly soak, 2026-10-18: +115 MB). Where the map
/// cannot be made, and on macOS and Windows (which return large frees by
/// themselves; an anonymous map is briefly executable on Windows), it is a
/// heap allocation. A sign-in never fails for want of a map.
struct Workspace {
    memory: Memory,
}

enum Memory {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    Mapped(memmap2::MmapMut),
    Heap(Vec<Block>),
}

impl Workspace {
    fn new(params: &Params) -> Result<Self> {
        let blocks = params.block_count();
        let bytes = blocks
            .checked_mul(Block::SIZE)
            .ok_or_else(|| AppError::Internal("argon2 working memory: too large".into()))?;
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        if let Some(map) = map_anon(bytes) {
            return Ok(Self {
                memory: Memory::Mapped(map),
            });
        }
        Ok(Self {
            memory: Memory::Heap(vec![Block::default(); bytes / Block::SIZE]),
        })
    }
}

/// An anonymous map of `bytes`, or None when the system refuses one.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn map_anon(bytes: usize) -> Option<memmap2::MmapMut> {
    #[cfg(test)]
    if tests::MAP_FAILS.with(|f| f.get()) {
        return None;
    }
    match memmap2::MmapMut::map_anon(bytes) {
        Ok(map) => Some(map),
        Err(e) => {
            tracing::warn!("Argon2 working memory not mapped, using the heap: {}", e);
            None
        }
    }
}

impl AsMut<[Block]> for Workspace {
    fn as_mut(&mut self) -> &mut [Block] {
        match &mut self.memory {
            #[cfg(all(target_os = "linux", target_env = "gnu"))]
            Memory::Mapped(map) => {
                let blocks = map.len() / Block::SIZE;
                // SAFETY: `blocks` whole blocks fit in the map, which is page
                // aligned (a Block needs 64) and zero-filled by the system;
                // a Block is plain u64s, valid for any bytes; the slice
                // borrows the map mutably, so nothing else reaches it while
                // the slice lives.
                unsafe { std::slice::from_raw_parts_mut(map.as_mut_ptr().cast::<Block>(), blocks) }
            }
            Memory::Heap(blocks) => blocks,
        }
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        for block in self.as_mut() {
            block.zeroize();
        }
    }
}

/// `argon.hash_password_into` with the working memory in a [`Workspace`].
fn run(argon: &Argon2<'_>, pwd: &[u8], salt: &[u8], out: &mut [u8]) -> Result<()> {
    let memory = Workspace::new(argon.params())?;
    argon
        .hash_password_into_with_memory(pwd, salt, out, memory)
        .map_err(|e| AppError::Internal(format!("argon2: {}", e)))
}

fn phc_error(e: argon2::password_hash::Error) -> AppError {
    AppError::Internal(format!("invalid password hash: {}", e))
}

pub fn hash_password(pepper: &[u8], password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    hash_with_salt(pepper, password, &salt)
}

/// The PHC string the argon2 crate's `PasswordHasher` makes, with the run's
/// memory from [`run`].
fn hash_with_salt(pepper: &[u8], password: &str, salt: &SaltString) -> Result<String> {
    let argon = argon2()?;
    let mut salt_buf = [0u8; 64];
    let salt_bytes = salt
        .as_salt()
        .decode_b64(&mut salt_buf)
        .map_err(phc_error)?;
    let mut out = [0u8; Params::DEFAULT_OUTPUT_LEN];
    run(
        &argon,
        peppered(pepper, password).as_bytes(),
        salt_bytes,
        &mut out,
    )?;
    let phc = PasswordHash {
        algorithm: Algorithm::Argon2id.ident(),
        version: Some(Version::V0x13.into()),
        params: ParamsString::try_from(argon.params()).map_err(phc_error)?,
        salt: Some(salt.as_salt()),
        hash: Some(Output::new(&out).map_err(phc_error)?),
    };
    out.zeroize();
    Ok(phc.to_string())
}

pub fn verify_password(pepper: &[u8], password: &str, hash: &str) -> Result<bool> {
    ARGON2_RUNS.with(|n| n.set(n.get() + 1));
    let parsed = PasswordHash::new(hash).map_err(phc_error)?;
    // As the crate's PasswordVerifier: a hash without a salt or an output
    // matches no password.
    let (Some(expected), Some(salt)) = (parsed.hash, parsed.salt) else {
        return Ok(false);
    };
    let algorithm = Algorithm::try_from(parsed.algorithm).map_err(phc_error)?;
    let version = match parsed.version {
        Some(v) => {
            Version::try_from(v).map_err(|e| AppError::Internal(format!("argon2: {}", e)))?
        }
        None => Version::default(),
    };
    let params = Params::try_from(&parsed).map_err(phc_error)?;
    if params.m_cost() > MAX_M_COST_KIB
        || params.t_cost() > MAX_T_COST
        || params.p_cost() > MAX_P_COST
    {
        return Err(AppError::Internal(
            "invalid password hash: parameters beyond what this app accepts".into(),
        ));
    }
    let mut salt_buf = [0u8; 64];
    let salt_bytes = salt.decode_b64(&mut salt_buf).map_err(phc_error)?;
    let mut out = vec![0u8; expected.len()];
    run(
        &Argon2::new(algorithm, version, params),
        peppered(pepper, password).as_bytes(),
        salt_bytes,
        &mut out,
    )?;
    // Output's equality is constant-time.
    let same = Output::new(&out).map_err(phc_error)? == expected;
    out.zeroize();
    Ok(same)
}

/// Deterministic, indexable digest of an API key: HMAC-SHA256 keyed with the
/// pepper. Lets the server find the one candidate row without running Argon2
/// over every stored key.
pub fn lookup_hmac(pepper: &[u8], api_key: &str) -> String {
    // HMAC accepts any key length, so this cannot fail.
    let mut mac = match <Hmac<Sha256> as Mac>::new_from_slice(pepper) {
        Ok(m) => m,
        Err(_) => return String::new(),
    };
    mac.update(b"openalgo-api-key:");
    mac.update(api_key.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Deterministic identity of a broker's saved keys (NEW-TR-01): HMAC-SHA256
/// keyed with the pepper, in a namespace of its own. It names the key set a
/// broker session was made from, for brokers whose login names no account,
/// without storing or revealing any part of the keys. Each part is length
/// prefixed, so two different sets never run together into one input.
pub fn broker_keys_hmac(pepper: &[u8], parts: &[&str]) -> String {
    // HMAC accepts any key length, so this cannot fail.
    let mut mac = match <Hmac<Sha256> as Mac>::new_from_slice(pepper) {
        Ok(m) => m,
        Err(_) => return String::new(),
    };
    mac.update(b"openalgo-broker-keys:");
    for part in parts {
        mac.update(&(part.len() as u64).to_le_bytes());
        mac.update(part.as_bytes());
    }
    hex::encode(mac.finalize().into_bytes())
}

/// Raw 32-byte key derived from a password, used to wrap the key vault
/// (KEY-01).
pub fn derive_kek(password: &str, salt: &[u8]) -> Result<[u8; 32]> {
    let mut out = [0u8; 32];
    run(&argon2()?, password.as_bytes(), salt, &mut out)
        .map_err(|e| AppError::Encryption(format!("key derivation failed: {}", e)))?;
    Ok(out)
}

thread_local! {
    /// Argon2 runs on this thread: a sign-in (which runs on one blocking
    /// thread) pads its work to the same count whatever it finds
    /// ([`pad_argon2_runs`]), and the tests read it.
    pub static ARGON2_RUNS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Argon2 runs on this thread so far.
pub fn argon2_runs() -> u32 {
    ARGON2_RUNS.with(|n| n.get())
}

/// Spend stand-in Argon2 runs until this thread has run `target` since
/// `since` (an earlier [`argon2_runs`]). A sign-in calls it on every way
/// out, so a wrong password, a name that is not the account's and keys that
/// cannot be read all cost what checking a password costs: no early answer
/// before Argon2 tells them apart (KEY-01 security review).
pub fn pad_argon2_runs(since: u32, target: u32, password: &str) {
    while argon2_runs().wrapping_sub(since) < target {
        stand_in_password_check(password);
    }
}

/// Whether `hash` is a password hash this build can check (a PHC string).
pub fn hash_is_readable(hash: &str) -> bool {
    PasswordHash::new(hash).is_ok()
}

/// A random pepper per process for the stand-in checks: it guards
/// nothing, it only makes them cost what a real check costs.
fn stand_in_pepper() -> &'static [u8; 32] {
    static PEPPER: std::sync::OnceLock<[u8; 32]> = std::sync::OnceLock::new();
    PEPPER.get_or_init(|| {
        let mut p = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut p);
        p
    })
}

/// An Argon2id hash with the account's parameters, of a random password
/// nobody knows, made once per process.
fn stand_in_hash() -> &'static str {
    static HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    if let Some(hash) = HASH.get() {
        return hash;
    }
    let mut secret = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut secret);
    match hash_password(stand_in_pepper(), &hex::encode(secret)) {
        // Kept only once it was made: an empty hash would end a stand-in
        // check at once, and the timing would tell again.
        Ok(hash) => HASH.get_or_init(|| hash),
        Err(_) => STAND_IN_FALLBACK,
    }
}

/// A well-formed hash with the account's parameters that no password
/// matches, for a stand-in check while the real stand-in hash could not be
/// made: checking it costs the same Argon2 work.
const STAND_IN_FALLBACK: &str =
    "$argon2id$v=19$m=19456,t=2,p=1$c3RhbmQtaW4tc2FsdA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

/// The work of checking `password` against the account's password hash,
/// spent where a sign-in has no hash to check or no vault to open (a name
/// that is not the account's, a password that did not open the vault, keys
/// that cannot be read), so that every sign-in costs the same and its
/// timing tells nothing about the name or the password (KEY-01; see
/// [`pad_argon2_runs`]).
pub fn stand_in_password_check(password: &str) {
    STAND_IN_CHECKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let _ = verify_password(stand_in_pepper(), password, stand_in_hash());
}

/// How many stand-in checks ran (tests read it to see a path that found no
/// hash to check still did the work).
pub static STAND_IN_CHECKS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(test)]
mod tests;
