//! Key material, encryption and hashing.
//!
//! # Where keys live (KEY-01, maintainer decision 2026-10-13)
//!
//! A random 256-bit data key (AES-256-GCM, encrypts every stored secret)
//! and a random 256-bit pepper (Argon2 pepper and HMAC key for the API-key
//! index) are made when the account is created. Both are wrapped with a key
//! derived from the trader's OpenAlgo password (Argon2id) and kept in the
//! one-row `key_vault` table of `openalgo.db` (`db::sqlite::key_vault`),
//! beside the password hash. No operating-system keychain is used on any
//! platform. A copied database is useless without the password.
//!
//! The keys are in memory only between a sign-in that issued a signed-in
//! session and the logout (or the app quitting): `install` and `lock`.
//! While locked nothing can be decrypted and no API key can be checked; the
//! surfaces that need one say "Sign in to OpenAlgo" (`docs/port/no-keychain.md`).
//!
//! # What lives where
//!
//! - this file: the keys, the manager that holds them, locking and
//!   unlocking, encryption and hashing with them;
//! - `sign_in`: what a sign-in's password opens, and what its caller may
//!   learn when it opens nothing;
//! - `migration`: moving an earlier build's keys (keychain, `vault.json`,
//!   `secrets.dat`) into the vault, once;
//! - `key_changes`: account setup, a password change, a fresh start and an
//!   account reset;
//! - `window`: the proof that a sign-in comes from the app window;
//! - `crypto`, `hashing`, `secret`, `keystore`, `legacy`, `totp`, `fsperm`:
//!   the primitives.

pub mod crypto;
pub mod fsperm;
pub mod hashing;
mod key_changes;
pub mod keystore;
pub mod legacy;
mod migration;
pub mod secret;
mod sign_in;
pub mod totp;
pub mod window;

use crate::db::sqlite::key_vault::{self, KeychainCleanup, Source, VaultRow};
use crate::db::sqlite::{user, SqliteDb};
use crate::error::{AppError, Result};
use base64::Engine;
use crypto::{Aad, DataCipher};
use keystore::{KeyStore, DATA_KEY, PEPPER};
use parking_lot::{Mutex, RwLock};
use rand::RngCore;
use rusqlite::Connection;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(crate) use key_changes::with_full_sync;
#[cfg(test)]
pub use migration::MoveStep;
pub use secret::{Secret, SecretBytes};
pub use sign_in::{Opened, SignInFrom, SIGN_IN_ARGON2_RUNS};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;
/// An earlier build's password-mode vault, moved into openalgo.db once.
pub const VAULT_FILE: &str = "vault.json";
/// An earlier build's vault for a new password, left by a password change
/// that stopped half way; moved like `VAULT_FILE` when its password is the
/// one the stored hash names.
pub const VAULT_NEXT_FILE: &str = "vault.next.json";

struct Keys {
    cipher: DataCipher,
    pepper: SecretBytes,
    raw_key: SecretBytes,
}

impl Keys {
    fn new(key: SecretBytes, pepper: SecretBytes) -> Result<Self> {
        if pepper.expose().len() != hashing::PEPPER_SIZE {
            return Err(AppError::Encryption("the pepper has the wrong size".into()));
        }
        Ok(Self {
            cipher: DataCipher::new(key.expose())?,
            pepper,
            raw_key: key,
        })
    }

    fn random() -> Result<Self> {
        let mut pepper = vec![0u8; hashing::PEPPER_SIZE];
        rand::rngs::OsRng.fill_bytes(&mut pepper);
        Self::new(crypto::generate_key(), SecretBytes::new(pepper))
    }

    fn wrap(&self, password: &str) -> Result<key_vault::Wrapped> {
        key_vault::wrap(self.raw_key.expose(), self.pepper.expose(), password)
    }

    fn same_as(&self, other: &Keys) -> bool {
        self.raw_key == other.raw_key && self.pepper == other.pepper
    }

    /// Whether `witness` decrypts with this data key (with associated data,
    /// or as a pre-AAD ciphertext of the oldest builds).
    fn opens(&self, w: &key_vault::Witness) -> bool {
        self.cipher.decrypt(&w.ciphertext, &w.nonce, &w.aad).is_ok()
            || self.cipher.decrypt_legacy(&w.ciphertext, &w.nonce).is_ok()
    }
}

/// Keys a sign-in opened, waiting for the session they belong to: a
/// sign-in that still needs its authenticator code holds them in its
/// pending browser session and uses them only to check the code. Dropped
/// (and zeroed with the last copy) when that session ends.
#[derive(Clone)]
pub struct Unlocked(Arc<Keys>);

impl std::fmt::Debug for Unlocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Unlocked([REDACTED])")
    }
}

impl PartialEq for Unlocked {
    fn eq(&self, other: &Self) -> bool {
        self.0.same_as(&other.0)
    }
}

impl Eq for Unlocked {}

impl Unlocked {
    /// New random keys (a new account, a fresh start).
    pub fn random() -> Result<Self> {
        Ok(Self(Arc::new(Keys::random()?)))
    }

    /// A manager holding these keys and nothing else, for reading and
    /// writing with them before they are installed (the pending sign-in's
    /// authenticator check, the rows of a new account).
    pub fn manager(&self) -> SecurityManager {
        SecurityManager::detached(self.0.clone())
    }
}

/// Where this install's keys are, read from the data folder and the
/// database. Never from the keychain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    /// No account yet: account setup makes the keys and the vault.
    NoAccount,
    /// The vault row in openalgo.db.
    Vault,
    /// An earlier build's `vault.json` (moved at the next sign-in).
    VaultFile,
    /// The retired `secrets.dat` (moved at the next sign-in).
    LegacyFile,
    /// An earlier build's OS keychain (read once at the next sign-in).
    Keychain,
}

pub struct SecurityManager {
    keys: RwLock<Option<Arc<Keys>>>,
    /// The OS keychain of earlier builds: read once by the move of an
    /// install that kept its keys there, and its entries deleted after.
    /// Never written.
    store: Arc<dyn KeyStore>,
    data_dir: PathBuf,
    /// Serialises every change of the keys or the password hash: sign-in
    /// (which may move keys), installing and locking them, a password
    /// change, a fresh start, an account reset.
    vault_lock: Mutex<()>,
    /// The credentials generation: bumped, under `vault_lock`, whenever the
    /// stored password hash or the keys change or the keys are locked
    /// (password change, fresh start, account reset, logout). A sign-in
    /// reads it before it reads the account and issues its session only if
    /// it is unchanged (`issue_if_current`).
    credentials_epoch: std::sync::atomic::AtomicU64,
    /// What shows a sign-in comes from the app window (`window`).
    window: window::WindowProof,
    #[cfg(test)]
    fault: Mutex<Option<MoveStep>>,
    #[cfg(test)]
    fail_rewrap: std::sync::atomic::AtomicBool,
}

impl std::fmt::Debug for SecurityManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecurityManager")
            .field("unlocked", &self.is_unlocked())
            .finish()
    }
}

impl SecurityManager {
    /// The manager for `data_dir`, locked. Makes no keychain call and reads
    /// nothing secret: the keys come with the first sign-in. Only the test
    /// harness's in-memory store (`KeyStore::harness_keys`) opens unlocked,
    /// with keys it keeps, so feature tests need no sign-in.
    pub fn open(data_dir: &Path, store: Arc<dyn KeyStore>) -> Result<Self> {
        let keys = if store.harness_keys() {
            Some(Arc::new(harness_keys(store.as_ref())?))
        } else {
            None
        };
        Ok(Self {
            keys: RwLock::new(keys),
            store,
            data_dir: data_dir.to_path_buf(),
            vault_lock: Mutex::new(()),
            credentials_epoch: Default::default(),
            window: window::WindowProof::new(),
            #[cfg(test)]
            fault: Mutex::new(None),
            #[cfg(test)]
            fail_rewrap: Default::default(),
        })
    }

    fn detached(keys: Arc<Keys>) -> Self {
        let m = Self::detached_locked();
        *m.keys.write() = Some(keys);
        m
    }

    fn detached_locked() -> Self {
        Self {
            keys: RwLock::new(None),
            store: Arc::new(keystore::MemoryKeyStore::new()),
            data_dir: std::env::temp_dir(),
            vault_lock: Mutex::new(()),
            credentials_epoch: Default::default(),
            window: window::WindowProof::new(),
            #[cfg(test)]
            fault: Mutex::new(None),
            #[cfg(test)]
            fail_rewrap: Default::default(),
        }
    }

    /// Random keys, unlocked, bound to no folder. For tests and the
    /// in-process HTTP harness.
    pub fn for_tests() -> Self {
        let m = Self::detached_locked();
        *m.keys.write() = Keys::random().ok().map(Arc::new);
        m
    }

    pub fn is_unlocked(&self) -> bool {
        self.keys.read().is_some()
    }

    /// The proof the app window shows on its sign-in (`window`).
    pub fn window_proof(&self) -> &window::WindowProof {
        &self.window
    }

    fn keys(&self) -> Result<Arc<Keys>> {
        self.keys.read().clone().ok_or(AppError::Locked)
    }

    // ------------------------------------------------------ the generation

    /// The credentials generation now. A sign-in reads it before it reads
    /// the account, and passes it to `issue_if_current`.
    pub fn credentials_epoch(&self) -> u64 {
        self.credentials_epoch
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    fn bump_credentials_epoch(&self) {
        self.credentials_epoch
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    /// Run `issue` (which creates the signed-in session) only if no password
    /// or key change and no logout happened since `epoch` was read, holding
    /// the lock every such change holds: a sign-in that checked a password a
    /// change then replaced, or that finished after a logout signed every
    /// window out, gets no session.
    pub fn issue_if_current<T>(&self, epoch: u64, issue: impl FnOnce() -> T) -> Option<T> {
        let _change = self.vault_lock.lock();
        if self.credentials_epoch() != epoch {
            return None;
        }
        Some(issue())
    }

    // ------------------------------------------------- install and lock

    /// Keep `keys` in memory: called with the session a sign-in issues
    /// (inside `issue_if_current`) and by account setup.
    pub fn install(&self, keys: &Unlocked) {
        *self.keys.write() = Some(keys.0.clone());
    }

    /// Drop the keys (logout, app quit, a fresh start, an account reset).
    /// The raw key and the pepper are zeroed when the last copy goes, the
    /// AES key schedule too (`aes/zeroize`); a request in flight holding a
    /// copy finishes with it. Every sign-in still in progress is voided.
    pub fn lock(&self) {
        self.lock_with(|| ());
    }

    /// `lock`, running `also` (signing every window out) in the same step,
    /// under the lock `issue_if_current` holds: no session issued just
    /// before the lock outlives the keys it was issued with.
    pub fn lock_with<T>(&self, also: impl FnOnce() -> T) -> T {
        let _change = self.vault_lock.lock();
        *self.keys.write() = None;
        self.bump_credentials_epoch();
        also()
    }

    // ------------------------------------------------------ where they are

    /// Where this install's keys are, from the database and the folder.
    pub fn key_source(&self, conn: &Connection) -> Result<KeySource> {
        if key_vault::exists(conn)? {
            return Ok(KeySource::Vault);
        }
        if !user::has_user(conn)? {
            return Ok(KeySource::NoAccount);
        }
        if self.data_dir.join(VAULT_FILE).exists() || self.data_dir.join(VAULT_NEXT_FILE).exists() {
            return Ok(KeySource::VaultFile);
        }
        if legacy::path(&self.data_dir).exists() {
            return Ok(KeySource::LegacyFile);
        }
        Ok(KeySource::Keychain)
    }

    // ---------------------------------------------------------------- crypto

    pub fn encrypt(&self, plaintext: &str, aad: &Aad) -> Result<(String, String)> {
        self.keys()?.cipher.encrypt(plaintext.as_bytes(), aad)
    }

    pub fn decrypt(&self, ct: &str, nonce: &str, aad: &Aad) -> Result<Secret> {
        self.keys()?.cipher.decrypt(ct, nonce, aad)
    }

    /// Pre-AAD ciphertexts, migration only.
    pub fn decrypt_legacy(&self, ct: &str, nonce: &str) -> Result<Secret> {
        self.keys()?.cipher.decrypt_legacy(ct, nonce)
    }

    pub fn hash_password(&self, password: &str) -> Result<String> {
        hashing::hash_password(self.keys()?.pepper.expose(), password)
    }

    pub fn verify_password(&self, password: &str, hash: &str) -> Result<bool> {
        hashing::verify_password(self.keys()?.pepper.expose(), password, hash)
    }

    pub fn api_key_lookup(&self, api_key: &str) -> Result<String> {
        Ok(hashing::lookup_hmac(self.keys()?.pepper.expose(), api_key))
    }

    /// The identity of a broker's saved keys ([`hashing::broker_keys_hmac`]).
    pub fn broker_keys_identity(&self, parts: &[&str]) -> Result<String> {
        Ok(hashing::broker_keys_hmac(
            self.keys()?.pepper.expose(),
            parts,
        ))
    }
}

/// The test harness's keys, kept in its in-memory store (made on first
/// use), as the keychain mode of earlier builds kept them.
fn harness_keys(store: &dyn KeyStore) -> Result<Keys> {
    let get = |n: &str| store.get(n).map_err(|e| AppError::Keychain(e.to_string()));
    if let (Some(k), Some(p)) = (get(DATA_KEY)?, get(PEPPER)?) {
        return Keys::new(decode_b64(k.expose())?, decode_b64(p.expose())?);
    }
    let keys = Keys::random()?;
    let set = |n: &str, v: &[u8]| {
        store
            .set(n, &B64.encode(v))
            .map_err(|e| AppError::Keychain(e.to_string()))
    };
    set(PEPPER, keys.pepper.expose())?;
    set(DATA_KEY, keys.raw_key.expose())?;
    Ok(keys)
}

fn decode_b64(s: &str) -> Result<SecretBytes> {
    B64.decode(s.trim())
        .map(SecretBytes::new)
        .map_err(|_| AppError::KeysDoNotMatch)
}

/// Test helper: write an earlier build's `vault.json` holding `keys`
/// wrapped with `password` (the format those builds wrote).
#[cfg(test)]
pub fn write_vault_file_for_test(path: &Path, keys: &Unlocked, password: &str) {
    let w = keys.0.wrap(password).unwrap();
    let json = serde_json::json!({
        "version": 1,
        "kdf": key_vault::KDF,
        "salt": w.salt,
        "nonce": w.nonce,
        "ciphertext": w.ciphertext,
    });
    std::fs::write(path, serde_json::to_vec(&json).unwrap()).unwrap();
}

#[cfg(test)]
impl Unlocked {
    /// Keys with the same pepper and another data key: what a password
    /// opens when its keys belong to other data.
    pub fn with_another_data_key(&self) -> Unlocked {
        Unlocked(Arc::new(
            Keys::new(crypto::generate_key(), self.0.pepper.clone()).unwrap(),
        ))
    }
}

/// Test helper: an earlier build's keychain entries for `keys`.
#[cfg(test)]
pub fn keychain_entries_for_test(keys: &Unlocked) -> (String, String) {
    (
        B64.encode(keys.0.raw_key.expose()),
        B64.encode(keys.0.pepper.expose()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use keystore::CountingKeyStore;

    fn db(dir: &Path) -> SqliteDb {
        SqliteDb::new(&dir.join("openalgo.db")).unwrap()
    }

    /// KEY-01: opening the app and creating the account make no keychain
    /// call; the vault is a row of openalgo.db and no key file exists.
    #[test]
    fn a_new_install_never_touches_the_keychain() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(CountingKeyStore::default());
        let m = SecurityManager::open(dir.path(), store.clone()).unwrap();
        let db = db(dir.path());
        assert!(!m.is_unlocked());
        assert_eq!(
            m.key_source(&db.conn().unwrap()).unwrap(),
            KeySource::NoAccount
        );
        m.create_account(&db, "First@123", |conn, with| {
            let hash = with.hash_password("First@123")?;
            user::insert(conn, with, "trader", "t@example.com", &hash, "SECRET")?;
            Ok(())
        })
        .unwrap();
        assert!(!m.is_unlocked(), "keys in memory without a sign-in");
        let conn = db.conn().unwrap();
        assert_eq!(m.key_source(&conn).unwrap(), KeySource::Vault);
        assert_eq!(
            key_vault::read(&conn).unwrap().unwrap().source,
            Source::Setup
        );
        drop(conn);
        let opened = m.open_for_sign_in(&db, "trader", "First@123", SignInFrom::AppWindow);
        assert!(matches!(opened, Opened::Keys(_)));
        assert!(!m.is_unlocked(), "a sign-in's password step installed keys");
        assert!(matches!(
            m.open_for_sign_in(&db, "trader", "Wrong@123", SignInFrom::AppWindow),
            Opened::WrongPassword
        ));
        assert_eq!(store.calls(), 0, "the keychain was called");
        assert!(!dir.path().join(VAULT_FILE).exists());
    }

    /// Locked, nothing decrypts or checks; the keys come back only through
    /// a sign-in.
    #[test]
    fn locked_means_nothing_decrypts() {
        let dir = tempfile::tempdir().unwrap();
        let m = SecurityManager::open(dir.path(), Arc::new(CountingKeyStore::default())).unwrap();
        let db = db(dir.path());
        m.create_account(&db, "First@123", |conn, with| {
            let hash = with.hash_password("First@123")?;
            user::insert(conn, with, "trader", "t@example.com", &hash, "SECRET")?;
            Ok(())
        })
        .unwrap();
        assert!(matches!(
            m.encrypt("x", &Aad::fixed("x")),
            Err(AppError::Locked)
        ));
        let Opened::Keys(keys) =
            m.open_for_sign_in(&db, "trader", "First@123", SignInFrom::Elsewhere)
        else {
            panic!("the password did not open the vault")
        };
        m.install(&keys);
        let (ct, n) = m.encrypt("x", &Aad::fixed("x")).unwrap();
        m.lock();
        assert!(matches!(
            m.decrypt(&ct, &n, &Aad::fixed("x")),
            Err(AppError::Locked)
        ));
        assert!(matches!(m.api_key_lookup("k"), Err(AppError::Locked)));
        assert!(matches!(m.hash_password("p"), Err(AppError::Locked)));
    }

    /// A lock voids a sign-in that read the generation before it.
    #[test]
    fn a_lock_voids_a_sign_in_in_progress() {
        let m = SecurityManager::for_tests();
        let epoch = m.credentials_epoch();
        m.lock();
        assert_eq!(m.issue_if_current(epoch, || ()), None);
        assert_eq!(m.issue_if_current(m.credentials_epoch(), || 1), Some(1));
    }

    /// An earlier build's install: the account and its encrypted TOTP
    /// secret written with `keys`, and no vault row.
    fn earlier_install(dir: &Path, keys: &Unlocked) -> SqliteDb {
        let db = db(dir);
        let with = keys.manager();
        let hash = with.hash_password(PW).unwrap();
        user::insert(
            &db.conn().unwrap(),
            &with,
            "trader",
            "t@example.com",
            &hash,
            "SECRET",
        )
        .unwrap();
        db
    }

    const PW: &str = "First@123";

    /// KEY-01 security review: every sign-in costs the same Argon2 work,
    /// whatever it finds, so its timing tells neither the name nor the
    /// password nor the state of the keys: two runs (open the vault, check
    /// the hash), two per vault file an earlier build left. Checked for a
    /// name that is not the account's, a wrong password and, where nothing
    /// can be proven, the right one; with the vault, a damaged vault, keys
    /// still in an earlier build's keychain (from the app window and from
    /// anywhere else, readable and locked), `vault.json` (with and without
    /// `vault.next.json`) and `secrets.dat`.
    #[test]
    fn every_sign_in_costs_the_same_work_whatever_it_finds() {
        use SignInFrom::{AppWindow, Elsewhere};
        // The stand-in hash is made once per process: make it now.
        hashing::stand_in_password_check("warm up");
        let check = |m: &SecurityManager,
                     db: &SqliteDb,
                     what: &str,
                     tries: &[(&str, &str, SignInFrom)],
                     runs: u32,
                     expect: &dyn Fn(&Opened) -> bool| {
            for (name, password, from) in tries {
                let since = hashing::argon2_runs();
                let opened = m.open_for_sign_in(db, name, password, *from);
                let spent = hashing::argon2_runs() - since;
                assert!(
                    expect(&opened),
                    "{}: {} {:?}: {:?}",
                    what,
                    name,
                    from,
                    opened
                );
                assert_eq!(spent, runs, "{}: {} {:?} {:?}", what, name, from, opened);
            }
        };
        let wrong = |o: &Opened| matches!(o, Opened::WrongPassword);
        let not_proven = [
            ("someone", PW, AppWindow),
            ("trader", "Wrong@123", AppWindow),
            ("someone", PW, Elsewhere),
            ("trader", "Wrong@123", Elsewhere),
        ];
        let anything = [
            ("trader", PW, AppWindow),
            ("trader", PW, Elsewhere),
            ("trader", "Wrong@123", AppWindow),
            ("someone", PW, Elsewhere),
        ];

        // The vault.
        let dir = tempfile::tempdir().unwrap();
        let m = SecurityManager::open(dir.path(), Arc::new(CountingKeyStore::default())).unwrap();
        let vdb = db(dir.path());
        m.create_account(&vdb, PW, |conn, with| {
            let hash = with.hash_password(PW)?;
            user::insert(conn, with, "trader", "t@example.com", &hash, "SECRET")?;
            Ok(())
        })
        .unwrap();
        check(&m, &vdb, "vault", &not_proven, 2, &wrong);
        check(&m, &vdb, "vault", &[("trader", PW, Elsewhere)], 2, &|o| {
            matches!(o, Opened::Keys(_))
        });
        // A damaged vault.
        vdb.conn()
            .unwrap()
            .execute("UPDATE key_vault SET nonce = 'AAAA'", [])
            .unwrap();
        check(&m, &vdb, "damaged vault", &anything, 2, &|o| {
            matches!(o, Opened::Unreadable(AppError::VaultUnreadable))
        });

        // Keys still in an earlier build's keychain.
        let dir = tempfile::tempdir().unwrap();
        let keys = Unlocked::random().unwrap();
        let kdb = earlier_install(dir.path(), &keys);
        let store = Arc::new(CountingKeyStore::default());
        let (k, p) = keychain_entries_for_test(&keys);
        store.seed(DATA_KEY, &k);
        store.seed(PEPPER, &p);
        let m = SecurityManager::open(dir.path(), store.clone()).unwrap();
        let elsewhere = [
            ("trader", PW, Elsewhere),
            ("trader", "Wrong@123", Elsewhere),
            ("someone", PW, Elsewhere),
        ];
        check(&m, &kdb, "keychain, elsewhere", &elsewhere, 2, &|o| {
            matches!(o, Opened::MoveInAppWindow)
        });
        assert_eq!(store.calls(), 0, "a sign-in elsewhere read the keychain");
        check(
            &m,
            &kdb,
            "keychain, app window",
            &not_proven[..2],
            2,
            &wrong,
        );
        store.inner.set_unavailable(true);
        check(
            &m,
            &kdb,
            "keychain locked",
            &[
                ("trader", PW, AppWindow),
                ("trader", "Wrong@123", AppWindow),
                ("someone", PW, AppWindow),
            ],
            2,
            &|o| matches!(o, Opened::Unreadable(AppError::KeychainUnavailable)),
        );

        // vault.json, then vault.next.json beside it.
        let dir = tempfile::tempdir().unwrap();
        let keys = Unlocked::random().unwrap();
        let fdb = earlier_install(dir.path(), &keys);
        write_vault_file_for_test(&dir.path().join(VAULT_FILE), &keys, PW);
        let m = SecurityManager::open(dir.path(), Arc::new(CountingKeyStore::default())).unwrap();
        check(&m, &fdb, "vault.json", &not_proven[..2], 2, &wrong);
        write_vault_file_for_test(&dir.path().join(VAULT_NEXT_FILE), &keys, "Next@1234");
        check(&m, &fdb, "two vault files", &not_proven[..2], 4, &wrong);

        // secrets.dat.
        let dir = tempfile::tempdir().unwrap();
        let keys = Unlocked::random().unwrap();
        let ldb = earlier_install(dir.path(), &keys);
        legacy::write_for_test(dir.path(), keys.0.raw_key.expose(), keys.0.pepper.expose());
        let m = SecurityManager::open(dir.path(), Arc::new(CountingKeyStore::default())).unwrap();
        check(&m, &ldb, "secrets.dat", &not_proven[..2], 2, &wrong);
    }

    #[test]
    fn unlocked_keys_never_print() {
        let k = Unlocked::random().unwrap();
        assert_eq!(format!("{:?}", k), "Unlocked([REDACTED])");
        assert_eq!(k, k.clone());
        assert_ne!(k, Unlocked::random().unwrap());
    }
}
