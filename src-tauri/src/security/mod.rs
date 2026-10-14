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
//! A password change rewrites the hash and the wrapping in one SQLite
//! transaction (`change_password`). A forgotten password is replaced with
//! new keys, dropping what the old ones encrypted (`replace_keys`).
//!
//! # Moving the keys of an earlier build, once
//!
//! Earlier builds kept the keys in the OS keychain, in `vault.json` (no
//! keychain), or XOR-ed in `secrets.dat`. The first sign-in with this build
//! reads them from there (the keychain only for the account's own name, the
//! one last prompt), proves them on the stored hash and on an encrypted
//! row, copies them into the vault in a transaction that is verified before
//! and after its commit, and only then deletes the source. A stop at any
//! step leaves a state the next sign-in finishes; the keychain is never read
//! once a vault row exists.

pub mod crypto;
pub mod fsperm;
pub mod hashing;
pub mod keystore;
pub mod legacy;
pub mod secret;
pub mod totp;

use crate::db::sqlite::key_vault::{self, KeychainCleanup, Source, VaultRow};
use crate::db::sqlite::{user, SqliteDb};
use crate::error::{AppError, Result};
use base64::Engine;
use crypto::{Aad, DataCipher};
use keystore::{KeyStore, KeyStoreError, DATA_KEY, PEPPER};
use parking_lot::{Mutex, RwLock};
use rand::RngCore;
use rusqlite::Connection;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use secret::{Secret, SecretBytes};

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

/// What a sign-in's password opened.
#[derive(Debug)]
pub enum Opened {
    Keys(Unlocked),
    WrongPassword,
}

/// Test faults: the steps of a move at which a test stops it, as a crash
/// would.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveStep {
    /// The old keys are read and proved; nothing is written.
    Read,
    /// The vault row is written in the open transaction.
    BeforeCommit,
    /// The transaction is committed.
    AfterCommit,
    /// The committed row was verified; the source is not deleted yet.
    AfterVerify,
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

    // ------------------------------------------------------------- sign-in

    /// Open the keys with `password` for the account `username`, moving
    /// them into the vault first when an earlier build kept them elsewhere.
    /// Nothing is installed: the caller installs them with the session.
    ///
    /// The password must open the vault and match the stored hash (read
    /// again under the lock every change holds); anything less is
    /// `WrongPassword` and leaves everything as it was. Errors:
    /// `KeychainUnavailable` (the keychain of an earlier build could not be
    /// read; retry or start fresh), `KeysDoNotMatch` (no keys there, or keys
    /// that open none of the account's data), `VaultUnreadable`.
    pub fn open_for_sign_in(
        &self,
        db: &SqliteDb,
        username: &str,
        password: &str,
    ) -> Result<Opened> {
        let _change = self.vault_lock.lock();
        let (account, row) = {
            let conn = db.conn()?;
            (
                user::find_by_username(&conn, username)?,
                key_vault::read(&conn)?,
            )
        };
        let Some(account) = account else {
            return Ok(Opened::WrongPassword);
        };
        let hash = account.password_hash.as_str();
        if let Some(row) = row {
            let keys = match key_vault::unwrap(&row, password) {
                Ok(Some((k, p))) => Keys::new(k, p)?,
                Ok(None) => return Ok(Opened::WrongPassword),
                Err(e) => {
                    tracing::error!("The key vault could not be read: {}", e);
                    return Err(AppError::VaultUnreadable);
                }
            };
            if !hashing::verify_password(keys.pepper.expose(), password, hash)? {
                return Ok(Opened::WrongPassword);
            }
            self.tidy_after_move(db, row.keychain_cleanup);
            return Ok(Opened::Keys(Unlocked(Arc::new(keys))));
        }
        // No vault yet: an install from an earlier build.
        let source = {
            let conn = db.conn()?;
            self.key_source(&conn)?
        };
        let (keys, src, cleanup) = match source {
            KeySource::Vault | KeySource::NoAccount => return Ok(Opened::WrongPassword),
            KeySource::VaultFile => match self.open_vault_files(password, hash)? {
                Some(k) => (k, Source::VaultFile, KeychainCleanup::None),
                None => return Ok(Opened::WrongPassword),
            },
            KeySource::LegacyFile => {
                let Some((k, p)) = legacy::read(&self.data_dir)? else {
                    return Ok(Opened::WrongPassword);
                };
                let keys = Keys::new(k, p)?;
                if !hashing::verify_password(keys.pepper.expose(), password, hash)? {
                    return Ok(Opened::WrongPassword);
                }
                (keys, Source::LegacyFile, KeychainCleanup::None)
            }
            KeySource::Keychain => {
                let keys = self.read_keychain()?;
                if !hashing::verify_password(keys.pepper.expose(), password, hash)? {
                    // A wrong password, or a keychain holding another
                    // install's keys: the account's data tells which.
                    let witness = {
                        let conn = db.conn()?;
                        key_vault::witness(&conn)?
                    };
                    return match witness {
                        Some(w) if !keys.opens(&w) => Err(AppError::KeysDoNotMatch),
                        _ => Ok(Opened::WrongPassword),
                    };
                }
                (keys, Source::Keychain, KeychainCleanup::Pending)
            }
        };
        self.move_into_vault(db, keys, password, src, cleanup)
            .map(Opened::Keys)
    }

    /// The keys in `vault.json`, else in `vault.next.json`, whichever this
    /// password opens with a pepper the stored hash verifies with (a
    /// password change an earlier build left half done keeps the file that
    /// matches the hash). None when neither does.
    fn open_vault_files(&self, password: &str, hash: &str) -> Result<Option<Keys>> {
        for name in [VAULT_FILE, VAULT_NEXT_FILE] {
            let path = self.data_dir.join(name);
            if !path.exists() {
                continue;
            }
            let keys = match open_vault_file(&path, password) {
                Ok(Some(k)) => k,
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!("Could not read {}: {}", name, e);
                    if name == VAULT_FILE {
                        return Err(AppError::VaultUnreadable);
                    }
                    continue;
                }
            };
            if hashing::verify_password(keys.pepper.expose(), password, hash)? {
                return Ok(Some(keys));
            }
        }
        Ok(None)
    }

    /// The keys an earlier build kept in the OS keychain: the one last
    /// read. A locked or refusing keychain is `KeychainUnavailable`; missing
    /// entries are `KeysDoNotMatch`.
    fn read_keychain(&self) -> Result<Keys> {
        let read = |name: &str| match self.store.get(name) {
            Ok(v) => Ok(v),
            Err(KeyStoreError::Unavailable(e)) | Err(KeyStoreError::Failure(e)) => {
                tracing::warn!("The keychain could not be read ({}): {}", name, e);
                Err(AppError::KeychainUnavailable)
            }
        };
        let data_key = read(DATA_KEY)?;
        let pepper = read(PEPPER)?;
        match (data_key, pepper) {
            (Some(k), Some(p)) => Keys::new(decode_b64(&k)?, decode_b64(&p)?),
            (k, p) => {
                tracing::warn!(
                    "The keychain holds {} of the account's two keys",
                    usize::from(k.is_some()) + usize::from(p.is_some())
                );
                Err(AppError::KeysDoNotMatch)
            }
        }
    }

    /// Copy `keys` into a new vault row wrapped with `password`, verify the
    /// row inside the transaction and again after the commit, then delete
    /// the source. Caller holds `vault_lock`.
    fn move_into_vault(
        &self,
        db: &SqliteDb,
        keys: Keys,
        password: &str,
        source: Source,
        cleanup: KeychainCleanup,
    ) -> Result<Unlocked> {
        // The data key must open the account's data before anything is
        // written: keys from somewhere else are never moved.
        let witness = {
            let conn = db.conn()?;
            key_vault::witness(&conn)?
        };
        if let Some(w) = &witness {
            if !keys.opens(w) {
                tracing::error!("The keys found for the account do not open its data");
                return Err(AppError::KeysDoNotMatch);
            }
        }
        self.fault_at(MoveStepAt::Read)?;
        let row = VaultRow::new(keys.wrap(password)?, source, cleanup);
        {
            let conn = db.conn()?;
            with_full_sync(&conn, || {
                conn.execute_batch("BEGIN IMMEDIATE")?;
                let written = (|| -> Result<()> {
                    key_vault::put(&conn, &row)?;
                    self.fault_at(MoveStepAt::BeforeCommit)?;
                    verify_vault(&conn, password, &keys)
                })();
                match written {
                    Ok(()) => {
                        conn.execute_batch("COMMIT")?;
                        Ok(())
                    }
                    Err(e) => {
                        let _ = conn.execute_batch("ROLLBACK");
                        Err(e)
                    }
                }
            })?;
        }
        self.fault_at(MoveStepAt::AfterCommit)?;
        {
            let conn = db.conn()?;
            verify_vault(&conn, password, &keys)?;
        }
        self.fault_at(MoveStepAt::AfterVerify)?;
        tracing::info!(
            "Keys moved into openalgo.db from {}",
            match source {
                Source::Keychain => "the system keychain",
                Source::VaultFile => "vault.json",
                Source::LegacyFile => "secrets.dat",
                _ => "an earlier build",
            }
        );
        self.tidy_after_move(db, cleanup);
        Ok(Unlocked(Arc::new(keys)))
    }

    /// Delete what a move copied from, once the vault row is committed and
    /// verified: `vault.json` and `vault.next.json` at once, keychain
    /// entries once (`pending` -> `done`, or `failed`: never retried, the
    /// keychain is not touched again). `secrets.dat` goes after the stored
    /// rows it encrypted without associated data are re-encrypted
    /// (`finish_legacy_migration`). Failures are logged: the vault works
    /// either way.
    fn tidy_after_move(&self, db: &SqliteDb, cleanup: KeychainCleanup) {
        let mut removed = false;
        for name in [VAULT_FILE, VAULT_NEXT_FILE] {
            let p = self.data_dir.join(name);
            if p.exists() {
                match std::fs::remove_file(&p) {
                    Ok(()) => removed = true,
                    Err(e) => tracing::warn!("Could not delete the old {}: {}", name, e),
                }
            }
        }
        if removed {
            fsperm::sync_dir(&self.data_dir);
        }
        if cleanup != KeychainCleanup::Pending {
            return;
        }
        let deleted = self
            .store
            .delete(PEPPER)
            .and_then(|_| self.store.delete(DATA_KEY));
        let state = match deleted {
            Ok(()) => {
                tracing::info!("Deleted the old keychain entries; the keychain is not used again");
                KeychainCleanup::Done
            }
            Err(e) => {
                tracing::warn!(
                    "Could not delete the old keychain entries ({}); they are left unused",
                    e
                );
                KeychainCleanup::Failed
            }
        };
        if let Err(e) = db
            .conn()
            .and_then(|c| key_vault::set_keychain_cleanup(&c, state))
        {
            tracing::warn!("Could not record the keychain clean-up: {}", e);
        }
    }

    /// Remove `secrets.dat` once the vault holds its keys and the stored
    /// rows were re-encrypted with associated data.
    pub fn finish_legacy_migration(&self, db: &SqliteDb) -> Result<()> {
        if !legacy::path(&self.data_dir).exists() {
            return Ok(());
        }
        let vaulted = {
            let conn = db.conn()?;
            key_vault::exists(&conn)?
        };
        if vaulted {
            legacy::remove(&self.data_dir)?;
            fsperm::sync_dir(&self.data_dir);
        }
        Ok(())
    }

    // ---------------------------------------------------- account setup

    /// Account setup: new keys, the vault and the account written by
    /// `create` (with a manager holding the new keys) in one transaction.
    /// The keys are not installed: the setup page sends the trader to sign
    /// in, and the keys come into memory with that session, like every
    /// other time (only the test harness, which opened unlocked, keeps its
    /// own). Any file an earlier build left (`vault.json`, `secrets.dat`)
    /// belongs to no account and is deleted.
    pub fn create_account(
        &self,
        db: &SqliteDb,
        password: &str,
        create: impl FnOnce(&Connection, &SecurityManager) -> Result<()>,
    ) -> Result<()> {
        let _change = self.vault_lock.lock();
        // New keys; the test harness's keys when it opened unlocked (the
        // only way keys are in memory before an account exists).
        let harness = self.keys.read().clone();
        let keys = match &harness {
            Some(k) => Unlocked(k.clone()),
            None => Unlocked::random()?,
        };
        let with_keys = keys.manager();
        let row = VaultRow::new(keys.0.wrap(password)?, Source::Setup, KeychainCleanup::None);
        {
            let conn = db.conn()?;
            with_full_sync(&conn, || {
                transaction(&conn, || {
                    if user::has_user(&conn)? {
                        return Err(AppError::Validation(
                            "An account already exists on this computer. Sign in instead.".into(),
                        ));
                    }
                    // Nothing encrypted can belong to an account that does
                    // not exist yet.
                    key_vault::drop_encrypted_values(&conn)?;
                    key_vault::put(&conn, &row)?;
                    create(&conn, &with_keys)
                })
            })?;
        }
        self.bump_credentials_epoch();
        drop(keys);
        self.remove_old_sources();
        Ok(())
    }

    fn remove_old_sources(&self) {
        for p in [
            self.data_dir.join(VAULT_FILE),
            self.data_dir.join(VAULT_NEXT_FILE),
            legacy::path(&self.data_dir),
        ] {
            if p.exists() {
                if let Err(e) = std::fs::remove_file(&p) {
                    tracing::warn!("Could not delete an old key file: {}", e);
                }
            }
        }
        fsperm::sync_dir(&self.data_dir);
    }

    // ------------------------------------------------- password change

    /// Change the password of the account `id`: the new hash (under the same
    /// pepper) and the keys wrapped with the new password, in ONE
    /// transaction, so no state exists in which one changed and the other
    /// did not. Applies only while the stored hash is still `old_hash`, the
    /// one the request was checked against (SEC-02b); otherwise
    /// `stale` is returned as the error. The keys must be unlocked (the
    /// trader is signed in) and stay installed.
    pub fn change_password(
        &self,
        db: &SqliteDb,
        id: i64,
        old_hash: &str,
        new_password: &str,
        stale: impl FnOnce() -> AppError,
    ) -> Result<()> {
        let _change = self.vault_lock.lock();
        let keys = self.keys()?;
        let new_hash = hashing::hash_password(keys.pepper.expose(), new_password)?;
        let wrapped = keys.wrap(new_password)?;
        let conn = db.conn()?;
        with_full_sync(&conn, || {
            transaction(&conn, || {
                let n = conn.execute(
                    "UPDATE users SET password_hash = ?1, updated_at = datetime('now')
                     WHERE id = ?2 AND password_hash = ?3",
                    rusqlite::params![new_hash, id, old_hash],
                )?;
                if n != 1 {
                    return Err(stale());
                }
                #[cfg(test)]
                if self.fail_rewrap.load(std::sync::atomic::Ordering::SeqCst) {
                    return Err(AppError::Io(std::io::Error::other("test fault")));
                }
                key_vault::rewrap(&conn, &wrapped)
            })
        })?;
        self.bump_credentials_epoch();
        Ok(())
    }

    // ---------------------------------------------------- fresh start

    /// Replace the keys: new random keys wrapped with `new_password`, every
    /// value the old keys encrypted dropped, and the account rebuilt by
    /// `rebuild` (a manager holding the new keys: the new hash, TOTP secret
    /// and API key), in one transaction. Then the new vault is unwrapped
    /// and its keys compared (verification), the keys are locked, and the
    /// files of earlier builds are deleted. The keychain is never touched:
    /// entries left there by an earlier build are marked `abandoned`.
    pub fn replace_keys(
        &self,
        db: &SqliteDb,
        new_password: &str,
        rebuild: impl FnOnce(&Connection, &SecurityManager) -> Result<()>,
    ) -> Result<()> {
        let _change = self.vault_lock.lock();
        let keys = Unlocked::random()?;
        let with_keys = keys.manager();
        {
            let conn = db.conn()?;
            let cleanup = match key_vault::read(&conn)? {
                Some(r) if r.keychain_cleanup != KeychainCleanup::Pending => r.keychain_cleanup,
                Some(_) => KeychainCleanup::Abandoned,
                None => match self.key_source(&conn)? {
                    KeySource::Keychain => KeychainCleanup::Abandoned,
                    _ => KeychainCleanup::None,
                },
            };
            let row = VaultRow::new(keys.0.wrap(new_password)?, Source::Fresh, cleanup);
            with_full_sync(&conn, || {
                transaction(&conn, || {
                    key_vault::drop_encrypted_values(&conn)?;
                    key_vault::put(&conn, &row)?;
                    rebuild(&conn, &with_keys)
                })
            })?;
            verify_vault(&conn, new_password, &keys.0)?;
        }
        *self.keys.write() = None;
        self.bump_credentials_epoch();
        self.remove_old_sources();
        Ok(())
    }

    /// Account reset: the vault row goes in the reset's own transaction
    /// (`key_vault::delete`); this locks the keys and deletes the files of
    /// earlier builds. The next account setup makes new keys.
    pub fn forget_keys(&self) {
        self.lock();
        self.remove_old_sources();
    }

    // ------------------------------------------------------------ test hooks

    #[cfg(test)]
    pub fn fail_move_at(&self, step: Option<MoveStep>) {
        *self.fault.lock() = step;
    }

    /// Test fault: the vault write of a password change fails after the
    /// hash was written in the same transaction.
    #[cfg(test)]
    pub fn fail_rewrap(&self, on: bool) {
        self.fail_rewrap
            .store(on, std::sync::atomic::Ordering::SeqCst);
    }

    #[allow(unused_variables)]
    fn fault_at(&self, step: MoveStepAt) -> Result<()> {
        #[cfg(test)]
        {
            let hit = matches!(
                (*self.fault.lock(), step),
                (Some(MoveStep::Read), MoveStepAt::Read)
                    | (Some(MoveStep::BeforeCommit), MoveStepAt::BeforeCommit)
                    | (Some(MoveStep::AfterCommit), MoveStepAt::AfterCommit)
                    | (Some(MoveStep::AfterVerify), MoveStepAt::AfterVerify)
            );
            if hit {
                return Err(AppError::Internal("test stop".into()));
            }
        }
        Ok(())
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
}

/// The steps `fault_at` is called at (release builds compile it away).
#[derive(Clone, Copy)]
enum MoveStepAt {
    Read,
    BeforeCommit,
    AfterCommit,
    AfterVerify,
}

/// The vault row on `conn` unwraps with `password` to exactly `keys`, and
/// the first encrypted row opens with them.
fn verify_vault(conn: &Connection, password: &str, keys: &Keys) -> Result<()> {
    let failed = |why: &str| {
        tracing::error!("The key vault did not verify: {}", why);
        AppError::Encryption("the key vault did not verify".into())
    };
    let row = key_vault::read(conn)?.ok_or_else(|| failed("no row"))?;
    let (k, p) = key_vault::unwrap(&row, password)?.ok_or_else(|| failed("does not open"))?;
    if k != keys.raw_key || p != keys.pepper {
        return Err(failed("other keys"));
    }
    if let Some(w) = key_vault::witness(conn)? {
        if !keys.opens(&w) {
            return Err(failed("the data does not open"));
        }
    }
    Ok(())
}

/// Run `f` inside `BEGIN IMMEDIATE`, committing on success and rolling back
/// on any error.
fn transaction<T>(conn: &Connection, f: impl FnOnce() -> Result<T>) -> Result<T> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    match f() {
        Ok(v) => {
            conn.execute_batch("COMMIT")?;
            Ok(v)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// Run `f` on `conn` with `synchronous = FULL`, so what it commits is on
/// disk when it returns (WAL with NORMAL may lose the last commits at a
/// power cut), then put the connection's own setting back. For key changes,
/// whose source is deleted right after.
pub(crate) fn with_full_sync<T>(conn: &Connection, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let before: i64 = conn.query_row("PRAGMA synchronous", [], |r| r.get(0))?;
    conn.execute_batch("PRAGMA synchronous = FULL")?;
    let out = f();
    if let Err(e) = conn.execute_batch(&format!("PRAGMA synchronous = {}", before)) {
        tracing::warn!("Could not restore the database sync setting: {}", e);
    }
    out
}

#[derive(Deserialize)]
struct VaultFile {
    salt: String,
    nonce: String,
    ciphertext: String,
}

/// The keys in an earlier build's vault file at `path`, or None when
/// `password` does not open it.
fn open_vault_file(path: &Path, password: &str) -> Result<Option<Keys>> {
    let damaged = || AppError::Encryption("vault file is damaged".into());
    let raw = std::fs::read(path)?;
    let vault: VaultFile = serde_json::from_slice(&raw).map_err(|_| damaged())?;
    let row = VaultRow::new(
        key_vault::Wrapped {
            salt: vault.salt,
            nonce: vault.nonce,
            ciphertext: vault.ciphertext,
        },
        Source::VaultFile,
        KeychainCleanup::None,
    );
    match key_vault::unwrap(&row, password)? {
        Some((k, p)) => Ok(Some(Keys::new(k, p)?)),
        None => Ok(None),
    }
}

/// The test harness's keys, kept in its in-memory store (made on first
/// use), as the keychain mode of earlier builds kept them.
fn harness_keys(store: &dyn KeyStore) -> Result<Keys> {
    let get = |n: &str| store.get(n).map_err(|e| AppError::Keychain(e.to_string()));
    if let (Some(k), Some(p)) = (get(DATA_KEY)?, get(PEPPER)?) {
        return Keys::new(decode_b64(&k)?, decode_b64(&p)?);
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
        let opened = m.open_for_sign_in(&db, "trader", "First@123").unwrap();
        assert!(matches!(opened, Opened::Keys(_)));
        assert!(!m.is_unlocked(), "a sign-in's password step installed keys");
        assert!(matches!(
            m.open_for_sign_in(&db, "trader", "Wrong@123").unwrap(),
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
        let Opened::Keys(keys) = m.open_for_sign_in(&db, "trader", "First@123").unwrap() else {
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

    #[test]
    fn unlocked_keys_never_print() {
        let k = Unlocked::random().unwrap();
        assert_eq!(format!("{:?}", k), "Unlocked([REDACTED])");
        assert_eq!(k, k.clone());
        assert_ne!(k, Unlocked::random().unwrap());
    }
}
