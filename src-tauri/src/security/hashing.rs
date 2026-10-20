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

/// One Argon2 run's working memory (19 MiB with the account's parameters)
/// in pages of its own, mapped for the run and unmapped when it ends, zeroed
/// first. Taken from the allocator instead, glibc kept the freed block in
/// the thread's malloc arena once the first one had raised its mmap
/// threshold, so every thread that ever hashed held on to up to ~19 MiB,
/// some several (weekly soak, 2026-10-18: +115 MB).
struct Workspace {
    map: memmap2::MmapMut,
    blocks: usize,
}

impl Workspace {
    fn new(params: &Params) -> Result<Self> {
        let blocks = params.block_count();
        let map = memmap2::MmapMut::map_anon(blocks * Block::SIZE)
            .map_err(|e| AppError::Internal(format!("argon2 working memory: {}", e)))?;
        Ok(Self { map, blocks })
    }
}

impl AsMut<[Block]> for Workspace {
    fn as_mut(&mut self) -> &mut [Block] {
        // SAFETY: the map is `blocks * Block::SIZE` bytes, page aligned (a
        // Block needs 64) and zero-filled by the system; a Block is plain
        // u64s, valid for any bytes; the slice borrows `self` mutably, so
        // nothing else reaches the map while it lives.
        unsafe {
            std::slice::from_raw_parts_mut(self.map.as_mut_ptr().cast::<Block>(), self.blocks)
        }
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        self.map[..].zeroize();
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
    HASH.get_or_init(|| {
        let mut secret = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut secret);
        hash_password(stand_in_pepper(), &hex::encode(secret)).unwrap_or_default()
    })
}

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
        use argon2::password_hash::PasswordHasher;
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
        use argon2::password_hash::PasswordHasher;
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
}
