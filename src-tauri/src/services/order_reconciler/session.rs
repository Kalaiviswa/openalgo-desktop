//! Whose book a pass read (NEW-TR-01): the Live session it was read
//! through, the same before and after the read, travels with the snapshot,
//! so an owner settles from it only orders of that session. A read during
//! which the session changed is set aside, counted, and read again next pass.

use super::{BookSnapshot, OrderReconciler};
use crate::services::live_owner::LiveOwner;
use crate::strategy::dispatch::RunMode;
use std::sync::atomic::Ordering;

impl BookSnapshot {
    /// The Live session the book was read through.
    pub fn with_session(self, session: Option<LiveOwner>) -> Self {
        Self { session, ..self }
    }
}

impl OrderReconciler {
    /// The Live session `mode`'s book is read through now.
    pub(super) fn session_of(&self, mode: RunMode) -> Option<LiveOwner> {
        (mode == RunMode::Live)
            .then(|| LiveOwner::active(self.gateway.as_ref()))
            .flatten()
    }

    /// Whether the Live session changed since `before`, during a book read:
    /// whose book it was is not known, so nothing is settled from it.
    pub(super) fn session_changed(&self, mode: RunMode, before: &Option<LiveOwner>) -> bool {
        if self.session_of(mode) == *before {
            return false;
        }
        let discarded = &self.stats.book_reads_discarded;
        discarded.fetch_add(1, Ordering::Relaxed);
        tracing::info!(
            "The broker session changed while the order book was read; it was set aside"
        );
        true
    }
}
