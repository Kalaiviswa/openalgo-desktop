//! Runtime checkpoints (`sm_strategy_checkpoint`): the run's live state
//! snapshot, written and restored.

use super::{f64_json, fnum, iso, read_json, round_to, Store};
use crate::error::Result;
use rusqlite::{params, OptionalExtension, Row};
use serde_json::{json, Value};

impl Store {
    pub fn write_checkpoint(&self, run_id: i64, snapshot: &Value) -> Result<()> {
        let f = |k: &str| round_to(snapshot[k].as_f64().unwrap_or(0.0), 2);
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO sm_strategy_checkpoint (run_id, ts, pnl_realized, pnl_unrealized, \
             pnl_total, pnl_peak, pnl_trough, lock_floor, trail_to_entry_active, leg_state) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                run_id,
                self.utcnow(),
                f("pnl_realized"),
                f("pnl_unrealized"),
                f("pnl_total"),
                f("pnl_peak"),
                f("pnl_trough"),
                snapshot["lock_floor"].as_f64().map(|v| round_to(v, 2)),
                snapshot["trail_to_entry_active"].as_bool().unwrap_or(false),
                snapshot
                    .get("leg_state")
                    .cloned()
                    .unwrap_or(json!({}))
                    .to_string(),
            ],
        )?;
        Ok(())
    }

    fn checkpoint_dict(r: &Row<'_>) -> rusqlite::Result<Value> {
        let lock_floor: Option<f64> = r.get(8)?;
        let ts: String = r.get(2)?;
        let id: i64 = r.get(0)?;
        let run_id: i64 = r.get(1)?;
        let money = |i: usize| -> rusqlite::Result<Value> {
            Ok(fnum(r.get::<_, Option<f64>>(i)?.unwrap_or(0.0)))
        };
        let (realized, unrealized, total, peak, trough) =
            (money(3)?, money(4)?, money(5)?, money(6)?, money(7)?);
        let trail: bool = r.get(9)?;
        let mut legs = read_json(r.get(10)?);
        if legs.is_null() {
            legs = json!({});
        }
        Ok(json!({
            "id": id,
            "run_id": run_id,
            "ts": iso(Some(&ts)),
            "pnl_realized": realized,
            "pnl_unrealized": unrealized,
            "pnl_total": total,
            "pnl_peak": peak,
            "pnl_trough": trough,
            "lock_floor": f64_json(lock_floor),
            "trail_to_entry_active": trail,
            "leg_state": legs,
        }))
    }

    const CHECKPOINT_COLS: &'static str =
        "c.id, c.run_id, c.ts, c.pnl_realized, c.pnl_unrealized, \
        c.pnl_total, c.pnl_peak, c.pnl_trough, c.lock_floor, c.trail_to_entry_active, c.leg_state";

    pub fn latest_checkpoint(&self, run_id: i64) -> Result<Option<Value>> {
        let conn = self.conn()?;
        Ok(conn
            .query_row(
                &format!(
                    "SELECT {} FROM sm_strategy_checkpoint c WHERE c.run_id = ?1 \
                     ORDER BY c.ts DESC, c.id DESC LIMIT 1",
                    Self::CHECKPOINT_COLS
                ),
                [run_id],
                Self::checkpoint_dict,
            )
            .optional()?)
    }

    /// A run's checkpoints, oldest first, narrowed to a run of `strategy_id`.
    pub fn list_checkpoints(
        &self,
        run_id: i64,
        limit: i64,
        strategy_id: Option<i64>,
    ) -> Result<Vec<Value>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM sm_strategy_checkpoint c JOIN sm_strategy_run r ON c.run_id = r.id \
             WHERE c.run_id = ?1 AND (?2 IS NULL OR r.strategy_id = ?2) \
             ORDER BY c.ts ASC, c.id ASC LIMIT ?3",
            Self::CHECKPOINT_COLS
        ))?;
        let rows = stmt
            .query_map(
                params![run_id, strategy_id, limit.max(1)],
                Self::checkpoint_dict,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Keep only the newest `keep` checkpoints for a run.
    pub fn prune_checkpoints(&self, run_id: i64, keep: i64) -> Result<usize> {
        let conn = self.conn()?;
        Ok(conn.execute(
            "DELETE FROM sm_strategy_checkpoint WHERE run_id = ?1 AND id NOT IN \
             (SELECT id FROM sm_strategy_checkpoint WHERE run_id = ?1 ORDER BY ts DESC, id DESC LIMIT ?2)",
            params![run_id, keep.max(1)],
        )?)
    }

    pub fn count_checkpoints(&self, run_id: i64) -> Result<i64> {
        let conn = self.conn()?;
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM sm_strategy_checkpoint WHERE run_id = ?1",
            [run_id],
            |r| r.get(0),
        )?)
    }
}
