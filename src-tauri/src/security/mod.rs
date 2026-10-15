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

/// Where a sign-in comes from, as far as the keys are concerned (KEY-01
/// security review).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignInFrom {
    /// The OpenAlgo Desktop window on this computer, shown by the proof
    /// only the shell hands out ([`window::WindowProof`]): it may read an
    /// earlier build's keychain once and move keys, and is told why keys
    /// could not be opened.
    AppWindow,
    /// Anything else: a browser or a program on this computer, another
    /// device, a tunnel.
    Elsewhere,
}

/// What a sign-in's password opened.
#[derive(Debug)]
pub enum Opened {
    /// The password opened the keys and matches the stored hash.
    Keys(Unlocked),
    /// Not proven: a wrong password, or a name that is not the account's.
    WrongPassword,
    /// Not proven, whatever was typed: an earlier build's keys still have
    /// to move, and only a sign-in in the app window moves them.
    MoveInAppWindow,
    /// Not proven, whatever was typed: this install's keys cannot be read
    /// (`KeychainUnavailable`, `KeysDoNotMatch`, `VaultUnreadable`, or the
    /// database failing). The same for every name and password.
    Unreadable(AppError),
    /// The password was proven, then opening or moving the keys failed.
    FailedAfterProof(AppError),
}

/// Argon2 runs every sign-in costs, whatever it finds: opening the vault
/// with the password and checking the password hash.
pub const SIGN_IN_ARGON2_RUNS: u32 = 2;

/// This install's keys as a sign-in finds them before it looks at the
/// name or the password.
enum KeyState {
    NoAccount,
    /// The vault row, checked without a password.
    Vault(VaultRow),
    /// An earlier build's `vault.json` (and `vault.next.json`), checked
    /// without a password.
    VaultFiles(Vec<(&'static str, VaultRow)>),
    /// Keys an earlier build kept in `secrets.dat` or the OS keychain, read
    /// and proven on the account's data.
    Earlier {
        keys: Box<Keys>,
        source: Source,
        cleanup: KeychainCleanup,
    },
    /// Keys still to move, for a sign-in that is not the app window's.
    MoveInAppWindow,
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

    // ------------------------------------------------------------- sign-in

    /// Open the keys with `password` for the account `username`, moving
    /// them into the vault first when an earlier build kept them elsewhere.
    /// Nothing is installed: the caller installs them with the session.
    ///
    /// What a caller can learn (KEY-01 security review): until the password
    /// is proven, nothing about the name or the password. The install's keys
    /// are read first, without either (`key_state`), so keys that cannot be
    /// read answer the same whatever was typed; every way out costs the same
    /// Argon2 work (`hashing::pad_argon2_runs`); only a sign-in in the app
    /// window reads an earlier build's keychain or moves keys. The route
    /// decides who hears `Unreadable` and `FailedAfterProof`: the app window
    /// gets the ways out, anyone else the answer to a wrong password.
    pub fn open_for_sign_in(
        &self,
        db: &SqliteDb,
        username: &str,
        password: &str,
        from: SignInFrom,
    ) -> Opened {
        let _change = self.vault_lock.lock();
        let since = hashing::argon2_runs();
        let mut work = SIGN_IN_ARGON2_RUNS;
        let opened = self.open_locked(db, username, password, from, &mut work);
        hashing::pad_argon2_runs(since, work, password);
        opened
    }

    fn open_locked(
        &self,
        db: &SqliteDb,
        username: &str,
        password: &str,
        from: SignInFrom,
        work: &mut u32,
    ) -> Opened {
        let state = match self.key_state(db, from, work) {
            Ok(KeyState::NoAccount) => return Opened::WrongPassword,
            Ok(KeyState::MoveInAppWindow) => return Opened::MoveInAppWindow,
            Ok(s) => s,
            Err(e) => return Opened::Unreadable(e),
        };
        let account = match db.conn().and_then(|c| user::find_by_username(&c, username)) {
            Ok(Some(a)) => a,
            Ok(None) => return Opened::WrongPassword,
            Err(e) => return Opened::Unreadable(e),
        };
        let hash = account.password_hash.as_str();
        // Checks the hash with keys a password already opened (a vault or a
        // vault file): from there on the password is proven.
        let proven_hash =
            |keys: &Keys| match hashing::verify_password(keys.pepper.expose(), password, hash) {
                Ok(ok) => Ok(ok),
                Err(e) => {
                    tracing::error!("The stored password hash could not be checked: {}", e);
                    Err(Opened::FailedAfterProof(AppError::VaultUnreadable))
                }
            };
        match state {
            KeyState::Vault(row) => {
                let keys = match key_vault::unwrap(&row, password) {
                    Ok(None) => return Opened::WrongPassword,
                    Ok(Some((k, p))) => Keys::new(k, p),
                    Err(e) => Err(e),
                };
                let keys = match keys {
                    Ok(k) => k,
                    Err(e) => {
                        tracing::error!("The key vault opened but its keys are unusable: {}", e);
                        return Opened::FailedAfterProof(AppError::VaultUnreadable);
                    }
                };
                match proven_hash(&keys) {
                    Ok(true) => {}
                    Ok(false) => return Opened::WrongPassword,
                    Err(o) => return o,
                }
                self.tidy_after_move(db, row.keychain_cleanup, from);
                Opened::Keys(Unlocked(Arc::new(keys)))
            }
            KeyState::VaultFiles(files) => {
                for (name, row) in files {
                    let keys = match key_vault::unwrap(&row, password) {
                        Ok(None) => continue,
                        Ok(Some((k, p))) => Keys::new(k, p),
                        Err(e) => Err(e),
                    };
                    let keys = match keys {
                        Ok(k) => k,
                        Err(e) => {
                            tracing::error!("{} opened but its keys are unusable: {}", name, e);
                            return Opened::FailedAfterProof(AppError::VaultUnreadable);
                        }
                    };
                    match proven_hash(&keys) {
                        Ok(true) => {
                            return self.move_proven(
                                db,
                                keys,
                                password,
                                Source::VaultFile,
                                KeychainCleanup::None,
                            )
                        }
                        Ok(false) => continue,
                        Err(o) => return o,
                    }
                }
                Opened::WrongPassword
            }
            KeyState::Earlier {
                keys,
                source,
                cleanup,
            } => match hashing::verify_password(keys.pepper.expose(), password, hash) {
                Ok(true) => self.move_proven(db, *keys, password, source, cleanup),
                Ok(false) => Opened::WrongPassword,
                Err(e) => {
                    // Nothing proven yet: a wrong password, whoever asks.
                    tracing::error!("The stored password hash could not be checked: {}", e);
                    Opened::WrongPassword
                }
            },
            KeyState::NoAccount | KeyState::MoveInAppWindow => Opened::WrongPassword,
        }
    }

    /// This install's keys as they stand, read without the name or the
    /// password: whatever this finds or fails on is the same for every name
    /// and password typed. Sets `work` to the Argon2 runs every sign-in in
    /// this state costs (two per vault file an earlier build left).
    ///
    /// Keys an earlier build kept elsewhere are read only for the app
    /// window, and must open the account's data (the first encrypted row)
    /// before any password is looked at: another install's keys are never
    /// moved, whatever was typed.
    fn key_state(&self, db: &SqliteDb, from: SignInFrom, work: &mut u32) -> Result<KeyState> {
        let (account, row, source) = {
            let conn = db.conn()?;
            (
                user::find_first(&conn)?,
                key_vault::read(&conn)?,
                self.key_source(&conn)?,
            )
        };
        let Some(account) = account else {
            return Ok(KeyState::NoAccount);
        };
        if source != KeySource::Vault && from != SignInFrom::AppWindow {
            return Ok(KeyState::MoveInAppWindow);
        }
        if !hashing::hash_is_readable(&account.password_hash) {
            tracing::error!("The stored password hash is damaged");
            return Err(AppError::VaultUnreadable);
        }
        match (source, row) {
            (KeySource::Vault, Some(row)) => {
                if let Err(e) = key_vault::check(&row) {
                    tracing::error!("The key vault cannot be read: {}", e);
                    return Err(AppError::VaultUnreadable);
                }
                Ok(KeyState::Vault(row))
            }
            (KeySource::VaultFile, _) => {
                let files = self.read_vault_files()?;
                *work = SIGN_IN_ARGON2_RUNS.saturating_mul(files.len() as u32);
                Ok(KeyState::VaultFiles(files))
            }
            (KeySource::LegacyFile, _) => {
                let keys = match legacy::read(&self.data_dir) {
                    Ok(Some((k, p))) => Keys::new(k, p),
                    Ok(None) => Err(AppError::VaultUnreadable),
                    Err(e) => Err(e),
                }
                .map_err(|e| {
                    tracing::error!("secrets.dat cannot be read: {}", e);
                    AppError::VaultUnreadable
                })?;
                self.prove_on_data(db, &keys, AppError::VaultUnreadable)?;
                Ok(KeyState::Earlier {
                    keys: Box::new(keys),
                    source: Source::LegacyFile,
                    cleanup: KeychainCleanup::None,
                })
            }
            (KeySource::Keychain, _) => {
                let keys = self.read_keychain()?;
                self.prove_on_data(db, &keys, AppError::KeysDoNotMatch)?;
                Ok(KeyState::Earlier {
                    keys: Box::new(keys),
                    source: Source::Keychain,
                    cleanup: KeychainCleanup::Pending,
                })
            }
            _ => Ok(KeyState::NoAccount),
        }
    }

    /// Keys an earlier build kept must open the account's data, checked
    /// before the password: `otherwise` when they open none of it.
    fn prove_on_data(&self, db: &SqliteDb, keys: &Keys, otherwise: AppError) -> Result<()> {
        let witness = {
            let conn = db.conn()?;
            key_vault::witness(&conn)?
        };
        match witness {
            Some(w) if !keys.opens(&w) => {
                tracing::warn!("The keys found for the account open none of its data");
                Err(otherwise)
            }
            _ => Ok(()),
        }
    }

    /// `vault.json`, then `vault.next.json` (a password change an earlier
    /// build left half done), each checked without a password. A damaged
    /// `vault.json` makes the keys unreadable; a damaged `vault.next.json`
    /// is left out.
    fn read_vault_files(&self) -> Result<Vec<(&'static str, VaultRow)>> {
        let mut files = Vec::new();
        for name in [VAULT_FILE, VAULT_NEXT_FILE] {
            let path = self.data_dir.join(name);
            if !path.exists() {
                continue;
            }
            match read_vault_file(&path) {
                Ok(row) => files.push((name, row)),
                Err(e) => {
                    tracing::warn!("{} cannot be read: {}", name, e);
                    if name == VAULT_FILE {
                        return Err(AppError::VaultUnreadable);
                    }
                }
            }
        }
        if files.is_empty() {
            return Err(AppError::VaultUnreadable);
        }
        Ok(files)
    }

    /// The keys an earlier build kept in the OS keychain: the one last
    /// read. A locked or refusing keychain is `KeychainUnavailable`; missing
    /// or unreadable entries are `KeysDoNotMatch`. What the keychain said goes
    /// to the process log only, never into the error log the Diagnostics
    /// page serves (KEY-01 security review): it can carry the keychain's
    /// item names and the system's own text.
    fn read_keychain(&self) -> Result<Keys> {
        let read = |name: &str| match self.store.get(name) {
            Ok(v) => Ok(v),
            Err(e) => {
                tracing::warn!(
                    "The system keychain could not be read; the keys stay where they are"
                );
                tracing::info!("System keychain: {}", e);
                Err(AppError::KeychainUnavailable)
            }
        };
        let data_key = read(DATA_KEY)?;
        let pepper = read(PEPPER)?;
        match (data_key, pepper) {
            (Some(k), Some(p)) => Keys::new(decode_b64(k.expose())?, decode_b64(p.expose())?)
                .map_err(|_| AppError::KeysDoNotMatch),
            (k, p) => {
                tracing::warn!(
                    "The system keychain holds {} of the account's two keys",
                    usize::from(k.is_some()) + usize::from(p.is_some())
                );
                Err(AppError::KeysDoNotMatch)
            }
        }
    }

    /// Keys an earlier build kept, proven by the password: move them into
    /// the vault. A failure here comes after the proof; only the app window
    /// moves keys, and only it is told.
    fn move_proven(
        &self,
        db: &SqliteDb,
        keys: Keys,
        password: &str,
        source: Source,
        cleanup: KeychainCleanup,
    ) -> Opened {
        match self.move_into_vault(db, keys, password, source, cleanup) {
            Ok(k) => Opened::Keys(k),
            Err(AppError::KeysDoNotMatch) if source != Source::Keychain => {
                Opened::FailedAfterProof(AppError::VaultUnreadable)
            }
            Err(e) => {
                tracing::error!("The keys could not be moved into openalgo.db: {}", e);
                Opened::FailedAfterProof(e)
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
        self.tidy_after_move(db, cleanup, SignInFrom::AppWindow);
        Ok(Unlocked(Arc::new(keys)))
    }

    /// Delete what a move copied from, once the vault row is committed and
    /// verified: `vault.json` and `vault.next.json` at once, keychain
    /// entries once (`pending` -> `done`, or `failed`: never retried, the
    /// keychain is not touched again), and only at a sign-in in the app
    /// window, the one place a keychain prompt belongs. `secrets.dat` goes
    /// after the stored rows it encrypted without associated data are
    /// re-encrypted (`finish_legacy_migration`). Failures are logged: the
    /// vault works either way.
    fn tidy_after_move(&self, db: &SqliteDb, cleanup: KeychainCleanup, from: SignInFrom) {
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
        if cleanup != KeychainCleanup::Pending || from != SignInFrom::AppWindow {
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
                tracing::warn!("Could not delete the old keychain entries; they are left unused");
                tracing::info!("System keychain: {}", e);
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

/// An earlier build's vault file at `path`, checked without a password.
fn read_vault_file(path: &Path) -> Result<VaultRow> {
    let damaged = || AppError::Encryption("vault file is damaged".into());
    let raw = zeroize::Zeroizing::new(std::fs::read(path)?);
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
    key_vault::check(&row)?;
    Ok(row)
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
