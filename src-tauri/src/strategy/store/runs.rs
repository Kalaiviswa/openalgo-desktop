//! Run rows: create, the terminal run CAS, the live run reads and the run
//! P&L (web `strategy_module_db.py` run functions).

use super::{json_col, round_to, run_from, ts, RunRow, Store, RUN_COLS};
use crate::error::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde_json::Value;

impl Store {
    pub fn create_run(
        &self,
        strategy_id: i64,
        mode: &str,
        broker: &str,
        trigger_source: &str,
        webhook_event_id: Option<i64>,
        resolved_expiries: Option<&Value>,
    ) -> Result<i64> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO sm_strategy_run (strategy_id, mode, broker, started_at, trigger_source, \
             webhook_event_id, resolved_expiries) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                strategy_id,
                mode,
                broker,
                self.utcnow(),
                trigger_source,
                webhook_event_id,
                resolved_expiries.and_then(json_col),
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn get_run(&self, run_id: i64) -> Result<Option<RunRow>> {
        let conn = self.conn()?;
        Ok(conn
            .query_row(
                &format!("SELECT {} FROM sm_strategy_run WHERE id = ?1", RUN_COLS),
                [run_id],
                run_from,
            )
            .optional()?)
    }

    pub fn list_runs(&self, strategy_id: i64, limit: i64) -> Result<Vec<Value>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM sm_strategy_run WHERE strategy_id = ?1 \
             ORDER BY started_at DESC, id DESC LIMIT ?2",
            RUN_COLS
        ))?;
        let rows = stmt
            .query_map(params![strategy_id, limit.max(1)], run_from)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows.iter().map(RunRow::to_dict).collect())
    }

    pub fn list_open_runs(&self) -> Result<Vec<RunRow>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM sm_strategy_run WHERE stopped_at IS NULL ORDER BY id",
            RUN_COLS
        ))?;
        let rows = stmt
            .query_map([], run_from)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn finish_fields(&self) -> String {
        self.utcnow()
    }

    /// Finish one active run and release only its current strategy, in one
    /// transaction. False when the run was already finished or the strategy
    /// now points at another run: neither row changes then.
    pub fn finish_run_and_release_strategy(
        &self,
        run_id: i64,
        strategy_id: i64,
        stop_reason: &str,
        pnl_realized: f64,
        pnl_peak: f64,
        pnl_trough: f64,
    ) -> Result<bool> {
        let now = self.finish_fields();
        let mut conn = self.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let finished = tx.execute(
            "UPDATE sm_strategy_run SET stopped_at = ?1, stop_reason = ?2, stop_requested_at = NULL, \
             stop_requested_reason = NULL, pnl_realized = ?3, pnl_peak = ?4, pnl_trough = ?5 \
             WHERE id = ?6 AND strategy_id = ?7 AND stopped_at IS NULL",
            params![
                now,
                stop_reason,
                round_to(pnl_realized, 2),
                round_to(pnl_peak, 2),
                round_to(pnl_trough, 2),
                run_id,
                strategy_id
            ],
        )?;
        if finished != 1 {
            tx.rollback()?;
            return Ok(false);
        }
        let released = tx.execute(
            "UPDATE sm_strategy SET status = 'stopped', current_run_id = NULL \
             WHERE id = ?1 AND current_run_id = ?2",
            params![strategy_id, run_id],
        )?;
        if released != 1 {
            tx.rollback()?;
            return Ok(false);
        }
        tx.commit()?;
        Ok(true)
    }

    /// Finish a residual run that is not the strategy's current run.
    pub fn finish_detached_run(
        &self,
        run_id: i64,
        strategy_id: i64,
        stop_reason: &str,
        pnl_realized: f64,
        pnl_peak: f64,
        pnl_trough: f64,
    ) -> Result<bool> {
        let conn = self.conn()?;
        Ok(conn.execute(
            "UPDATE sm_strategy_run SET stopped_at = ?1, stop_reason = ?2, stop_requested_at = NULL, \
             stop_requested_reason = NULL, pnl_realized = ?3, pnl_peak = ?4, pnl_trough = ?5 \
             WHERE id = ?6 AND strategy_id = ?7 AND stopped_at IS NULL AND NOT EXISTS \
             (SELECT 1 FROM sm_strategy WHERE id = ?7 AND current_run_id = ?6)",
            params![
                self.utcnow(),
                stop_reason,
                round_to(pnl_realized, 2),
                round_to(pnl_peak, 2),
                round_to(pnl_trough, 2),
                run_id,
                strategy_id
            ],
        )? == 1)
    }

    /// Close a never-linked run, then release only its empty strategy claim.
    pub fn finish_unlinked_run_and_release_claim(
        &self,
        run_id: i64,
        strategy_id: i64,
        stop_reason: &str,
    ) -> Result<bool> {
        if !self.finish_detached_run(run_id, strategy_id, stop_reason, 0.0, 0.0, 0.0)? {
            return Ok(false);
        }
        let conn = self.conn()?;
        Ok(conn.execute(
            "UPDATE sm_strategy SET status = 'stopped', current_run_id = NULL \
             WHERE id = ?1 AND status = 'running' AND current_run_id IS NULL",
            [strategy_id],
        )? == 1)
    }

    /// Persist a stop request while leaving the run active until it is flat.
    /// True when a request is (now or already) recorded on an open run.
    pub fn request_run_stop(&self, run_id: i64, reason: &str) -> Result<bool> {
        let conn = self.conn()?;
        let updated = conn.execute(
            "UPDATE sm_strategy_run SET stop_requested_at = ?1, stop_requested_reason = ?2 \
             WHERE id = ?3 AND stopped_at IS NULL AND stop_requested_at IS NULL \
             AND stop_requested_reason IS NULL",
            params![self.utcnow(), reason, run_id],
        )?;
        if updated == 1 {
            return Ok(true);
        }
        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM sm_strategy_run WHERE id = ?1 AND stopped_at IS NULL \
                 AND stop_requested_at IS NOT NULL AND stop_requested_reason IS NOT NULL",
                [run_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(existing.is_some())
    }

    /// What earlier runs banked since `since` (a daily loss limit's session).
    pub fn realized_pnl_since(
        &self,
        strategy_id: i64,
        since: DateTime<Utc>,
        exclude_run_id: Option<i64>,
    ) -> Result<f64> {
        let conn = self.conn()?;
        let total: Option<f64> = conn.query_row(
            "SELECT SUM(pnl_realized) FROM sm_strategy_run WHERE strategy_id = ?1 \
             AND started_at >= ?2 AND id != ?3",
            params![strategy_id, ts(since), exclude_run_id.unwrap_or(-1)],
            |r| r.get(0),
        )?;
        Ok(total.unwrap_or(0.0))
    }
}
