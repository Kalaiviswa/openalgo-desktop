//! The monitor's writes to a stop's row: its exit attempt and its trailed
//! stop. Each is written on the blocking pool, after the monitor's lock is
//! released and never awaited by the tick that called for it (PROD-REL-02):
//! a busy database (another writer, no free pooled connection) holds
//! neither the ticks nor another leg's breach. A leg's writes of one kind
//! take turns under that leg's writer lock (`LegWriters`), so the row ends
//! as memory has it.

use super::{update_payload, ExitAttempt, Gen, RiskMonitor, SL_UPDATE_EVENT};
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// The locks that order one leg's writes, a kind apart from the other so
/// a trail save never delays an exit's.
#[derive(Default)]
pub(super) struct LegWriters {
    attempt: tokio::sync::Mutex<()>,
    trail: tokio::sync::Mutex<()>,
}

/// A trail save in flight, counted until it ends, aborted or not
/// (`RiskMonitor::wait_idle`).
pub(super) struct TrailSave(Arc<RiskMonitor>);

impl TrailSave {
    pub(super) fn new(monitor: Arc<RiskMonitor>) -> Self {
        monitor.trail_saves.fetch_add(1, Ordering::SeqCst);
        Self(monitor)
    }

    /// Save the leg's trailed stop (`RiskMonitor::save_trail`).
    pub(super) async fn run(self, key: String, gen: Gen, emit: bool) {
        self.0.save_trail(&key, gen, emit).await;
    }
}

impl Drop for TrailSave {
    fn drop(&mut self) {
        self.0.trail_saves.fetch_sub(1, Ordering::SeqCst);
    }
}

impl RiskMonitor {
    /// The locks that order one leg's writes (kept while anyone holds or
    /// awaits them; pruned with the leg).
    fn writers(&self, key: &str) -> Arc<LegWriters> {
        self.inner
            .lock()
            .writers
            .entry(key.to_string())
            .or_default()
            .clone()
    }

    /// Change a leg's attempt in memory and write it to the stop's row, in
    /// turn with every other change of that leg's attempt, so two writers (a
    /// reconcile pass folding fills, an exit adding an order, the trader's
    /// "not placed", a restored attempt saved again) never overwrite each
    /// other's orders: the row always ends as memory has it. The turn is
    /// the leg's writer lock, never the monitor's. Answers the attempt as it
    /// now stands and whether the row was written (memory has the change
    /// either way).
    pub(super) async fn update_attempt(
        &self,
        key: &str,
        row_id: i64,
        change: impl FnOnce(&mut Option<ExitAttempt>),
    ) -> (Option<ExitAttempt>, bool) {
        let writers = self.writers(key);
        let _turn = writers.attempt.lock().await;
        let slot = {
            let mut inner = self.inner.lock();
            let mut slot = inner.attempts.remove(key);
            change(&mut slot);
            if let Some(a) = &slot {
                inner.attempts.insert(key.to_string(), a.clone());
            }
            slot
        };
        let saved = match &slot {
            Some(a) => self.persist_attempt(row_id, a).await,
            None => true,
        };
        (slot, saved)
    }

    /// Write a leg's attempt to its row; false when it could not be saved
    /// (then no further exit order is sent for it).
    async fn persist_attempt(&self, row_id: i64, a: &ExitAttempt) -> bool {
        let Some(text) = a.to_json() else {
            tracing::error!("Scalping exit orders for a leg could not be encoded");
            return false;
        };
        let store = self.store.clone();
        let written =
            tokio::task::spawn_blocking(move || store.set_exit_attempt(row_id, Some(&text))).await;
        match written {
            Ok(Ok(true)) => true,
            Ok(Ok(false)) => {
                tracing::warn!(
                    "Scalping stop row {} is gone; its exit orders were not saved",
                    row_id
                );
                false
            }
            Ok(Err(e)) => {
                tracing::error!("Scalping exit orders for a leg could not be saved: {}", e);
                false
            }
            Err(e) => {
                tracing::error!("Scalping exit orders for a leg could not be saved: {}", e);
                false
            }
        }
    }

    /// Save a leg's trailed stop as memory has it when its turn comes, then
    /// push it to the terminal when `emit`. A later move may have overtaken
    /// the one that asked: the newest is written, never an older one over
    /// it, and nothing for a stop replaced or cleared meanwhile.
    async fn save_trail(&self, key: &str, gen: Gen, emit: bool) {
        let writers = self.writers(key);
        let _turn = writers.trail.lock().await;
        let newest = self
            .inner
            .lock()
            .states
            .get(key)
            .filter(|l| l.gen == gen)
            .map(|l| l.sl.clone());
        let Some(sl) = newest else {
            return;
        };
        let (store, row) = (self.store.clone(), sl.clone());
        match tokio::task::spawn_blocking(move || store.update_trail(&row)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::debug!("Trailing stop for {} not saved: {}", sl.symbol, e),
            Err(e) => tracing::debug!("Trailing stop for {} not saved: {}", sl.symbol, e),
        }
        if emit {
            self.ui.emit(SL_UPDATE_EVENT, update_payload(&sl)).await;
        }
    }
}
