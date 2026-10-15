//! The charting terminal's watchlists, per signed-in user (web
//! `blueprints/watchlist.py`), in openalgo.db. Every call names the user,
//! and a list of another user reads as missing.

use crate::db::sqlite::watchlist as store;
use crate::error::Result;
use crate::state::AppState;
use serde_json::Value;

pub use store::MAX_ITEMS_PER_LIST;

/// The user's lists with their instruments.
pub fn lists(ctx: &AppState, user: &str) -> Result<Vec<Value>> {
    let c = ctx.sqlite.conn()?;
    store::lists(&c, user)
}

/// Create a list, optionally with `(symbol, exchange)` items. `None`: the
/// name is taken or the user has as many lists as allowed.
pub fn create(
    ctx: &AppState,
    user: &str,
    name: &str,
    items: &[(String, String)],
) -> Result<Option<Value>> {
    let c = ctx.sqlite.conn()?;
    store::create(&c, user, name, items)
}

/// Rename a list. `false`: no such list, or the name is taken.
pub fn rename(ctx: &AppState, user: &str, id: i64, name: &str) -> Result<bool> {
    let c = ctx.sqlite.conn()?;
    store::rename(&c, user, id, name)
}

/// Delete a list. `false`: no such list.
pub fn delete(ctx: &AppState, user: &str, id: i64) -> Result<bool> {
    let c = ctx.sqlite.conn()?;
    store::delete(&c, user, id)
}

/// Remove every instrument from a list. `false`: no such list.
pub fn clear(ctx: &AppState, user: &str, id: i64) -> Result<bool> {
    let c = ctx.sqlite.conn()?;
    store::clear(&c, user, id)
}

/// Add an instrument (an existing one is answered as it is). `None`: no
/// such list, or it is full.
pub fn add_item(
    ctx: &AppState,
    user: &str,
    id: i64,
    symbol: &str,
    exchange: &str,
) -> Result<Option<Value>> {
    let c = ctx.sqlite.conn()?;
    store::add_item(&c, user, id, symbol, exchange)
}

/// Remove an instrument. `false`: no such list or instrument.
pub fn remove_item(ctx: &AppState, user: &str, id: i64, item_id: i64) -> Result<bool> {
    let c = ctx.sqlite.conn()?;
    store::remove_item(&c, user, id, item_id)
}

/// Put a list's instruments in `order` (item ids). `false`: no such list.
pub fn reorder(ctx: &AppState, user: &str, id: i64, order: &[i64]) -> Result<bool> {
    let c = ctx.sqlite.conn()?;
    store::reorder(&c, user, id, order)
}
