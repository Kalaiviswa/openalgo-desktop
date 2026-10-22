//! What the tick path needs of a run that does not change while it runs,
//! kept in memory so no tick waits on the database.
//!
//! Every tick of a held symbol used to read the run row and its strategy
//! (two synchronous SQLite reads on the async worker) and render the
//! strategy to a dict before evaluating; at a few hundred held legs the
//! tick consumer fell behind the feed. A running strategy's configuration
//! cannot change (edits, the Live opt-in and deletion are refused while it
//! runs; the webhook lock and token are not read here), so it is read
//! once per run, on the blocking pool, and kept until the run ends, as the
//! scalping monitor keeps its legs. A run that has ended is not in the live
//! state registry, so it is never evaluated; its entry is forgotten when it
//! finishes and pruned with every new one.
//!
//! The daily loss limit's banked P&L (the realized P&L of the strategy's
//! other runs this session) cannot change during the run either (one run
//! per strategy at a time); it is read once per session, also off the
//! async worker.
//!
//! Both are read when the run's prices are subscribed (`follow_prices`),
//! because the tick reader decides every price itself: a read on a run's
//! first price would hold the reader, and a burst could overrun the
//! market-data channel behind it.

use super::dispatch::RunMode;
use super::store::{RunRow, StrategyRow};
use super::StrategyModule;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// One running run's fixed inputs to the tick path.
pub struct TickContext {
    pub strategy: StrategyRow,
    /// `strategy.to_dict(false)`, what the risk core reads.
    pub strategy_dict: Value,
    pub mode: RunMode,
    /// The banked P&L and the session it was read for.
    banked: Mutex<Option<(DateTime<Utc>, f64)>>,
}

/// The contexts of the running runs, bounded by the live state registry.
#[derive(Default)]
pub struct TickContexts {
    map: Mutex<HashMap<i64, Arc<TickContext>>>,
}

impl TickContexts {
    fn get(&self, run_id: i64) -> Option<Arc<TickContext>> {
        self.map.lock().get(&run_id).cloned()
    }

    /// Keep one, dropping those of runs no longer running.
    fn keep(&self, run_id: i64, ctx: Arc<TickContext>, running: &[i64]) {
        let mut map = self.map.lock();
        map.retain(|id, _| running.contains(id));
        map.insert(run_id, ctx);
    }

    /// Forget a finished run's context.
    pub fn forget(&self, run_id: i64) {
        self.map.lock().remove(&run_id);
    }

    /// Contexts held (hygiene tests).
    pub fn len(&self) -> usize {
        self.map.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl StrategyModule {
    /// Subscribe the run's instruments and read what its ticks need now,
    /// so its first price finds them in memory.
    pub(crate) async fn follow_prices(&self, run_id: i64, symbols: &[(String, String)]) {
        self.feed.add_run(run_id, symbols).await;
        if let Some(ctx) = self.tick_context(run_id).await {
            self.session_loss(&ctx, run_id).await;
        }
    }

    /// The run's tick context, read on the blocking pool the first time.
    /// `None` when the run has ended or cannot be read (the tick is not
    /// evaluated, as before: a read failure was never a decision).
    pub(crate) async fn tick_context(&self, run_id: i64) -> Option<Arc<TickContext>> {
        if let Some(ctx) = self.tick_contexts.get(run_id) {
            return Some(ctx);
        }
        let store = self.store.clone();
        let read = tokio::task::spawn_blocking(move || -> crate::error::Result<_> {
            let Some(run) = store.get_run(run_id)? else {
                return Ok(None);
            };
            let strategy = store.get_strategy_unscoped(run.strategy_id)?;
            Ok(strategy.map(|s| (run, s)))
        })
        .await;
        let (run, strategy): (RunRow, StrategyRow) = match read {
            Ok(Ok(Some(found))) => found,
            Ok(Ok(None)) => return None,
            Ok(Err(e)) => {
                tracing::error!(
                    "Run {} was not evaluated: it could not be read: {}",
                    run_id,
                    e
                );
                return None;
            }
            Err(e) => {
                tracing::error!("Run {} was not evaluated: {}", run_id, e);
                return None;
            }
        };
        if run.stopped_at.is_some() {
            return None;
        }
        let ctx = Arc::new(TickContext {
            strategy_dict: strategy.to_dict(false),
            mode: self.mode_of(&run.mode),
            strategy,
            banked: Mutex::new(None),
        });
        self.tick_contexts
            .keep(run_id, ctx.clone(), &self.state.active_run_ids());
        Some(ctx)
    }

    /// The strategy's daily loss limit and, when it has one, the realized
    /// P&L its other runs banked earlier this session (read once per
    /// session, off the async worker; a read that fails counts nothing
    /// banked, as before, and is read again on the next tick).
    pub(crate) async fn session_loss(
        &self,
        ctx: &TickContext,
        run_id: i64,
    ) -> (Option<f64>, Option<f64>) {
        let limit = ctx
            .strategy
            .daily_loss_limit_inr
            .map(f64::abs)
            .filter(|l| *l > 0.0);
        if limit.is_none() {
            return (None, None);
        }
        let since =
            super::session::session_started_at(self.clock.now(), self.session.0, self.session.1);
        if let Some((at, banked)) = *ctx.banked.lock() {
            if at == since {
                return (limit, Some(banked));
            }
        }
        let store = self.store.clone();
        let strategy_id = ctx.strategy.id;
        let read = tokio::task::spawn_blocking(move || {
            store.realized_pnl_since(strategy_id, since, Some(run_id))
        })
        .await;
        let banked = match read {
            Ok(Ok(b)) => {
                *ctx.banked.lock() = Some((since, b));
                b
            }
            Ok(Err(e)) => {
                tracing::error!("Banked P&L of strategy {} not read: {}", strategy_id, e);
                0.0
            }
            Err(e) => {
                tracing::error!("Banked P&L of strategy {} not read: {}", strategy_id, e);
                0.0
            }
        };
        (limit, Some(banked))
    }
}
