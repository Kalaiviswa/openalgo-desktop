//! Live orders still unconfirmed from before this version recorded the
//! account an order went through (migration 094; NEW-TR-01).
//!
//! Such a row may belong to another broker's session than the one connected
//! now, so no book read here can say it was not placed: it is never
//! released or held as absent, never counted as unresolved, and no book is
//! read for it alone (`reconcile`). A book read anyway that finds it adopts
//! it. Otherwise the trader settles it: it is listed with the held orders on
//! the Health page, and once they have checked their broker their mark sets
//! it aside (after the reconciler's last look at the books, as for any held
//! order). It is told in the deployment's `elsewhere` meanwhile.

use super::{store, Runner};
use crate::services::order_reconciler::HeldOrder;

/// What the row reads once the trader has checked it at their broker.
const CHECKED: &str = "You checked this order at your broker; OpenAlgo does not follow it";

/// Whether `o` is a Live row whose account was never recorded.
pub(super) fn is_unowned_live(o: &store::OrderRow) -> bool {
    o.mode == "live" && o.broker.is_empty()
}

impl Runner {
    /// The unowned unconfirmed rows, as held orders for the trader to check.
    pub(super) fn unowned_held(&self) -> Vec<HeldOrder> {
        let rows = match self
            .db
            .conn()
            .and_then(|c| store::unconfirmed_rows(&c, "live"))
        {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!("Unconfirmed OpenScript orders could not be read: {}", e);
                return Vec::new();
            }
        };
        rows.into_iter()
            .filter(is_unowned_live)
            .map(|o| {
                let placed_at = chrono::DateTime::parse_from_rfc3339(&o.placed_at)
                    .ok()
                    .map(|t| t.with_timezone(&chrono::Utc));
                HeldOrder {
                    owner: String::new(),
                    id: o.id.to_string(),
                    message: format!(
                        "OpenScript {}: {} {} {} may have been placed at your broker before this version recorded which account it went through. Check your broker's order book, then mark it here.",
                        o.deployment, o.action, o.quantity, o.symbol
                    ),
                    symbol: o.symbol,
                    exchange: o.exchange,
                    action: o.action,
                    quantity: o.quantity,
                    since: placed_at.unwrap_or_else(|| self.clock.now()),
                    product: Some(o.product),
                    client_tag: o.client_tag,
                    placed_at,
                }
            })
            .collect()
    }

    /// The trader checked an unowned unconfirmed row at their broker: it is
    /// set aside as checked. False when it is not one (any more).
    pub(super) fn dismiss_unowned(&self, id: &str) -> Result<bool, String> {
        let Ok(row_id) = id.parse::<i64>() else {
            return Ok(false);
        };
        let conn = self.db.conn().map_err(|e| {
            tracing::error!("OpenScript order {} could not be read: {}", id, e);
            "The order could not be read. Try again.".to_string()
        })?;
        let row = store::order_by_id(&conn, row_id).map_err(|e| {
            tracing::error!("OpenScript order {} could not be read: {}", id, e);
            "The order could not be read. Try again.".to_string()
        })?;
        let Some(o) = row.filter(|o| is_unowned_live(o) && o.status == store::UNCONFIRMED) else {
            return Ok(false);
        };
        let done =
            store::release_unconfirmed(&conn, o.id, CHECKED, self.clock.now()).map_err(|e| {
                tracing::error!("OpenScript order {} could not be marked: {}", id, e);
                "The order could not be marked just now. Nothing was changed. Try again."
                    .to_string()
            })?;
        drop(conn);
        if done {
            self.tell(
                &o,
                &format!(
                    "{} {} {} from before this version recorded the account: {}.",
                    o.action, o.quantity, o.symbol, CHECKED
                ),
            );
        }
        Ok(done)
    }
}
