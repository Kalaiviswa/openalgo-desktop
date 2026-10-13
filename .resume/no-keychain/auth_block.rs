    /// Store a new password: the hash and the keys wrapped with it, in one
    /// transaction (KEY-01; `SecurityManager::change_password`), so there is
    /// no state in which one changed and the other did not.
    ///
    /// `old_hash` is the hash the caller checked the request against. The
    /// change applies only while it is still the stored one, checked in the
    /// same statement that writes the new hash, under the vault lock: a
    /// second change validated against the same old password, finishing
    /// after the first, is refused instead of overwriting it (SEC-02b).
    /// Every browser session ends and every cache is cleared after the
    /// commit; the keys do not change, so they stay in memory.
    pub(crate) fn set_password(state: &AppState, id: i64, old_hash: &str, new: &str) -> Result<()> {
        state
            .security
            .change_password(&state.sqlite, id, old_hash, new, || {
                AppError::Validation(PASSWORD_CHANGED_MEANWHILE.into())
            })?;
        state.sessions.clear();
        Self::clear_caches(state);
        Ok(())
    }

    /// Every cache a change of credentials makes stale (the web's
    /// `invalidate_user_cache`): API keys and order mode, instrument facts.
    /// Run after the change is committed.
    pub fn clear_caches(state: &AppState) {
        state.api_keys.clear();
        state.trading.facts.clear();
    }

    /// A new password without the old one, which starts fresh: new keys
    /// wrapped with the new password, every value the old keys encrypted
    /// dropped (saved broker keys, stored broker sessions, the Telegram bot
    /// token and linked users' keys, the WhatsApp pairing), and a new API
    /// key (order mode kept) and TOTP secret (every "require the
    /// authenticator" switch off), in one transaction. The account, its
    /// settings and its history stay. The keys are locked afterwards: the
    /// trader signs in with the new password.
    ///
    /// Used for a forgotten password and for an install whose earlier
    /// build's keys cannot be read. Only the desktop window calls it (the
    /// `start_fresh` command, after a native confirmation); no HTTP route
    /// reaches it.
    pub fn start_fresh(state: &AppState, new: &str, confirm: &str) -> Result<()> {
        if new != confirm {
            return Err(AppError::Validation("The passwords do not match.".into()));
        }
        validate_password_strength(new).map_err(|m| AppError::Validation(m.into()))?;
        let row = {
            let conn = state.sqlite.conn()?;
            user::find_first(&conn)?
        }
        .ok_or_else(|| {
            AppError::Validation(
                "There is no account on this computer yet. Create one instead.".into(),
            )
        })?;
        let secret = totp::generate_secret();
        let key = api_keys::generate_api_key();
        state.security.replace_keys(&state.sqlite, new, |conn, with| {
            let hash = with.hash_password(new)?;
            conn.execute(
                "UPDATE users SET password_hash = ?1, updated_at = datetime('now') WHERE id = ?2",
                rusqlite::params![hash, row.id],
            )?;
            user::set_totp_secret(conn, with, &row.username, &secret)?;
            api_keys::upsert_for_user(conn, with, &row.username, &key)?;
            Ok(())
        })?;
        // The stored broker sessions are gone, so nothing is left to revoke.
        crate::services::broker_auth_service::BrokerAuthService::clear_pending_revoke(state);
        state.sessions.clear();
        state.set_broker_session(None);
        Self::clear_caches(state);
        tracing::info!("Started fresh with a new password; the old encrypted values were dropped");
        Ok(())
    }

    /// The fresh start as the desktop window runs it: end the live broker
    /// session and the bots, start fresh, clear every adapter's state, then
    /// sign every window out.
    pub async fn start_fresh_everywhere(
        ctx: &std::sync::Arc<AppState>,
        new: &str,
        confirm: &str,
    ) -> Result<()> {
        if new != confirm {
            return Err(AppError::Validation("The passwords do not match.".into()));
        }
        validate_password_strength(new).map_err(|m| AppError::Validation(m.into()))?;
        if let Err(e) = crate::services::broker_auth_service::BrokerAuthService::revoke(
            ctx,
            crate::events::SessionEndReason::Logout,
        )
        .await
        {
            tracing::warn!(
                "Fresh start: the broker session ended; its stored row could not be revoked: {}",
                e
            );
        }
        ctx.messaging.telegram.stop(ctx).await;
        ctx.messaging.whatsapp.stop_bot(ctx).await;
        let c = ctx.clone();
        let (n, f) = (new.to_string(), confirm.to_string());
        tokio::task::spawn_blocking(move || Self::start_fresh(&c, &n, &f))
            .await
            .map_err(|e| AppError::Internal(format!("fresh start task: {}", e)))??;
        Self::forget_adapter_state(ctx).await;
        ctx.bus.publish(crate::events::Event::ForceLogout {
            message: "A new password was set on this computer. Sign in with it.".into(),
        });
        Ok(())
    }

    /// Log out: after every browser session ended and the broker session
    /// was revoked, stop the bots, lock the keys, and clear every cache and
    /// every adapter's in-memory state (KEY-01). Until the next sign-in
    /// nothing can be decrypted.
    pub async fn lock_everything(ctx: &std::sync::Arc<AppState>) {
        ctx.messaging.telegram.stop(ctx).await;
        ctx.messaging.whatsapp.stop_bot(ctx).await;
        ctx.security.lock();
        Self::clear_caches(ctx);
        ctx.clear_symbol_cache();
        Self::forget_adapter_state(ctx).await;
        tracing::info!("Keys locked until the next sign-in");
    }

    /// `on_logout` of every broker adapter, not only the logged-in one: an
    /// adapter a sign-in or resume touched but never completed may still
    /// hold keys or tokens (XTS market keys, OTP tokens).
    async fn forget_adapter_state(ctx: &AppState) {
        for b in ctx.brokers.list() {
            b.on_logout().await;
        }
    }

