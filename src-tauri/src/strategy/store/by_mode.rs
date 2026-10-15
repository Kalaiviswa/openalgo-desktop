//! A strategy's history by the mode of the run that made it (SM-03,
//! SM-05): its order rows, each with its run's mode, and its realized P&L
//! per mode. Views and analytics attribute to Live or Sandbox by the run:
//! never by which book is being read, and never by the at most 100 runs a
//! page has loaded.

use super::{fnum, order_from, OrderRow, Store, ORDER_COLS, RUN_MODES};
use crate::error::Result;
use rusqlite::params;
use serde_json::{json, Map, Value};

/// An order row and the mode of the run that placed it.
#[derive(Debug, Clone, PartialEq)]
pub struct ModedOrder {
    pub order: OrderRow,
    /// `live` or `sandbox`: `sm_strategy_run.mode` of the order's run.
    pub mode: String,
}

impl ModedOrder {
    /// The order's dict with its run's `mode`, for the Detail page. The
    /// `/api/v1/strategy/orders` answer keeps the web's shape
    /// (`OrderRow::to_dict`).
    pub fn to_dict(&self) -> Value {
        let mut dict = self.order.to_dict();
        dict["mode"] = json!(self.mode);
        dict
    }
}

impl Store {
    /// Orders across a strategy's runs, oldest first, optionally narrowed to
    /// one run (web `list_orders_for_strategy`), each with its run's mode.
    pub fn list_orders_for_strategy(
        &self,
        strategy_id: i64,
        run_id: Option<i64>,
    ) -> Result<Vec<ModedOrder>> {
        let conn = self.conn()?;
        let cols = ORDER_COLS
            .split(", ")
            .map(|c| format!("o.{}", c.trim()))
            .collect::<Vec<_>>()
            .join(", ");
        let mut stmt = conn.prepare(&format!(
            "SELECT {}, r.mode FROM sm_strategy_order o JOIN sm_strategy_run r ON o.run_id = r.id \
             WHERE r.strategy_id = ?1 AND (?2 IS NULL OR o.run_id = ?2) \
             ORDER BY o.placed_at ASC, o.id ASC",
            cols
        ))?;
        let rows = stmt
            .query_map(params![strategy_id, run_id], |r| {
                Ok(ModedOrder {
                    order: order_from(r)?,
                    mode: r.get(ORDER_COLS.split(", ").count())?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Realized P&L of every finalised run, summed per mode over the
    /// strategy's whole history: `{"live": x, "sandbox": y}`.
    pub fn realized_by_mode(&self, strategy_id: i64) -> Result<Value> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT mode, COALESCE(SUM(pnl_realized), 0) FROM sm_strategy_run \
             WHERE strategy_id = ?1 AND stopped_at IS NOT NULL GROUP BY mode",
        )?;
        let sums = stmt
            .query_map([strategy_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut totals: Map<String, Value> = RUN_MODES
            .iter()
            .map(|m| (m.to_string(), fnum(0.0)))
            .collect();
        for (mode, sum) in sums {
            totals.insert(mode, fnum(sum));
        }
        Ok(Value::Object(totals))
    }
}
