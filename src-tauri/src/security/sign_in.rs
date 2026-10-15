//! Signing in against the key vault (KEY-01): what a password opens, and
//! what the caller may learn when it opens nothing.
//!
//! The rule from the security review: until the password is proven, a
//! sign-in says nothing about the name or the password. So the install's
//! key state is read first, without either (`key_state`); every way out
//! costs the same Argon2 work (`hashing::pad_argon2_runs`); only a sign-in
//! in the app window (`SignInFrom::AppWindow`) reads an earlier build's
//! keychain or moves keys; and the route answers `Unreadable` and
//! `FailedAfterProof` with the ways out to the app window only.

use super::*;

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

impl SecurityManager {
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
