//! The claim that moves a strategy from stopped to running (SM-01).
//!
//! A start reads the strategy, decides on it (the destination, the Live
//! opt-in) and then awaits its legs, which includes a broker price call. A
//! Live disable, a kill switch or an edit can land in that wait and be
//! confirmed to the trader. The claim is therefore one conditional UPDATE
//! that checks those decisions again against the row as it is now
//! (`Store::claim_strategy_for_run`); this turns its answer into the reason
//! the trader reads. No lock is held across the await.

use super::dispatch::RunMode;
use super::store::{ClaimOutcome, StartClaim, StrategyRow, ALREADY_RUNNING};
use super::StrategyModule;

/// A start whose strategy record could not be read: not "not found".
pub(super) const START_READ_FAILED: &str =
    "The strategy could not be started because its record could not be read. Try again.";
/// A claim whose UPDATE failed: nothing was claimed, so nothing runs.
const CLAIM_WRITE_FAILED: &str =
    "The strategy's record could not be updated, so no run was opened. Try again.";

impl StrategyModule {
    /// Claim `strategy`, as read before the start's awaits, for a start in
    /// `run_mode` from `trigger_source`. `Err` is the trader-facing refusal.
    pub(super) fn claim_start(
        &self,
        strategy: &StrategyRow,
        run_mode: RunMode,
        trigger_source: &str,
    ) -> Result<(), String> {
        let claim = StartClaim {
            revision: strategy.revision,
            live: run_mode == RunMode::Live,
            webhook: trigger_source == "webhook",
        };
        match self.store.claim_strategy_for_run(strategy.id, claim) {
            Ok(ClaimOutcome::Claimed) => Ok(()),
            Ok(refused) => Err(refused.refusal().unwrap_or(ALREADY_RUNNING).to_string()),
            Err(e) => {
                tracing::error!("Could not claim strategy {}: {}", strategy.id, e);
                Err(CLAIM_WRITE_FAILED.into())
            }
        }
    }
}
