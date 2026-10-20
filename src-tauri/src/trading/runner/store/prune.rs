//! Pruning the runner's order rows by age without erasing an obligation
//! (DB-04).

use super::{ORDER_RETENTION_DAYS, TERMINAL};
use crate::error::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, TransactionBehavior};
use std::collections::BTreeMap;

/// Drop old order rows, but never an obligation (DB-04).
///
/// A deployment's position is what its own rows add up to, so deleting rows
/// by age alone invents or erases a position: a BUY 100 from 200 days ago
/// pruned beside a SELL 50 kept from 100 days ago reads as short 50, and Stop
/// would then buy 50 more. So, for each contract a deployment traded on each
/// side, only its oldest rows go, and only up to the last point at which every
/// one of them was finished (filled, cancelled, rejected or expired), older
/// than the retention, and the contract was flat. A position still held, an
/// order still working and everything after either stay, so what is left adds
/// up to exactly what it did before. Read and delete are one immediate
/// transaction.
pub fn prune_orders(conn: &Connection, now: DateTime<Utc>) -> Result<usize> {
    type Key = (String, String, String, String, String);
    let cutoff = (now - chrono::Duration::days(ORDER_RETENTION_DAYS)).to_rfc3339();
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let mut groups: BTreeMap<Key, Vec<(i64, i64, bool)>> = BTreeMap::new();
    {
        let mut st = tx.prepare(
            "SELECT id, deployment, mode, symbol, exchange, product, action, filled_quantity,
                    status, placed_at
             FROM openscript_orders ORDER BY id",
        )?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            let key: Key = (r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?);
            let filled = r.get::<_, i64>(7)?.max(0);
            let signed = match r.get::<_, String>(6)?.as_str() {
                "BUY" => filled,
                "SELL" => -filled,
                _ => 0,
            };
            let status: String = r.get(8)?;
            let placed_at: String = r.get(9)?;
            let finished = TERMINAL.contains(&status.as_str()) && placed_at < cutoff;
            groups
                .entry(key)
                .or_default()
                .push((r.get(0)?, signed, finished));
        }
    }
    let mut gone = Vec::new();
    for rows in groups.values() {
        let mut net: i64 = 0;
        let mut flat_through = 0;
        for (n, (_, signed, finished)) in rows.iter().enumerate() {
            if !finished {
                break;
            }
            net = net.saturating_add(*signed);
            if net == 0 {
                flat_through = n + 1;
            }
        }
        gone.extend(rows[..flat_through].iter().map(|(id, _, _)| *id));
    }
    {
        let mut del = tx.prepare_cached("DELETE FROM openscript_orders WHERE id = ?1")?;
        for id in &gone {
            del.execute(params![id])?;
        }
    }
    tx.commit()?;
    Ok(gone.len())
}
