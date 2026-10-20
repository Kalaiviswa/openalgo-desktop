//! The batches of intents being sent, owned by the runner (NEW-TR-04).
//!
//! A batch runs in a task of its own so that a page's request dropped while
//! an order is at the broker cannot cut the placement off between sending
//! and saving its answer. The task is still owned: kept in one set, reaped
//! as batches finish, counted with the runner's other tasks, and waited for
//! at shutdown, so a quit never abandons an order mid-placement. A batch is
//! not aborted on Pause or Stop for the same reason: a Stop takes the run's
//! order lock, so it waits for the batch in flight, and a Pause leaves it to
//! finish and save what the broker said.

use super::Runner;
use serde_json::Value;
use std::future::Future;
use std::time::Duration;

/// The longest shutdown waits for batches in flight: a placement's own
/// request is bounded well inside it. What is left after it is aborted.
const DRAIN: Duration = Duration::from_secs(20);

impl Runner {
    /// Run one batch in the runner's set; its answer comes back on the
    /// receiver (closed when the batch was aborted).
    pub(super) fn spawn_batch(
        &self,
        batch: impl Future<Output = Value> + Send + 'static,
    ) -> tokio::sync::oneshot::Receiver<Value> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut set = self.batches.lock();
        while set.try_join_next().is_some() {}
        set.spawn(async move {
            // The page may have gone: its answer is then not needed.
            let _ = tx.send(batch.await);
        });
        rx
    }

    /// Batches still running.
    pub(super) fn batches_alive(&self) -> usize {
        let mut set = self.batches.lock();
        while set.try_join_next().is_some() {}
        set.len()
    }

    /// Wait for the batches in flight to finish, up to [`DRAIN`]; abort what
    /// is left after it.
    pub(super) async fn drain_batches(&self) {
        let mut set = std::mem::take(&mut *self.batches.lock());
        let drained =
            tokio::time::timeout(DRAIN, async { while set.join_next().await.is_some() {} }).await;
        if drained.is_err() {
            tracing::warn!(
                "{} OpenScript order batch(es) still at the broker at shutdown were stopped",
                set.len()
            );
            set.abort_all();
            while set.join_next().await.is_some() {}
        }
    }
}
