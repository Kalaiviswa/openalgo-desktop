//! What the owners of orders say about their own orders, asked on the
//! blocking pool (PROD-REL-02). Owners answer from their saved rows (the
//! OpenScript runner's and the scalping page's unconfirmed orders, the
//! strategy runs' orders), and the reconciler asks every pass, every few
//! seconds, from a task on an async worker: the worker that carries the
//! feed and the risk owners. A pool waiting for a free connection held that
//! worker for the whole wait. Which of a book's orders are already someone's
//! is asked once per read, and the matching reads the answer from memory.

use super::{slot, BookSnapshot, OrderOwner, MODES};
use crate::strategy::dispatch::RunMode;
use std::collections::HashSet;
use std::sync::Arc;

type Owners = Vec<Arc<dyn OrderOwner>>;

/// `ask` over `owners` on the blocking pool; `unknown` when it could not
/// finish.
async fn off_runtime<T: Send + 'static>(
    owners: &[Arc<dyn OrderOwner>],
    unknown: T,
    ask: impl FnOnce(&[Arc<dyn OrderOwner>]) -> T + Send + 'static,
) -> T {
    let owners = owners.to_vec();
    tokio::task::spawn_blocking(move || ask(&owners))
        .await
        .unwrap_or_else(|e| {
            tracing::error!("The owners of open orders could not be asked: {}", e);
            unknown
        })
}

/// The owners holding working or unconfirmed orders, per destination
/// (indexed by `slot`). Unknown, every owner is taken to hold some: a book
/// is read rather than an order left unsettled.
pub(super) async fn holding(owners: &[Arc<dyn OrderOwner>]) -> [Owners; 2] {
    let every = owners.to_vec();
    off_runtime(owners, [every.clone(), every], |owners| {
        let mut by: [Owners; 2] = Default::default();
        for mode in MODES {
            by[slot(mode)] = owners
                .iter()
                .filter(|o| o.has_open_orders(mode))
                .cloned()
                .collect();
        }
        by
    })
    .await
}

/// Whether any owner holds working or unconfirmed orders in `mode`.
/// Unknown counts as yes, so a lost order fact still opens its gap.
pub(super) async fn any_holding(owners: &[Arc<dyn OrderOwner>], mode: RunMode) -> bool {
    off_runtime(owners, true, move |owners| {
        owners.iter().any(|o| o.has_open_orders(mode))
    })
    .await
}

/// Whether no owner holds an order for the trader. Unknown counts as no,
/// so the held-order alert stays until a pass can tell.
pub(super) async fn none_held(owners: &[Arc<dyn OrderOwner>]) -> bool {
    off_runtime(owners, false, |owners| {
        owners.iter().all(|o| o.held_orders().is_empty())
    })
    .await
}

/// The ids among `ids` that some owner already holds. Unknown, every one
/// counts as held: no placement is bound to an order that may be another's.
pub(super) async fn owned(owners: &[Arc<dyn OrderOwner>], ids: Vec<String>) -> HashSet<String> {
    if ids.is_empty() {
        return HashSet::new();
    }
    let every = ids.iter().cloned().collect();
    off_runtime(owners, every, move |owners| {
        owners.iter().flat_map(|o| o.owned_among(&ids)).collect()
    })
    .await
}

/// Fold one book read by every owner holding orders in `mode`. Answers
/// how many placements stay unresolved, and whether every owner could read
/// its own orders: one that could not settled nothing, so the gap stays.
pub(super) async fn fold(
    holders: &[Arc<dyn OrderOwner>],
    mode: RunMode,
    book: &BookSnapshot,
) -> (usize, bool) {
    let (mut unresolved, mut folded) = (0, true);
    for o in holders {
        match o.reconcile(mode, book).await {
            Ok(n) => unresolved += n,
            Err(e) => {
                tracing::error!(
                    "The {} orders could not be read to reconcile the {} order book: {}",
                    o.name(),
                    mode.as_str(),
                    e
                );
                folded = false;
            }
        }
    }
    (unresolved, folded)
}

impl BookSnapshot {
    /// Ask `owners`, on the blocking pool, which of this read's orders are
    /// already theirs ([`OrderOwner::owned_among`]), once: matching then
    /// reads it from memory, never an owner's saved rows on the pass's
    /// async worker.
    pub async fn owned_by(self, owners: &[Arc<dyn OrderOwner>]) -> Self {
        let ids = self.rows.iter().map(|r| r.order_id.clone()).collect();
        *self.owned.lock() = owned(owners, ids).await;
        self
    }

    /// An owner bound this book order to one of its placements: no other
    /// placement is matched to it in this pass.
    pub fn claim(&self, order_id: &str) {
        self.owned.lock().insert(order_id.to_string());
    }
}
