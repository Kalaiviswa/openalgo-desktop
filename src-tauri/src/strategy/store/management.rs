//! Management writes on a strategy: edit, delete, the start claim and its
//! release, the webhook token, the Live opt-in and the webhook lock. The
//! reads and the run, order and event bookkeeping stay in `store.rs`.

use super::{
    generate_webhook_token, hash_webhook_token, json_col, num_param, time_col, Store, StrategyRow,
    UPDATABLE_FIELDS,
};
use crate::error::{AppError, Result};
use rusqlite::{params, TransactionBehavior};
use serde_json::{Map, Value};

impl Store {
    /// Update a stopped strategy with already-validated `changes`.
    pub fn update_strategy(
        &self,
        strategy_id: i64,
        user_id: &str,
        changes: &Map<String, Value>,
    ) -> Result<StrategyRow> {
        let Some(row) = self.get_strategy(strategy_id, user_id)? else {
            return Err(AppError::NotFound("Strategy not found".into()));
        };
        if row.status == "running" {
            return Err(AppError::Validation(
                "Stop the strategy before editing it".into(),
            ));
        }
        if let Some(kind) = changes.get("strategy_kind").and_then(Value::as_str) {
            if kind != row.strategy_kind {
                return Err(AppError::Validation(
                    "A strategy cannot change between batch and signal. The two kinds do not \
                     share a leg shape, so every leg would describe the wrong kind of contract. \
                     Create a new strategy instead."
                        .into(),
                ));
            }
        }
        let conn = self.conn()?;
        for (field, value) in changes {
            if !UPDATABLE_FIELDS.contains(&field.as_str()) {
                continue;
            }
            let sql = format!("UPDATE sm_strategy SET {} = ?1 WHERE id = ?2", field);
            let param: rusqlite::types::Value = match field.as_str() {
                "entry_time" | "exit_time" => time_col(value).into(),
                "legs" | "lock_profit" | "scheduler" | "webhook_ip_allowlist" => {
                    json_col(value).into()
                }
                "overall_sl_mtm" | "overall_target_mtm" | "daily_loss_limit_inr" => {
                    num_param(value).into()
                }
                "trail_sl_to_entry" => i64::from(value.as_bool().unwrap_or(false)).into(),
                _ => value.as_str().map(str::to_string).into(),
            };
            conn.execute(&sql, params![param, strategy_id])?;
        }
        conn.execute(
            "UPDATE sm_strategy SET updated_at = ?1 WHERE id = ?2",
            params![self.utcnow(), strategy_id],
        )?;
        drop(conn);
        self.get_strategy_unscoped(strategy_id)?
            .ok_or_else(|| AppError::NotFound("Strategy not found".into()))
    }

    /// Delete a stopped strategy and every row that belongs to it.
    pub fn delete_strategy(&self, strategy_id: i64, user_id: &str) -> Result<()> {
        let Some(row) = self.get_strategy(strategy_id, user_id)? else {
            return Err(AppError::NotFound("Strategy not found".into()));
        };
        if row.status == "running" {
            return Err(AppError::Validation(
                "Stop the strategy before deleting it".into(),
            ));
        }
        let mut conn = self.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM sm_strategy_checkpoint WHERE run_id IN \
             (SELECT id FROM sm_strategy_run WHERE strategy_id = ?1)",
            [strategy_id],
        )?;
        tx.execute(
            "DELETE FROM sm_strategy_order WHERE run_id IN \
             (SELECT id FROM sm_strategy_run WHERE strategy_id = ?1)",
            [strategy_id],
        )?;
        tx.execute(
            "DELETE FROM sm_strategy_event WHERE strategy_id = ?1",
            [strategy_id],
        )?;
        tx.execute(
            "DELETE FROM sm_webhook_event WHERE strategy_id = ?1",
            [strategy_id],
        )?;
        tx.execute(
            "DELETE FROM sm_strategy_run WHERE strategy_id = ?1",
            [strategy_id],
        )?;
        tx.execute("DELETE FROM sm_strategy WHERE id = ?1", [strategy_id])?;
        tx.commit()?;
        Ok(())
    }

    /// Stopped to running, in one conditional UPDATE. True only for the caller
    /// that made the move.
    pub fn claim_strategy_for_run(&self, strategy_id: i64) -> Result<bool> {
        let conn = self.conn()?;
        Ok(conn.execute(
            "UPDATE sm_strategy SET status = 'running' WHERE id = ?1 AND status = 'stopped'",
            [strategy_id],
        )? == 1)
    }

    pub fn release_strategy(&self, strategy_id: i64) -> Result<bool> {
        let conn = self.conn()?;
        Ok(conn.execute(
            "UPDATE sm_strategy SET status = 'stopped', current_run_id = NULL WHERE id = ?1",
            [strategy_id],
        )? == 1)
    }

    pub fn rotate_webhook_token(&self, strategy_id: i64, user_id: &str) -> Result<String> {
        if self.get_strategy(strategy_id, user_id)?.is_none() {
            return Err(AppError::NotFound("Strategy not found".into()));
        }
        let token = generate_webhook_token();
        let conn = self.conn()?;
        conn.execute(
            "UPDATE sm_strategy SET webhook_token_hash = ?1, updated_at = ?2 WHERE id = ?3",
            params![hash_webhook_token(&token), self.utcnow(), strategy_id],
        )?;
        Ok(token)
    }

    pub fn set_live_enabled(&self, strategy_id: i64, user_id: &str, enabled: bool) -> Result<()> {
        let Some(row) = self.get_strategy(strategy_id, user_id)? else {
            return Err(AppError::NotFound("Strategy not found".into()));
        };
        if row.status == "running" {
            return Err(AppError::Validation(
                "Stop the strategy before changing its mode".into(),
            ));
        }
        let conn = self.conn()?;
        conn.execute(
            "UPDATE sm_strategy SET live_enabled = ?1 WHERE id = ?2",
            params![enabled, strategy_id],
        )?;
        Ok(())
    }

    pub fn set_webhook_locked(&self, strategy_id: i64, user_id: &str, locked: bool) -> Result<()> {
        if self.get_strategy(strategy_id, user_id)?.is_none() {
            return Err(AppError::NotFound("Strategy not found".into()));
        }
        let conn = self.conn()?;
        conn.execute(
            "UPDATE sm_strategy SET webhook_locked = ?1 WHERE id = ?2",
            params![locked, strategy_id],
        )?;
        Ok(())
    }
}
