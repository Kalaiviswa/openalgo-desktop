//! Every change of the keys or of the password hash besides a sign-in's
//! move (KEY-01): account setup, a password change (the hash and the
//! wrapping in one SQLite transaction), a fresh start with new keys, and an
//! account reset. Each runs under the vault lock and bumps the credentials
//! generation, so a sign-in checked against the old state gets no session.

use super::*;

impl SecurityManager {
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
            super::migration::verify_vault(&conn, new_password, &keys.0)?;
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

    /// Test fault: the vault write of a password change fails after the
    /// hash was written in the same transaction.
    #[cfg(test)]
    pub fn fail_rewrap(&self, on: bool) {
        self.fail_rewrap
            .store(on, std::sync::atomic::Ordering::SeqCst);
    }
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
