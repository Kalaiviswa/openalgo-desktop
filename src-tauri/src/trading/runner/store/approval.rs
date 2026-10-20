//! A run's orders that Semi-Auto order mode held for the trader's approval
//! in the Action Center (LOG-04). The row keeps the pending order's id, so
//! the run follows what its approval became (`runner::approval`): bound to
//! the broker order it placed, rejected, or unconfirmed (LOG-08).
//! Migration `088_openscript_pending_link` adds the link.

use super::UNCONFIRMED;
use crate::error::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

/// What a held row reads until the trader decides.
const AWAITING_APPROVAL: &str = "Waiting for approval in the Action Center";

/// Migration `088_openscript_pending_link` (LOG-04): the Action Center
/// order a row was held as. Checks before altering.
pub fn migrate_pending_link(conn: &Connection) -> Result<()> {
    if !crate::db::sqlite::migrations::column_exists(conn, "openscript_orders", "pending_order_id")?
    {
        conn.execute_batch("ALTER TABLE openscript_orders ADD COLUMN pending_order_id INTEGER;")?;
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS ix_openscript_orders_pending
            ON openscript_orders(pending_order_id);",
    )?;
    Ok(())
}

/// Record a placement Semi-Auto held for approval: open, with no broker
/// order yet, linked to the Action Center order.
pub fn held_for_approval(
    conn: &Connection,
    id: i64,
    pending_order_id: i64,
    now: DateTime<Utc>,
) -> Result<()> {
    conn.execute(
        "UPDATE openscript_orders SET status = 'working', pending_order_id = ?2, message = ?3,
            updated_at = ?4 WHERE id = ?1",
        params![id, pending_order_id, AWAITING_APPROVAL, now.to_rfc3339()],
    )?;
    Ok(())
}

/// Bind a held row to the broker order its approval placed. False when it
/// was bound already (the order book found it first) or is no longer open.
pub fn bind_approved(
    conn: &Connection,
    id: i64,
    orderid: &str,
    now: DateTime<Utc>,
) -> Result<bool> {
    Ok(conn.execute(
        "UPDATE openscript_orders SET orderid = ?2, status = 'working', message = NULL,
            updated_at = ?3
         WHERE id = ?1 AND orderid IS NULL AND pending_order_id IS NOT NULL
           AND status IN ('working', ?4)",
        params![id, orderid, now.to_rfc3339(), UNCONFIRMED],
    )? > 0)
}

/// A held row whose order was never placed: rejected by the trader, or
/// refused by the broker on approval. False when it is no longer open.
pub fn refuse_held(conn: &Connection, id: i64, message: &str, now: DateTime<Utc>) -> Result<bool> {
    Ok(conn.execute(
        "UPDATE openscript_orders SET status = 'rejected', message = ?2, updated_at = ?3
         WHERE id = ?1 AND orderid IS NULL AND pending_order_id IS NOT NULL
           AND status IN ('working', ?4)",
        params![id, message, now.to_rfc3339(), UNCONFIRMED],
    )? > 0)
}
