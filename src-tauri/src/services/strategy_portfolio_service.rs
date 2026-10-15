//! Saved Strategy Builder strategies in the `mytrades` and `simulation`
//! watchlists (web `blueprints/strategy_portfolio.py`), in openalgo.db.

use crate::db::sqlite::strategy_portfolio::{self as store, Entry};
use crate::error::Result;
use crate::state::AppState;
use serde_json::Value;

/// The saved strategies, of one watchlist or of both.
pub fn list(ctx: &AppState, watchlist: Option<&str>) -> Result<Vec<Value>> {
    let c = ctx.sqlite.conn()?;
    store::list(&c, watchlist)
}

/// The saved strategy `id`, if any.
pub fn get(ctx: &AppState, id: i64) -> Result<Option<Value>> {
    let c = ctx.sqlite.conn()?;
    store::get(&c, id)
}

/// Save a new strategy (`id` `None`) or replace the strategy `id`, stamped
/// now. `None`: nothing was saved (no such strategy to replace).
pub fn save(ctx: &AppState, id: Option<i64>, entry: &Entry) -> Result<Option<Value>> {
    let c = ctx.sqlite.conn()?;
    store::save(&c, id, entry, ctx.now())
}

/// Delete the strategy `id`. `false`: no such strategy.
pub fn delete(ctx: &AppState, id: i64) -> Result<bool> {
    let c = ctx.sqlite.conn()?;
    store::delete(&c, id)
}
