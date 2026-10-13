import sys
src=open(sys.argv[1]).read()
def rep(old,new):
    global src
    assert old in src, old[:80]
    src=src.replace(old,new,1)
def cut(a_mark,b_mark,new):
    global src
    a=src.index(a_mark); b=src.index(b_mark)
    src=src[:a]+new+src[b:]
rep('''use crate::db::sqlite::{api_keys, auth, credentials, user};
use crate::error::{AppError, Result};
use crate::security::{totp, KeyMode, Secret};
use crate::services::apikey_service::ApiKeyService;
use crate::state::AppState;''','''use crate::db::sqlite::{api_keys, auth, credentials, user};
use crate::error::{AppError, Result};
use crate::security::{totp, Opened, Secret, SecurityManager, Unlocked};
use crate::state::AppState;''')
cut('/// Run `f` on `conn` with `synchronous=FULL`','/// Pepper of the stand-in hash','''pub(crate) use crate::security::with_full_sync;

''')
rep('''#[derive(Debug, PartialEq, Eq)]
pub enum LoginOutcome {
    Success(String),
    TotpRequired(String),
    Invalid,
}''','''/// What a password sign-in came to. The keys the password opened travel
/// with it: they are installed only with the signed-in session
/// (`SecurityManager::install`, inside `issue_if_current`), so a sign-in
/// still waiting for its authenticator code unlocks nothing (KEY-01).
#[derive(Debug, PartialEq, Eq)]
pub enum LoginOutcome {
    Success(String, Unlocked),
    TotpRequired(String, Unlocked),
    Invalid,
}''')
cut('''        validate_password_strength(password).map_err(|m| AppError::Validation(m.into()))?;
        // Password mode: the first password creates the key vault.''','''    /// Check a username and password. Does not create a web session.''','''        validate_password_strength(password).map_err(|m| AppError::Validation(m.into()))?;
        // The keys, their vault, the account and its API key in one
        // transaction (KEY-01). Creating the account counts as its first
        // sign-in: the keys stay in memory until the trader logs out.
        let secret = totp::generate_secret();
        let key = api_keys::generate_api_key();
        state
            .security
            .create_account(&state.sqlite, password, |conn, with| {
                let hash = with.hash_password(password)?;
                user::insert(conn, with, username, email.trim(), &hash, &secret)?;
                api_keys::upsert_for_user(conn, with, username, &key)?;
                Ok(())
            })?;
        state.api_keys.clear();
        tracing::info!("Account created");
        Ok(())
    }

''')
cut('''    /// Check a username and password. Does not create a web session.''','''    /// Accounts created by earlier desktop builds have no TOTP secret.''','''    /// Check a username and password. Does not create a web session and
    /// installs no keys: the keys the password opened come back in the
    /// outcome, for the caller to install with the session.
    ///
    /// The first sign-in of an install from an earlier build moves its keys
    /// into openalgo.db (`SecurityManager::open_for_sign_in`): from the OS
    /// keychain (the one last read), `vault.json` or `secrets.dat`. That can
    /// fail with `KeychainUnavailable` (retry or start fresh),
    /// `KeysDoNotMatch` or `VaultUnreadable` (start fresh).
    pub fn verify_credentials(
        state: &AppState,
        username: &str,
        password: &str,
    ) -> Result<LoginOutcome> {
        let row = {
            let conn = state.sqlite.conn()?;
            user::find_by_username(&conn, username)?
        };
        let Some(row) = row else {
            // A name that is not the account's is checked against a fixed
            // stand-in hash with the account's Argon2 parameters: the same
            // work, the same answer and (in the caller) the same budget as
            // a wrong password, so nothing tells which names exist. The
            // keychain of an earlier build is never read for it.
            STAND_IN_CHECKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let _ = crate::security::hashing::verify_password(
                stand_in_pepper(),
                password,
                stand_in_hash(),
            );
            return Ok(LoginOutcome::Invalid);
        };
        #[cfg(test)]
        test_hooks::after_account_read(state);
        // The password must open the vault and match the stored hash, read
        // again under the lock a password change holds (SEC-02).
        let keys = match state
            .security
            .open_for_sign_in(&state.sqlite, &row.username, password)?
        {
            Opened::Keys(k) => k,
            Opened::WrongPassword => return Ok(LoginOutcome::Invalid),
        };
        let with = keys.manager();
        // Rows an early build stored without associated data are
        // re-encrypted now that the keys are open (a no-op once done).
        crate::db::sqlite::data_migrations::run(&state.sqlite, &with)?;
        state.security.finish_legacy_migration(&state.sqlite)?;
        Self::ensure_totp_secret(state, &with, &row)?;
        if row.is_totp_required_for("login") {
            Ok(LoginOutcome::TotpRequired(row.username, keys))
        } else {
            Ok(LoginOutcome::Success(row.username, keys))
        }
    }

''')
rep('''    fn ensure_totp_secret(state: &AppState, row: &user::UserRow) -> Result<()> {
        if row.totp_secret(&state.security)?.is_none() {
            let conn = state.sqlite.conn()?;
            user::set_totp_secret(
                &conn,
                &state.security,
                &row.username,
                &totp::generate_secret(),
            )?;
        }
        Ok(())
    }''','''    fn ensure_totp_secret(
        state: &AppState,
        with: &SecurityManager,
        row: &user::UserRow,
    ) -> Result<()> {
        if row.totp_secret(with)?.is_none() {
            let conn = state.sqlite.conn()?;
            user::set_totp_secret(&conn, with, &row.username, &totp::generate_secret())?;
        }
        Ok(())
    }''')
cut('''    pub fn verify_totp_by_email(''','''    pub fn change_password(''','''    /// The authenticator code of a sign-in still waiting for it, checked
    /// with the keys its password opened (not installed yet).
    pub fn verify_totp_with(
        state: &AppState,
        username: &str,
        code: &str,
        keys: &Unlocked,
    ) -> Result<bool> {
        let row = {
            let conn = state.sqlite.conn()?;
            user::find_by_username(&conn, username)?
        };
        let Some(row) = row else { return Ok(false) };
        let Some(secret) = row.totp_secret(&keys.manager())? else {
            return Ok(false);
        };
        let now = state.now().timestamp().max(0) as u64;
        Ok(totp::verify(secret.expose(), code, now))
    }

''')
cut('''    /// Reset by email + TOTP (the web's TOTP path; no email step on desktop).''','''    pub fn profile(''',open('auth_block.rs').read())
rep('''                conn.execute("DELETE FROM whatsapp_users", [])?;
                crate::messaging::whatsapp::db::clear_session(&conn)?;
                Ok(())
            })();''','''                conn.execute("DELETE FROM whatsapp_users", [])?;
                crate::messaging::whatsapp::db::clear_session(&conn)?;
                // The keys go with the account; the next setup makes new
                // ones (KEY-01).
                crate::db::sqlite::key_vault::delete(&conn)?;
                Ok(())
            })();''')
rep('''        // The account is gone whatever the key rotation does: every window
        // is signed out and nothing cached survives, even when the keychain
        // refuses the new keys (the error is still returned).
        let rotated = state.security.rotate();
        state.api_keys.clear();
        state.sessions.clear();
        state.set_broker_session(None);
        rotated''','''        // The account and its keys are gone: the keys are locked, every
        // window is signed out and nothing cached survives.
        state.security.forget_keys();
        Self::clear_caches(state);
        state.sessions.clear();
        state.set_broker_session(None);
        Ok(())''')
rep('''            .map_err(|e| AppError::Internal(format!("account reset task: {}", e)))??;
        ctx.bus.publish(crate::events::Event::ForceLogout {''','''            .map_err(|e| AppError::Internal(format!("account reset task: {}", e)))??;
        Self::forget_adapter_state(ctx).await;
        ctx.bus.publish(crate::events::Event::ForceLogout {''')
open(sys.argv[2],'w').write(src)
