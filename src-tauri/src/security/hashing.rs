//! Argon2id password hashing with a pepper, and the HMAC lookup index for
//! API keys.

use crate::error::{AppError, Result};
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2, Params, Version,
};
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;

pub const PEPPER_SIZE: usize = 32;

/// OWASP baseline for Argon2id. Parameters are stored in the PHC string, so
/// verification of older hashes keeps working if these change.
fn argon2() -> Result<Argon2<'static>> {
    ARGON2_RUNS.with(|n| n.set(n.get() + 1));
    let params = Params::new(19456, 2, 1, None)
        .map_err(|e| AppError::Internal(format!("argon2 params: {}", e)))?;
    Ok(Argon2::new(
        argon2::Algorithm::Argon2id,
        Version::V0x13,
        params,
    ))
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

pub fn hash_password(pepper: &[u8], password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    argon2()?
        .hash_password(peppered(pepper, password).as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| AppError::Internal(format!("password hashing failed: {}", e)))
}

pub fn verify_password(pepper: &[u8], password: &str, hash: &str) -> Result<bool> {
    ARGON2_RUNS.with(|n| n.set(n.get() + 1));
    let parsed = PasswordHash::new(hash)
        .map_err(|e| AppError::Internal(format!("invalid password hash: {}", e)))?;
    match Argon2::default().verify_password(peppered(pepper, password).as_bytes(), &parsed) {
        Ok(()) => Ok(true),
        Err(argon2::password_hash::Error::Password) => Ok(false),
        Err(e) => Err(AppError::Internal(format!("password verification: {}", e))),
    }
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
    argon2()?
        .hash_password_into(password.as_bytes(), salt, &mut out)
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

    #[test]
    fn kek_depends_on_password_and_salt() {
        let a = derive_kek("pw", b"0123456789abcdef").unwrap();
        assert_eq!(a, derive_kek("pw", b"0123456789abcdef").unwrap());
        assert_ne!(a, derive_kek("pw2", b"0123456789abcdef").unwrap());
        assert_ne!(a, derive_kek("pw", b"fedcba9876543210").unwrap());
    }
}
