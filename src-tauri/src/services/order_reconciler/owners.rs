//! What the owners of orders say about their own orders, asked on the
//! blocking pool (PROD-REL-02). Owners answer from their saved rows (the
//! OpenScript runner's and the scalping page's unconfirmed orders, the
//! strategy runs' orders), and the reconciler asks every pass, every few
//! seconds, from a task on an async worker: the worker that carries the
//! feed and the risk owners. A pool waiting for a free connection held that
//! worker for the whole wait.

use super::{slot, OrderOwner, MODES};
use crate::strategy::dispatch::RunMode;
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
