//! The API plan the trader chose per broker (`Broker::rate_plans`), so the
//! broker's requests are paced for it across restarts. Desktop only: the
//! web keeps one limiter setting per process.

use crate::error::Result;
use rusqlite::{params, Connection, OptionalExtension};

/// Migration 081: one row per broker with a chosen plan.
pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS broker_rate_plans (
            broker_id TEXT PRIMARY KEY,
            plan TEXT NOT NULL,
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )?;
    Ok(())
}

/// The plan saved for `broker`.
pub fn get(conn: &Connection, broker: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT plan FROM broker_rate_plans WHERE broker_id = ?1",
            params![broker],
            |r| r.get(0),
        )
        .optional()?)
}

/// Save `plan` for `broker`.
pub fn set(conn: &Connection, broker: &str, plan: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO broker_rate_plans (broker_id, plan, updated_at)
         VALUES (?1, ?2, datetime('now'))
         ON CONFLICT(broker_id) DO UPDATE SET plan = excluded.plan, updated_at = excluded.updated_at",
        params![broker, plan],
    )?;
    Ok(())
}

/// Every saved `(broker, plan)`.
pub fn all(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare("SELECT broker_id, plan FROM broker_rate_plans")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_and_replaces_a_plan_per_broker() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        assert_eq!(get(&conn, "fyers").unwrap(), None);
        set(&conn, "fyers", "prime").unwrap();
        set(&conn, "fyers", "standard").unwrap();
        assert_eq!(get(&conn, "fyers").unwrap().as_deref(), Some("standard"));
        assert_eq!(
            all(&conn).unwrap(),
            vec![("fyers".into(), "standard".into())]
        );
    }
}
