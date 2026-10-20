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
mod tests {
    use super::*;
    use argon2::password_hash::{PasswordHasher, PasswordVerifier};
    use argon2::{AssociatedData, KeyId, ParamsBuilder};

    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    thread_local! {
        /// Makes [`map_anon`] refuse on this thread, as a system out of
        /// map space or address space would.
        pub(super) static MAP_FAILS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    /// Review P2: a map the system refuses must not become a sign-in that
    /// fails (the key vault would read as unreadable, and the only way out
    /// offered is Start fresh). The run falls back to the heap and gives the
    /// same answers.
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[test]
    fn a_refused_map_falls_back_to_the_heap_and_signs_in() {
        MAP_FAILS.with(|f| f.set(true));
        let pepper = [7u8; 32];
        let verified = verify_password(&pepper, "Secret@123", BEFORE);
        let kek = derive_kek("pw", b"0123456789abcdef");
        let hash = hash_password(&pepper, "Secret@123");
        MAP_FAILS.with(|f| f.set(false));
        assert!(verified.unwrap());
        assert_eq!(hex::encode(kek.unwrap()), KEK_BEFORE);
        assert!(verify_password(&pepper, "Secret@123", &hash.unwrap()).unwrap());
    }

    /// Review P3: the stand-in check's fallback hash is well formed, has the
    /// account's parameters (so checking it costs the same work) and
    /// matches nothing.
    #[test]
    fn the_stand_in_fallback_costs_a_full_check_and_matches_nothing() {
        let parsed = PasswordHash::new(STAND_IN_FALLBACK).unwrap();
        let params = Params::try_from(&parsed).unwrap();
        let own = argon2().unwrap();
        assert_eq!(
            (params.m_cost(), params.t_cost(), params.p_cost()),
            (
                own.params().m_cost(),
                own.params().t_cost(),
                own.params().p_cost()
            )
        );
        assert!(!verify_password(&[7u8; 32], "anything", STAND_IN_FALLBACK).unwrap());
        assert!(!stand_in_hash().is_empty());
    }

    /// Review P3: a stored hash asking for more than the app accepts is
    /// refused before any memory is taken, never run.
    #[test]
    fn a_hash_asking_for_huge_costs_is_refused_unrun() {
        let tail = "$c29tZXNhbHRzb21lc2FsdA$EOS7b0Gy28yNMM9Y3Eb0xS8WgPgz2iXnO87OMlG2usA";
        for params in ["m=262145,t=2,p=1", "m=19456,t=17,p=1", "m=19456,t=2,p=17"] {
            let hash = format!("$argon2id$v=19${}{}", params, tail);
            let started = std::time::Instant::now();
            assert!(
                verify_password(&[7u8; 32], "pw", &hash).is_err(),
                "{}",
                hash
            );
            assert!(
                started.elapsed() < std::time::Duration::from_secs(1),
                "{}",
                hash
            );
        }
        let at_the_bound = format!("$argon2id$v=19$m=8,t=16,p=1{}", tail);
        assert!(!verify_password(&[7u8; 32], "pw", &at_the_bound).unwrap());
    }

    /// What the crate's own verifier says, as this file asked it before:
    /// Some(true or false), or None for an error.
    fn crate_verdict(pepper: &[u8], password: &str, hash: &str) -> Option<bool> {
        let parsed = PasswordHash::new(hash).ok()?;
        match Argon2::default().verify_password(peppered(pepper, password).as_bytes(), &parsed) {
            Ok(()) => Some(true),
            Err(argon2::password_hash::Error::Password) => Some(false),
            Err(_) => None,
        }
    }

    fn within_bounds(hash: &str) -> bool {
        PasswordHash::new(hash)
            .ok()
            .and_then(|p| Params::try_from(&p).ok())
            .is_none_or(|p| {
                p.m_cost() <= MAX_M_COST_KIB && p.t_cost() <= MAX_T_COST && p.p_cost() <= MAX_P_COST
            })
    }

    /// Review P3: side by side with the crate's own verifier (the security
    /// review's harness, cut to cheap costs): every algorithm, both
    /// versions, other output lengths, key ids and associated data, a
    /// version left out, malformed strings and mutations of a good one.
    /// The answers must agree: match, no match or error.
    #[test]
    fn verification_agrees_with_the_crates_verifier() {
        let pepper = [7u8; 32];
        let mut corpus: Vec<String> = Vec::new();
        for alg in [Algorithm::Argon2d, Algorithm::Argon2i, Algorithm::Argon2id] {
            for ver in [Version::V0x10, Version::V0x13] {
                for (m, t, p) in [(8, 1, 1), (16, 2, 2), (64, 3, 4)] {
                    for out in [None, Some(10), Some(64)] {
                        for extra in 0..3 {
                            let mut b = ParamsBuilder::new();
                            b.m_cost(m).t_cost(t).p_cost(p);
                            if let Some(o) = out {
                                b.output_len(o);
                            }
                            if extra >= 1 {
                                b.keyid(KeyId::new(b"kid12").unwrap());
                            }
                            if extra == 2 {
                                b.data(AssociatedData::new(b"assoc data here").unwrap());
                            }
                            let salt = SaltString::from_b64("c29tZXNhbHRzb21lc2FsdA").unwrap();
                            let h = Argon2::new(alg, ver, b.build().unwrap())
                                .hash_password(peppered(&pepper, "pw").as_bytes(), &salt)
                                .unwrap()
                                .to_string();
                            corpus.push(h.replace("$v=19$", "$").replace("$v=16$", "$"));
                            corpus.push(h);
                        }
                    }
                }
            }
        }
        let good = "$argon2id$v=19$m=8,t=1,p=1$c29tZXNhbHRzb21lc2FsdA$EOS7b0Gy28yNMM9Y3Eb0xS8WgPgz2iXnO87OMlG2usA";
        for odd in [
            "",
            "not a hash",
            "$argon2id$v=19$m=8,t=1,p=1",
            "$argon2id$v=19$m=8,t=1,p=1$c29tZXNhbHRzb21lc2FsdA",
            "$argon2id$v=19$m=8,t=1,p=1$c29tZXNhbHRzb21lc2FsdA$",
            "$argon2id$v=19$m=8,t=1,p=1$$EOS7b0Gy28yNMM9Y3Eb0xS8WgPgz2iXnO87OMlG2usA",
            "$argon2id$v=19$m=8,t=1,p=1$c29tZQ$EOS7b0Gy28yNMM9Y3Eb0xS8WgPgz2iXnO87OMlG2usA",
            "$argon2id$v=19$m=8,t=1,p=1$c29tZXNhbHRzb21lc2FsdA$AAAAAAAAAAAA",
            "$argon2id$v=20$m=8,t=1,p=1$c29tZXNhbHRzb21lc2FsdA$EOS7b0Gy28yNMM9Y3Eb0xS8WgPgz2iXnO87OMlG2usA",
            "$argon2id$v=19$m=7,t=1,p=1$c29tZXNhbHRzb21lc2FsdA$EOS7b0Gy28yNMM9Y3Eb0xS8WgPgz2iXnO87OMlG2usA",
            "$argon2id$v=19$m=8,t=0,p=1$c29tZXNhbHRzb21lc2FsdA$EOS7b0Gy28yNMM9Y3Eb0xS8WgPgz2iXnO87OMlG2usA",
            "$argon2id$v=19$m=8,t=1,p=2$c29tZXNhbHRzb21lc2FsdA$EOS7b0Gy28yNMM9Y3Eb0xS8WgPgz2iXnO87OMlG2usA",
            "$argon2id$v=19$m=8,t=1,p=1,x=1$c29tZXNhbHRzb21lc2FsdA$EOS7b0Gy28yNMM9Y3Eb0xS8WgPgz2iXnO87OMlG2usA",
            "$argon2id$v=19$m=8,m=16,t=1,p=1$c29tZXNhbHRzb21lc2FsdA$EOS7b0Gy28yNMM9Y3Eb0xS8WgPgz2iXnO87OMlG2usA",
            "$argon2$v=19$m=8,t=1,p=1$c29tZXNhbHRzb21lc2FsdA$EOS7b0Gy28yNMM9Y3Eb0xS8WgPgz2iXnO87OMlG2usA",
            "$scrypt$ln=15,r=8,p=1$c29tZXNhbHRzb21lc2FsdA$EOS7b0Gy28yNMM9Y3Eb0xS8WgPgz2iXnO87OMlG2usA",
            "$argon2id$v=19$m=8,t=1,p=1,data=$c29tZXNhbHRzb21lc2FsdA$EOS7b0Gy28yNMM9Y3Eb0xS8WgPgz2iXnO87OMlG2usA",
        ] {
            corpus.push(odd.to_string());
        }
        // Mutations of a good hash: one to three edits each.
        let mut rng = 0x1234_5678_9abc_def0u64;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        let alphabet = b"$=,+/.-0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ ";
        for _ in 0..400 {
            let mut s = good.as_bytes().to_vec();
            for _ in 0..1 + next() % 3 {
                let i = (next() as usize) % (s.len() + 1);
                match next() % 3 {
                    0 if i < s.len() => {
                        s.remove(i);
                    }
                    1 => s.insert(i, alphabet[(next() as usize) % alphabet.len()]),
                    _ if i < s.len() => s[i] = alphabet[(next() as usize) % alphabet.len()],
                    _ => {}
                }
            }
            if let Ok(s) = String::from_utf8(s) {
                corpus.push(s);
            }
        }
        let mut compared = 0;
        for hash in corpus.iter().filter(|h| within_bounds(h)) {
            for (pepper, password) in [([7u8; 32], "pw"), ([7u8; 32], "pW"), ([8u8; 32], "pw")] {
                let ours = verify_password(&pepper, password, hash).ok();
                assert_eq!(
                    ours,
                    crate_verdict(&pepper, password, hash),
                    "{:?} with {}",
                    hash,
                    password
                );
                compared += 1;
            }
        }
        assert!(compared > 1000, "{}", compared);
    }

    #[test]
    fn hash_and_verify() {
        let pepper = [7u8; 32];
        let h = hash_password(&pepper, "Secret@123").unwrap();
        assert!(verify_password(&pepper, "Secret@123", &h).unwrap());
        assert!(!verify_password(&pepper, "wrong", &h).unwrap());
        assert!(!verify_password(&[8u8; 32], "Secret@123", &h).unwrap());
    }

    #[test]
    fn hmac_is_deterministic_and_keyed() {
        let a = lookup_hmac(&[1u8; 32], "key");
        assert_eq!(a, lookup_hmac(&[1u8; 32], "key"));
        assert_ne!(a, lookup_hmac(&[2u8; 32], "key"));
        assert_ne!(a, lookup_hmac(&[1u8; 32], "key2"));
        assert_eq!(a.len(), 64);
    }

    /// The crate's own PasswordHasher, as this file used it before the
    /// run's memory moved to a [`Workspace`].
    fn crate_hash(pepper: &[u8], password: &str, salt: &SaltString) -> String {
        argon2()
            .unwrap()
            .hash_password(peppered(pepper, password).as_bytes(), salt)
            .unwrap()
            .to_string()
    }

    #[test]
    fn hashes_are_the_crates_own_byte_for_byte() {
        let pepper = [7u8; 32];
        for (password, salt) in [
            ("Secret@123", "c29tZXNhbHRzb21lc2FsdA"),
            ("Market#2026", "MTIzNDU2Nzg5MGFiY2RlZg"),
        ] {
            let salt = SaltString::from_b64(salt).unwrap();
            assert_eq!(
                hash_with_salt(&pepper, password, &salt).unwrap(),
                crate_hash(&pepper, password, &salt)
            );
        }
    }

    #[test]
    fn a_hash_made_before_the_change_still_verifies() {
        // BEFORE comes from the reference implementation (libargon2 through
        // argon2-cffi 23.1.0) with the same inputs; the crate's own hasher,
        // which made every stored hash before the run's memory moved to a
        // Workspace, gives the same string.
        let pepper = [7u8; 32];
        assert_eq!(
            crate_hash(
                &pepper,
                "Secret@123",
                &SaltString::from_b64("c29tZXNhbHRzb21lc2FsdA").unwrap()
            ),
            BEFORE
        );
        assert!(verify_password(&pepper, "Secret@123", BEFORE).unwrap());
        assert!(!verify_password(&pepper, "Secret@124", BEFORE).unwrap());
        assert!(!verify_password(&[8u8; 32], "Secret@123", BEFORE).unwrap());
    }

    const BEFORE: &str =
        "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHRzb21lc2FsdA$EOS7b0Gy28yNMM9Y3Eb0xS8WgPgz2iXnO87OMlG2usA";

    #[test]
    fn verification_follows_the_parameters_in_the_hash() {
        // Another cost and output length, made by the crate: verification
        // reads them from the PHC string, as the crate's verifier did.
        let pepper = [7u8; 32];
        let params = Params::new(4096, 1, 1, Some(40)).unwrap();
        let salt = SaltString::from_b64("c29tZXNhbHRzb21lc2FsdA").unwrap();
        let other = Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password(peppered(&pepper, "pw").as_bytes(), &salt)
            .unwrap()
            .to_string();
        assert!(other.contains("m=4096,t=1,p=1"));
        assert!(verify_password(&pepper, "pw", &other).unwrap());
        assert!(!verify_password(&pepper, "pW", &other).unwrap());
        // No salt or no output matches nothing, as with the crate's verifier.
        assert!(!verify_password(&pepper, "pw", "$argon2id$v=19$m=4096,t=1,p=1").unwrap());
        assert!(verify_password(&pepper, "pw", "not a hash").is_err());
    }

    #[test]
    fn kek_is_the_crates_own_and_unchanged() {
        let mut crate_out = [0u8; 32];
        argon2()
            .unwrap()
            .hash_password_into(b"pw", b"0123456789abcdef", &mut crate_out)
            .unwrap();
        let ours = derive_kek("pw", b"0123456789abcdef").unwrap();
        assert_eq!(ours, crate_out);
        // From the reference implementation as above: a vault wrapped before
        // this change still opens.
        assert_eq!(hex::encode(ours), KEK_BEFORE);
    }

    const KEK_BEFORE: &str = "aa11986f53323c413e6844bf08de8d0900cb42b5af9b3c88f960319d7d992e62";

    #[test]
    fn kek_depends_on_password_and_salt() {
        let a = derive_kek("pw", b"0123456789abcdef").unwrap();
        assert_eq!(a, derive_kek("pw", b"0123456789abcdef").unwrap());
        assert_ne!(a, derive_kek("pw2", b"0123456789abcdef").unwrap());
        assert_ne!(a, derive_kek("pw", b"fedcba9876543210").unwrap());
    }

    /// NEW-TR-01: the identity of a broker's saved keys is stable for the
    /// same keys, changes with any part of them or the pepper, never runs
    /// two parts together, and is not the API-key index's digest.
    #[test]
    fn broker_keys_identity_is_stable_and_separated() {
        let pepper = b"pepper";
        let id = broker_keys_hmac(pepper, &["paytm", "key1", ""]);
        assert_eq!(id, broker_keys_hmac(pepper, &["paytm", "key1", ""]));
        assert_eq!(id.len(), 64);
        assert_ne!(id, broker_keys_hmac(pepper, &["paytm", "key2", ""]));
        assert_ne!(id, broker_keys_hmac(b"other", &["paytm", "key1", ""]));
        assert_ne!(
            broker_keys_hmac(pepper, &["ab", "c"]),
            broker_keys_hmac(pepper, &["a", "bc"])
        );
        assert_ne!(
            broker_keys_hmac(pepper, &["key1"]),
            lookup_hmac(pepper, "key1")
        );
    }
}
