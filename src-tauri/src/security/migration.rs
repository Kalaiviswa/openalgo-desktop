//! Moving an earlier build's keys into the vault, once (KEY-01).
//!
//! Earlier builds kept the keys in the OS keychain, in `vault.json` (no
//! keychain), or XOR-ed in `secrets.dat`. The first sign-in in the app
//! window with this build reads them from there, proves them on the stored
//! hash and on an encrypted row (`sign_in`), copies them into the vault in a
//! transaction that is verified before and after its commit, and only then
//! deletes the source. A stop at any step leaves a state the next sign-in
//! finishes; the keychain is never read once a vault row exists, and never
//! written.

use super::*;

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

impl SecurityManager {
    /// Copy `keys` into a new vault row wrapped with `password`, verify the
    /// row inside the transaction and again after the commit, then delete
    /// the source. Caller holds `vault_lock`.
    pub(super) fn move_into_vault(
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
    pub(super) fn tidy_after_move(
        &self,
        db: &SqliteDb,
        cleanup: KeychainCleanup,
        from: SignInFrom,
    ) {
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

    #[cfg(test)]
    pub fn fail_move_at(&self, step: Option<MoveStep>) {
        *self.fault.lock() = step;
    }

    /// Stop a move at `step` when a test asked for it, as a crash would.
    #[cfg(test)]
    fn fault_at(&self, step: MoveStepAt) -> Result<()> {
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
        Ok(())
    }

    /// Release builds have no test faults.
    #[cfg(not(test))]
    fn fault_at(&self, _step: MoveStepAt) -> Result<()> {
        Ok(())
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
pub(super) fn verify_vault(conn: &Connection, password: &str, keys: &Keys) -> Result<()> {
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
