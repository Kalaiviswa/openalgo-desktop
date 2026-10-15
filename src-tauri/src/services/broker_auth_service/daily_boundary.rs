//! The daily boundary (SES-01, SES-02): what ends when the 03:00 IST
//! boundary passes, and what does not.

use super::{BrokerAuthService, PendingRevoke};
use crate::brokers::catalog;
use crate::db::sqlite::auth;
use crate::error::Result;
use crate::events::{Event, SessionEndReason};
use crate::state::AppState;

impl BrokerAuthService {
    /// The daily boundary (SES-02): end only what was authenticated before
    /// `boundary`, so a sign-in made after it (before the next 30-second
    /// poll, or right after a wake from sleep) keeps its session, its
    /// streaming and its stored row. Continuous (crypto) sessions are not
    /// ended (SES-01). Runs under the session lock, so no sign-in or resume
    /// completes meanwhile; the in-memory session is taken only if it is
    /// from before the boundary, and streaming is torn down only while the
    /// runtime still runs a session from before it (the freshness check in
    /// `get_broker_session` may already have dropped the memory copy).
    ///
    /// The stored rows from before the boundary are revoked by time. When
    /// that write fails, the revoke is left pending like a failed sign-out
    /// (SEC-05b: in memory and on disk across a restart; no resume until it
    /// is written), recorded as the boundary's (`PendingRevoke::Before`):
    /// its retry revokes only what this write would have, never a session
    /// signed in after the boundary or a crypto one. Returns whether a live
    /// session ended.
    pub async fn expire_before(
        state: &AppState,
        boundary: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool> {
        let daily = catalog::has_daily_boundary;
        let _held = Self::session_lock(state).await;
        let taken = {
            let mut live = state.broker_session.write();
            if live
                .as_ref()
                .is_some_and(|s| s.authenticated_at < boundary && daily(&s.broker_id))
            {
                live.take()
            } else {
                None
            }
        };
        let torn_down = state.runtime.teardown_started_before(state, boundary).await;
        let ended = taken.is_some() || torn_down;
        if ended {
            state.api_keys.clear();
            state.clear_symbol_cache();
            state.bus.publish(Event::BrokerSessionEnded {
                reason: SessionEndReason::DailyExpiry,
            });
        }
        let written = state
            .sqlite
            .conn()
            .and_then(|conn| auth::revoke_before(&conn, boundary, |b| !daily(b)));
        if let Err(e) = written {
            let mut pending = state.revoke_pending.lock();
            Self::mark_pending_locked(state, &mut pending, PendingRevoke::Before(boundary));
            return Err(e);
        }
        Ok(ended)
    }
}
