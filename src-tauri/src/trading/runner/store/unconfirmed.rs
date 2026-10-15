//! The runner's orders that had no definite answer (LOG-08): a placement the
//! destination neither accepted nor refused may be at the broker, so its row
//! is `unconfirmed` until the order book settles it (`runner::reconcile`).
//! Migration `084_openscript_unconfirmed` adds what the book is searched
//! with.

use super::{order_from_row, OrderRow, ORDER_COLUMNS};
use crate::error::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};

/// A placement the destination neither accepted nor refused (LOG-08): it may
/// be at the broker. Never placed again, never counted as flat, until the
/// order book settles it.
pub const UNCONFIRMED: &str = "unconfirmed";

/// Migration `084_openscript_unconfirmed` (LOG-08): what the order book is
/// searched with for a placement that had no definite answer (the client
/// tag it was sent with, its limit and trigger prices), and an index for
/// the reconciler's look for unconfirmed rows. Checks before altering.
pub fn migrate_unconfirmed(conn: &Connection) -> Result<()> {
    for (column, kind) in [
        ("client_tag", "TEXT"),
        ("price", "REAL"),
        ("trigger_price", "REAL"),
    ] {
        let has = conn
            .prepare("SELECT 1 FROM pragma_table_info('openscript_orders') WHERE name = ?1")?
            .exists([column])?;
        if !has {
            conn.execute_batch(&format!(
                "ALTER TABLE openscript_orders ADD COLUMN {} {};",
                column, kind
            ))?;
        }
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS ix_openscript_orders_status ON openscript_orders(status);",
    )?;
    Ok(())
}

/// Record an uncertain answer to a placement: the row is unconfirmed and
/// keeps the tag the order was sent with.
pub fn unconfirmed(
    conn: &Connection,
    id: i64,
    client_tag: Option<&str>,
    message: Option<&str>,
    now: DateTime<Utc>,
) -> Result<()> {
    conn.execute(
        "UPDATE openscript_orders SET status = ?2, client_tag = ?3, message = ?4, updated_at = ?5
         WHERE id = ?1",
        params![id, UNCONFIRMED, client_tag, message, now.to_rfc3339()],
    )?;
    Ok(())
}

/// Bind an unconfirmed row to the broker order the book found for it.
pub fn bind(conn: &Connection, id: i64, orderid: &str, now: DateTime<Utc>) -> Result<bool> {
    Ok(conn.execute(
        "UPDATE openscript_orders SET orderid = ?2, status = 'working', message = NULL,
            updated_at = ?3 WHERE id = ?1 AND status = ?4 AND orderid IS NULL",
        params![id, orderid, now.to_rfc3339(), UNCONFIRMED],
    )? > 0)
}

/// Settle an unconfirmed row as never placed (the sandbox's book confirmed
/// its absence, or the trader checked the broker's books). False when the
/// row is no longer unconfirmed: a pass found it, or it was settled already.
pub fn release_unconfirmed(
    conn: &Connection,
    id: i64,
    message: &str,
    now: DateTime<Utc>,
) -> Result<bool> {
    Ok(conn.execute(
        "UPDATE openscript_orders SET status = 'rejected', message = ?2, updated_at = ?3
         WHERE id = ?1 AND status = ?4 AND orderid IS NULL",
        params![id, message, now.to_rfc3339(), UNCONFIRMED],
    )? > 0)
}

/// One order row by its id.
pub fn order_by_id(conn: &Connection, id: i64) -> Result<Option<OrderRow>> {
    let mut st = conn.prepare_cached(&format!(
        "SELECT {} FROM openscript_orders WHERE id = ?1",
        ORDER_COLUMNS
    ))?;
    Ok(st.query_row(params![id], order_from_row).optional()?)
}

/// Rows a previous run of the app left between sending and recording the
/// answer: the order may be at the destination, so they are unconfirmed,
/// for the order book to settle. Run at start-up, when nothing is sending.
pub fn unconfirm_leftovers(conn: &Connection, now: DateTime<Utc>) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE openscript_orders SET status = ?1, updated_at = ?2,
            message = 'The app stopped before the answer to this order was recorded'
         WHERE status = 'placed' AND orderid IS NULL",
        params![UNCONFIRMED, now.to_rfc3339()],
    )?)
}

/// Every unconfirmed row on one side, oldest first.
pub fn unconfirmed_rows(conn: &Connection, mode: &str) -> Result<Vec<OrderRow>> {
    let mut st = conn.prepare_cached(&format!(
        "SELECT {} FROM openscript_orders WHERE status = ?1 AND mode = ?2 ORDER BY id",
        ORDER_COLUMNS
    ))?;
    let rows = st
        .query_map(params![UNCONFIRMED, mode], order_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Whether one side has an unconfirmed row (the reconciler reads a book
/// only while someone does).
pub fn has_unconfirmed(conn: &Connection, mode: &str) -> Result<bool> {
    Ok(conn
        .prepare_cached("SELECT 1 FROM openscript_orders WHERE status = ?1 AND mode = ?2 LIMIT 1")?
        .exists(params![UNCONFIRMED, mode])?)
}

/// Whether a destination order id is already one of the runner's rows.
pub fn owns_orderid(conn: &Connection, orderid: &str) -> Result<bool> {
    Ok(conn
        .prepare_cached("SELECT 1 FROM openscript_orders WHERE orderid = ?1 LIMIT 1")?
        .exists(params![orderid])?)
}

#[cfg(test)]
mod tests {
    use super::super::{insert_order, orders_of, placed, NewOrder};
    use super::*;
    use chrono::TimeZone;

    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        super::super::migrate(&c).unwrap();
        migrate_unconfirmed(&c).unwrap();
        c
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 5, 4, 0, 0).unwrap()
    }

    /// LOG-08: a row the app stopped on between sending and recording the
    /// answer may be at the destination. At start it becomes unconfirmed,
    /// for the order book to settle; rows with an answer are untouched. An
    /// unconfirmed row is settled once: bound to one order, or released.
    #[test]
    fn rows_left_without_an_answer_are_unconfirmed_at_start() {
        let c = db();
        let order = || NewOrder {
            deployment: "d".into(),
            mode: "live".into(),
            symbol: "SBIN".into(),
            exchange: "NSE".into(),
            action: "BUY".into(),
            quantity: 5,
            pricetype: "LIMIT".into(),
            price: Some(101.5),
            product: "MIS".into(),
            ..Default::default()
        };
        let sent = insert_order(&c, &order(), now()).unwrap();
        let answered = insert_order(&c, &order(), now()).unwrap();
        placed(&c, answered, Some("OID9"), "working", None, now()).unwrap();
        assert_eq!(unconfirm_leftovers(&c, now()).unwrap(), 1);
        let rows = unconfirmed_rows(&c, "live").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].id, rows[0].price), (sent, Some(101.5)));
        assert!(has_unconfirmed(&c, "live").unwrap());
        assert!(!has_unconfirmed(&c, "sandbox").unwrap());
        assert!(bind(&c, sent, "OID10", now()).unwrap());
        assert!(!bind(&c, sent, "OID11", now()).unwrap(), "bound once");
        assert!(!release_unconfirmed(&c, sent, "not placed", now()).unwrap());
        assert!(owns_orderid(&c, "OID10").unwrap());
        assert_eq!(order_by_id(&c, sent).unwrap().unwrap().status, "working");
    }

    /// Migration 084 on a table that already holds rows (as 074 left it):
    /// the rows stay, the new columns are empty, and a second run changes
    /// nothing.
    #[test]
    fn the_unconfirmed_migration_keeps_existing_rows() {
        let c = Connection::open_in_memory().unwrap();
        super::super::migrate(&c).unwrap();
        c.execute(
            "INSERT INTO openscript_orders (deployment, mode, tag, symbol, exchange, action,
                quantity, pricetype, product, orderid, status, filled_quantity, placed_at,
                updated_at)
             VALUES ('d', 'live', '', 'SBIN', 'NSE', 'BUY', 5, 'MARKET', 'MIS', 'OID1',
                'filled', 5, '2026-10-01T04:00:00+00:00', '2026-10-01T04:00:00+00:00')",
            [],
        )
        .unwrap();
        migrate_unconfirmed(&c).unwrap();
        migrate_unconfirmed(&c).unwrap();
        let rows = orders_of(&c, "d", "live").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].orderid.as_deref(), Some("OID1"));
        assert_eq!(rows[0].filled_quantity, 5);
        assert_eq!(
            (
                rows[0].client_tag.clone(),
                rows[0].price,
                rows[0].trigger_price
            ),
            (None, None, None)
        );
    }
}
