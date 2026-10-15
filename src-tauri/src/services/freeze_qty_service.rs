//! The freeze quantity table the Admin page edits (web `blueprints/admin.py`
//! freeze routes, `database/qty_freeze_db.py`), in openalgo.db.

use crate::db::sqlite::webui::{self, FreezeQty};
use crate::error::Result;
use crate::state::AppState;

/// Every entry.
pub fn list(ctx: &AppState) -> Result<Vec<FreezeQty>> {
    let c = ctx.sqlite.conn()?;
    webui::freeze_list(&c)
}

/// How many entries there are, on one exchange or on all.
pub fn count(ctx: &AppState, exchange: Option<&str>) -> Result<i64> {
    let c = ctx.sqlite.conn()?;
    webui::freeze_count(&c, exchange)
}

/// Add an entry. `None`: the symbol already has one on that exchange.
pub fn add(ctx: &AppState, exchange: &str, symbol: &str, qty: i64) -> Result<Option<FreezeQty>> {
    let c = ctx.sqlite.conn()?;
    webui::freeze_add(&c, exchange, symbol, qty)
}

/// The entry `id`. An entry that cannot be read reads as missing (the
/// page answers "not found"); `Err` only when the database is unavailable.
pub fn get(ctx: &AppState, id: i64) -> Result<Option<FreezeQty>> {
    let c = ctx.sqlite.conn()?;
    Ok(webui::freeze_get(&c, id).ok().flatten())
}

/// Change the quantity of the entry `id`.
pub fn update(ctx: &AppState, id: i64, qty: i64) -> Result<bool> {
    let c = ctx.sqlite.conn()?;
    webui::freeze_update(&c, id, qty)
}

/// Remove the entry `id`.
pub fn delete(ctx: &AppState, id: i64) -> Result<bool> {
    let c = ctx.sqlite.conn()?;
    webui::freeze_delete(&c, id)
}

/// Replace every entry of `exchange` with `rows` (an uploaded exchange
/// file) and answer how many the exchange now has.
pub fn replace_exchange(ctx: &AppState, exchange: &str, rows: &[(String, i64)]) -> Result<i64> {
    let c = ctx.sqlite.conn()?;
    webui::freeze_replace_exchange(&c, exchange, rows)?;
    webui::freeze_count(&c, Some(exchange))
}
