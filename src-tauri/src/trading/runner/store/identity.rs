//! Who a run's orders belong to (NEW-TR-01, NEW-TR-03, NEW-TR-04).
//!
//! **A run is an incarnation.** Each start of a deployment writes one row to
//! `openscript_incarnations`; its id, unique and never reused (AUTOINCREMENT,
//! so not even after a deployment is removed and made again), is the run's
//! engine incarnation. A fresh engine numbers its intents from 1 again, so
//! an intent id means something only with its incarnation: every order row
//! records both, a page hears only about its own incarnation's rows, and
//! `(incarnation, intent_id)` is unique, so a page that asks again about an
//! intent is answered from its row and the order is never placed twice.
//!
//! **Every row records where it went**: the side, the broker and the account
//! (`services::live_owner::Destination`). A run's ledger is the rows of its
//! deployment that its destination owns, so a run started on another broker
//! or account never reads, reconciles, cancels or closes them.
//!
//! Migration `093_openscript_identity` adds both. Rows written before it
//! keep no incarnation, so no page hears of them again. Sandbox rows are
//! recorded as sandbox, which is all a sandbox destination is. Live rows
//! keep a blank broker: nothing recorded says which account placed them
//! (the active broker is the last one signed in to, not necessarily
//! theirs), so they belong to no run and the trader is told to close what
//! they hold at the broker. One still unconfirmed is the exception: the
//! broker switch guard kept it on the connected broker, and the order book
//! that finds it adopts it (`runner::reconcile`).

use super::{order_from_row, OrderRow, ORDER_COLUMNS};
use crate::db::sqlite::migrations::column_exists;
use crate::error::Result;
use crate::services::live_owner::{Destination, LiveOwner, SANDBOX_BROKER};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};

/// Migration `093_openscript_identity`. Checks before altering; idempotent.
pub fn migrate_identity(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS openscript_incarnations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            deployment TEXT NOT NULL,
            mode TEXT NOT NULL,
            broker TEXT NOT NULL,
            account TEXT NOT NULL DEFAULT '',
            started_at TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS ix_openscript_incarnations_deployment
            ON openscript_incarnations(deployment);",
    )?;
    for (column, kind) in [
        ("incarnation", "INTEGER"),
        ("broker", "TEXT NOT NULL DEFAULT ''"),
        ("account", "TEXT NOT NULL DEFAULT ''"),
    ] {
        if !column_exists(conn, "openscript_orders", column)? {
            conn.execute_batch(&format!(
                "ALTER TABLE openscript_orders ADD COLUMN {} {};",
                column, kind
            ))?;
        }
    }
    conn.execute(
        "UPDATE openscript_orders SET broker = ?1 WHERE mode = 'sandbox' AND broker = ''",
        params![SANDBOX_BROKER],
    )?;
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS ux_openscript_orders_intent
            ON openscript_orders(incarnation, intent_id)
            WHERE incarnation IS NOT NULL AND intent_id IS NOT NULL;",
    )?;
    Ok(())
}

/// Record the start of a run of `deployment` on `destination`; answers its
/// incarnation.
pub fn begin_incarnation(
    conn: &Connection,
    deployment: &str,
    destination: &Destination,
    now: DateTime<Utc>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO openscript_incarnations (deployment, mode, broker, account, started_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            deployment,
            destination.mode.as_str(),
            destination.owner.broker,
            destination.owner.account,
            now.to_rfc3339()
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// The row one intent of one run became, if it became one.
pub fn order_of_intent(
    conn: &Connection,
    incarnation: i64,
    intent_id: i64,
) -> Result<Option<OrderRow>> {
    let mut st = conn.prepare_cached(&format!(
        "SELECT {} FROM openscript_orders WHERE incarnation = ?1 AND intent_id = ?2",
        ORDER_COLUMNS
    ))?;
    Ok(st
        .query_row(params![incarnation, intent_id], order_from_row)
        .optional()?)
}

/// Record the session whose order book found a Live row that had none (a
/// row from before owners were recorded). False when it has one already.
pub fn adopt(conn: &Connection, id: i64, owner: &LiveOwner, now: DateTime<Utc>) -> Result<bool> {
    Ok(conn.execute(
        "UPDATE openscript_orders SET broker = ?2, account = ?3, updated_at = ?4
         WHERE id = ?1 AND mode = 'live' AND broker = ''",
        params![id, owner.broker, owner.account, now.to_rfc3339()],
    )? > 0)
}

#[cfg(test)]
mod tests {
    use super::super::{
        insert_order, migrate, migrate_pending_link, migrate_unconfirmed, orders_of, NewOrder,
    };
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 5, 4, 0, 0).unwrap()
    }

    fn legacy_schema() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        migrate(&c).unwrap();
        migrate_unconfirmed(&c).unwrap();
        migrate_pending_link(&c).unwrap();
        c
    }

    /// Migration 093 on a populated store: rows written before it keep
    /// their facts, sandbox rows are recorded as sandbox, Live rows keep no
    /// broker (nothing says which account placed them), and two legacy rows
    /// with the same intent number do not stop the unique key, which holds
    /// from here on. Running it again changes nothing.
    #[test]
    fn migration_093_records_what_is_known_and_keys_new_intents() {
        let c = legacy_schema();
        for (mode, intent) in [("live", 1), ("live", 1), ("sandbox", 1)] {
            c.execute(
                "INSERT INTO openscript_orders (deployment, mode, intent_id, symbol, exchange,
                    action, quantity, pricetype, product, status, filled_quantity, placed_at,
                    updated_at)
                 VALUES ('d', ?1, ?2, 'SBIN', 'NSE', 'BUY', 3, 'MARKET', 'MIS', 'filled', 3,
                    '2026-10-01T04:00:00+00:00', '2026-10-01T04:00:00+00:00')",
                params![mode, intent],
            )
            .unwrap();
        }
        migrate_identity(&c).unwrap();
        migrate_identity(&c).unwrap();
        let live = orders_of(&c, "d", "live").unwrap();
        assert_eq!(live.len(), 2);
        for o in &live {
            assert_eq!((o.incarnation, o.broker.as_str()), (None, ""));
            assert_eq!((o.filled_quantity, o.status.as_str()), (3, "filled"));
            assert!(!o.destination().unwrap().is_known());
        }
        let sandbox = orders_of(&c, "d", "sandbox").unwrap();
        assert_eq!(sandbox[0].broker, "sandbox");
        assert_eq!(sandbox[0].destination(), Some(Destination::sandbox()));

        let zerodha = Destination::live(LiveOwner::new("zerodha", "AB1234"));
        let run = begin_incarnation(&c, "d", &zerodha, now()).unwrap();
        let next = begin_incarnation(&c, "d", &zerodha, now()).unwrap();
        assert!(next > run, "never reused");
        let order = NewOrder {
            deployment: "d".into(),
            mode: "live".into(),
            intent_id: Some(1),
            symbol: "SBIN".into(),
            exchange: "NSE".into(),
            action: "BUY".into(),
            quantity: 3,
            pricetype: "MARKET".into(),
            product: "MIS".into(),
            incarnation: Some(run),
            broker: "zerodha".into(),
            account: "AB1234".into(),
            ..Default::default()
        };
        let id = insert_order(&c, &order, now()).unwrap();
        assert!(
            insert_order(&c, &order, now()).is_err(),
            "one row per intent of a run"
        );
        let other_run = NewOrder {
            incarnation: Some(next),
            ..order.clone()
        };
        insert_order(&c, &other_run, now()).unwrap();
        let found = order_of_intent(&c, run, 1).unwrap().unwrap();
        assert_eq!(found.id, id);
        assert_eq!(found.destination(), Some(zerodha));
        assert!(order_of_intent(&c, run, 2).unwrap().is_none());
    }

    /// A Live row with no recorded broker is adopted once, by the session
    /// whose order book found it; a recorded owner is never replaced.
    #[test]
    fn only_a_row_without_an_owner_is_adopted() {
        let c = legacy_schema();
        migrate_identity(&c).unwrap();
        let id = insert_order(
            &c,
            &NewOrder {
                deployment: "d".into(),
                mode: "live".into(),
                symbol: "SBIN".into(),
                exchange: "NSE".into(),
                action: "BUY".into(),
                quantity: 3,
                pricetype: "MARKET".into(),
                product: "MIS".into(),
                ..Default::default()
            },
            now(),
        )
        .unwrap();
        let zerodha = LiveOwner::new("zerodha", "AB1234");
        assert!(adopt(&c, id, &zerodha, now()).unwrap());
        assert!(!adopt(&c, id, &LiveOwner::new("angel", "XY9876"), now()).unwrap());
        let row = orders_of(&c, "d", "live").unwrap().remove(0);
        assert_eq!(
            (row.broker.as_str(), row.account.as_str()),
            ("zerodha", "AB1234")
        );
    }

    /// DB-04 per destination (NEW-TR-01): an old BUY 3 at Zerodha and an old
    /// SELL 3 at Angel One are two positions, not a flat one. Netted
    /// together, both rows were pruned and Zerodha's 3 vanished from the
    /// ledger.
    #[test]
    fn pruning_never_nets_one_accounts_rows_against_anothers() {
        let c = legacy_schema();
        migrate_identity(&c).unwrap();
        let old = now() - chrono::Duration::days(400);
        for (broker, action) in [("zerodha", "BUY"), ("angel", "SELL")] {
            let id = insert_order(
                &c,
                &NewOrder {
                    deployment: "d".into(),
                    mode: "live".into(),
                    symbol: "SBIN".into(),
                    exchange: "NSE".into(),
                    action: action.into(),
                    quantity: 3,
                    pricetype: "MARKET".into(),
                    product: "MIS".into(),
                    broker: broker.into(),
                    ..Default::default()
                },
                old,
            )
            .unwrap();
            super::super::placed(&c, id, Some("OID"), "filled", None, old).unwrap();
            super::super::progressed(&c, id, "filled", 3, Some(100.0), old).unwrap();
        }
        assert_eq!(super::super::prune_orders(&c, now()).unwrap(), 0);
        assert_eq!(orders_of(&c, "d", "live").unwrap().len(), 2);
    }
}
