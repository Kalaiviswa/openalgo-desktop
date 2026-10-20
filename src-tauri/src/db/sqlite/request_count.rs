//! Requests counted against a broker's daily caps today (Fyers), one row
//! per broker and request category, so a restart in the middle of the day
//! keeps the day's count. A row carries its day (IST); a row of an earlier
//! day counts nothing and is replaced by the next save. Desktop only: the
//! web counts no daily requests.

use crate::brokers::common::ratelimit::DailyCount;
use crate::error::Result;
use chrono::NaiveDate;
use rusqlite::{params, Connection};

/// Migration 093: at most one row per broker and category.
pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS broker_daily_requests (
            broker_id TEXT NOT NULL,
            category TEXT NOT NULL,
            day TEXT NOT NULL,
            sent INTEGER NOT NULL,
            PRIMARY KEY (broker_id, category)
        );",
    )?;
    Ok(())
}

/// Save `counts` for `broker`, replacing what was saved for each category,
/// in one writing transaction (`BEGIN IMMEDIATE`).
pub fn save(conn: &Connection, broker: &str, counts: &[DailyCount]) -> Result<()> {
    let tx = super::write_transaction(conn)?;
    for c in counts {
        tx.execute(
            "INSERT INTO broker_daily_requests (broker_id, category, day, sent)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(broker_id, category) DO UPDATE SET day = excluded.day, sent = excluded.sent",
            params![broker, c.category, c.day.to_string(), c.sent],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// What was saved for `broker` on `day`.
pub fn on_day(conn: &Connection, broker: &str, day: NaiveDate) -> Result<Vec<DailyCount>> {
    let mut stmt = conn.prepare(
        "SELECT category, sent FROM broker_daily_requests
         WHERE broker_id = ?1 AND day = ?2 ORDER BY category",
    )?;
    let rows = stmt.query_map(params![broker, day.to_string()], |r| {
        Ok(DailyCount {
            category: r.get(0)?,
            day,
            sent: r.get(1)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// Drop the rows of days before `today`: those days are over.
pub fn clear_before(conn: &Connection, today: NaiveDate) -> Result<usize> {
    Ok(conn.execute(
        "DELETE FROM broker_daily_requests WHERE day < ?1",
        params![today.to_string()],
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(category: &str, day: NaiveDate, sent: u32) -> DailyCount {
        DailyCount {
            category: category.into(),
            day,
            sent,
        }
    }

    #[test]
    fn one_row_per_category_cleared_after_its_day() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        let mon = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        let tue = NaiveDate::from_ymd_opt(2026, 10, 20).unwrap();
        save(
            &conn,
            "fyers",
            &[count("data", mon, 40), count("transaction", mon, 3)],
        )
        .unwrap();
        save(&conn, "fyers", &[count("data", mon, 45)]).unwrap();
        assert_eq!(
            on_day(&conn, "fyers", mon).unwrap(),
            vec![count("data", mon, 45), count("transaction", mon, 3)]
        );
        // The next day replaces the row; Monday's is gone.
        save(&conn, "fyers", &[count("data", tue, 2)]).unwrap();
        assert_eq!(
            on_day(&conn, "fyers", tue).unwrap(),
            vec![count("data", tue, 2)]
        );
        assert_eq!(clear_before(&conn, tue).unwrap(), 1);
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM broker_daily_requests", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(rows, 1);
        assert!(on_day(&conn, "upstox", tue).unwrap().is_empty());
    }
}
