//! The risk-event audit trail (`sm_strategy_event`).

use super::{event_from, json_col, EventFields, EventRow, Store, EVENT_COLS};
use crate::error::Result;
use rusqlite::params;
use serde_json::Value;

impl Store {
    pub fn record_event(
        &self,
        strategy_id: i64,
        user_id: &str,
        kind: &str,
        message: &str,
        f: &EventFields,
    ) -> Result<EventRow> {
        let conn = self.conn()?;
        let now = self.utcnow();
        conn.execute(
            "INSERT INTO sm_strategy_event (run_id, strategy_id, user_id, ts, kind, severity, \
             leg_id, message, payload) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                f.run_id,
                strategy_id,
                user_id,
                now,
                kind,
                f.severity.unwrap_or("info"),
                f.leg_id,
                message,
                f.payload.as_ref().and_then(json_col),
            ],
        )?;
        let id = conn.last_insert_rowid();
        Ok(EventRow {
            id,
            run_id: f.run_id,
            strategy_id,
            user_id: user_id.to_string(),
            ts: now,
            kind: kind.to_string(),
            severity: f.severity.unwrap_or("info").to_string(),
            leg_id: f.leg_id,
            message: message.to_string(),
            payload: f.payload.clone().unwrap_or(Value::Null),
        })
    }

    pub fn list_events(
        &self,
        strategy_id: i64,
        run_id: Option<i64>,
        kind: Option<&str>,
        severity: Option<&str>,
        limit: i64,
    ) -> Result<Vec<Value>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM sm_strategy_event WHERE strategy_id = ?1 \
             AND (?2 IS NULL OR run_id = ?2) AND (?3 IS NULL OR kind = ?3) \
             AND (?4 IS NULL OR severity = ?4) ORDER BY ts DESC, id DESC LIMIT ?5",
            EVENT_COLS
        ))?;
        let rows = stmt
            .query_map(
                params![strategy_id, run_id, kind, severity, limit.max(1)],
                event_from,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows.iter().map(EventRow::to_dict).collect())
    }

    /// The append-only lost-acknowledgement witnesses of one run, oldest first.
    pub fn list_order_ack_events(&self, run_id: i64) -> Result<Vec<EventRow>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM sm_strategy_event WHERE run_id = ?1 AND kind = 'order_ack_unrecorded' \
             ORDER BY id ASC",
            EVENT_COLS
        ))?;
        let rows = stmt
            .query_map([run_id], event_from)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}
