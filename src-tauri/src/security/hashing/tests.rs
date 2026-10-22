//! Tests for [`super`]: Argon2 known-answer vectors, the crate's own
//! verifier side by side, and the fixed test passwords and salts they need.
//! A file of its own so CodeQL's test-path exclusion covers it
//! (`.github/codeql/codeql-config.yml`).

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
