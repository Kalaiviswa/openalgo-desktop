//! Periodic durability for live runs (web
//! `services/strategy_module/checkpoint.py`).
//!
//! Every 5 s one `sm_strategy_checkpoint` row per live run, so a crash costs
//! seconds, not a run. The snapshot is taken under the run lock (in-memory
//! only) and written after release. Rows are pruned to the newest 200 per run
//! every 120 passes, which bounds a table written all day in a process that
//! never restarts. Nothing starts at construction; one owned task.

use super::store::StrategyRow;
use super::StrategyModule;
use crate::error::Result;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(5);
pub const CHECKPOINT_KEEP: i64 = 200;
pub const PRUNE_EVERY_PASSES: u64 = 120;

static PASSES: AtomicU64 = AtomicU64::new(0);

/// One pass: a checkpoint per live run. Returns rows written.
pub fn write_once(module: &StrategyModule, prune: Option<bool>) -> usize {
    let prune =
        prune.unwrap_or_else(|| PASSES.fetch_add(1, Ordering::Relaxed) + 1 >= PRUNE_EVERY_PASSES);
    if prune {
        PASSES.store(0, Ordering::Relaxed);
    }
    let mut written = 0;
    for run_id in module.state.active_run_ids() {
        let Some(snapshot) = module
            .state
            .with_run(run_id, |r| r.snapshot_for_checkpoint())
        else {
            continue;
        };
        match module.store.write_checkpoint(run_id, &snapshot) {
            Ok(()) => written += 1,
            Err(e) => tracing::error!("Could not checkpoint run {}: {}", run_id, e),
        }
        if prune {
            if let Err(e) = module.store.prune_checkpoints(run_id, CHECKPOINT_KEEP) {
                tracing::error!("Could not prune checkpoints for run {}: {}", run_id, e);
            }
        }
    }
    written
}

impl StrategyModule {
    /// The checkpoint page the Detail page reads, `{"data", "run_id"}`: of
    /// the `requested` run, else the current run, else the latest one.
    /// `Ok(None)` when the requested run is not this strategy's. A failed
    /// read is an error, never an empty page that reads as "never ran"
    /// (SM-04).
    pub fn checkpoint_page(
        &self,
        strategy: &StrategyRow,
        requested: Option<i64>,
    ) -> Result<Option<Value>> {
        let run_id = match requested {
            Some(id) => match self.store.get_run(id)? {
                Some(run) if run.strategy_id == strategy.id => Some(id),
                _ => return Ok(None),
            },
            None => match strategy.current_run_id {
                Some(id) => Some(id),
                None => self
                    .store
                    .list_runs(strategy.id, 1)?
                    .first()
                    .and_then(|r| r["id"].as_i64()),
            },
        };
        let Some(run_id) = run_id else {
            return Ok(Some(json!({"data": [], "run_id": null})));
        };
        let rows = self
            .store
            .list_checkpoints(run_id, 1000, Some(strategy.id))?;
        Ok(Some(json!({"data": rows, "run_id": run_id})))
    }
}

/// Start the owned writer; it ends on module shutdown.
pub fn start(module: &Arc<StrategyModule>) {
    let weak = Arc::downgrade(module);
    let token = module.shutdown_token();
    module.spawn(async move {
        let mut tick = tokio::time::interval(CHECKPOINT_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tick.tick().await;
        loop {
            tokio::select! {
                _ = token.cancelled() => break,
                _ = tick.tick() => {
                    let Some(m) = weak.upgrade() else { break };
                    write_once(&m, None);
                }
            }
        }
    });
}
