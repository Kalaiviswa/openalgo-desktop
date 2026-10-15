//! The webhook audit (`sm_webhook_event`).

use super::{iso, json_col, read_json, Store, MAX_UNATTRIBUTED_WEBHOOK_EVENTS};
use crate::error::Result;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

impl Store {
    /// Audit one inbound webhook, whatever the outcome. Ownerless rows (an
    /// unknown token) are capped at the newest `MAX_UNATTRIBUTED_WEBHOOK_EVENTS`.
    #[allow(clippy::too_many_arguments)]
    pub fn record_webhook_event(
        &self,
        result: &str,
        strategy_id: Option<i64>,
        action: Option<&str>,
        mode: Option<&str>,
        payload: Option<&Value>,
        ip: Option<&str>,
        user_agent: Option<&str>,
        error: Option<&str>,
    ) -> Result<i64> {
        let conn = self.conn()?;
        let ua = user_agent
            .map(|u| u.chars().take(255).collect::<String>())
            .filter(|u| !u.is_empty());
        conn.execute(
            "INSERT INTO sm_webhook_event (strategy_id, action, mode, payload, ip, user_agent, \
             received_at, result, error) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                strategy_id,
                action.map(|a| a.chars().take(20).collect::<String>()),
                mode.map(|m| m.chars().take(10).collect::<String>()),
                payload.and_then(json_col),
                ip.map(|i| i.chars().take(45).collect::<String>()),
                ua,
                self.utcnow(),
                result,
                error
            ],
        )?;
        let id = conn.last_insert_rowid();
        if strategy_id.is_none() {
            conn.execute(
                "DELETE FROM sm_webhook_event WHERE strategy_id IS NULL AND id NOT IN \
                 (SELECT id FROM sm_webhook_event WHERE strategy_id IS NULL ORDER BY id DESC LIMIT ?1)",
                [MAX_UNATTRIBUTED_WEBHOOK_EVENTS],
            )?;
        }
        Ok(id)
    }

    pub fn list_webhook_events(&self, strategy_id: i64, limit: i64) -> Result<Vec<Value>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, strategy_id, action, mode, payload, ip, user_agent, received_at, result, \
             error FROM sm_webhook_event WHERE strategy_id = ?1 \
             ORDER BY received_at DESC, id DESC LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![strategy_id, limit.max(1)], |r| {
                let received: String = r.get(7)?;
                Ok(json!({
                    "id": (r.get::<_, i64>(0)?),
                    "strategy_id": (r.get::<_, Option<i64>>(1)?),
                    "action": (r.get::<_, Option<String>>(2)?),
                    "mode": (r.get::<_, Option<String>>(3)?),
                    "payload": read_json(r.get(4)?),
                    "ip": (r.get::<_, Option<String>>(5)?),
                    "user_agent": (r.get::<_, Option<String>>(6)?),
                    "received_at": iso(Some(&received)),
                    "result": (r.get::<_, String>(8)?),
                    "error": (r.get::<_, Option<String>>(9)?),
                }))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn count_unattributed_webhook_events(&self) -> Result<i64> {
        let conn = self.conn()?;
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM sm_webhook_event WHERE strategy_id IS NULL",
            [],
            |r| r.get(0),
        )?)
    }

    /// The username that owns the API key (single-user: one row).
    pub fn api_key_owner(&self) -> Result<Option<String>> {
        let conn = self.conn()?;
        Ok(conn
            .query_row("SELECT name FROM api_keys ORDER BY id LIMIT 1", [], |r| {
                r.get(0)
            })
            .optional()?)
    }

    /// Raw SQL for tests and recovery tooling that need to place the store in
    /// a state no product path produces.
    pub fn execute_raw(&self, sql: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute_batch(sql)?;
        Ok(())
    }
}
