//! The charting terminal's alert firing log, per signed-in user (web
//! `blueprints/alerts.py`), in openalgo.db.

use crate::db::sqlite::alert_log::{self as store, Fire};
use crate::error::Result;
use crate::state::AppState;
use serde_json::Value;

/// Record that an alert fired, now, and answer the logged row.
pub fn record(ctx: &AppState, user: &str, fire: &Fire) -> Result<Value> {
    let c = ctx.sqlite.conn()?;
    store::record(&c, user, fire, ctx.now())
}

/// The user's logged fires, newest first (`limit` as the page sent it).
pub fn list(ctx: &AppState, user: &str, limit: Option<&str>) -> Result<Vec<Value>> {
    let c = ctx.sqlite.conn()?;
    store::list(&c, user, limit)
}

/// Remove the user's logged fires, of one alert or of all; answers how
/// many went.
pub fn clear(ctx: &AppState, user: &str, alert_id: Option<&str>) -> Result<i64> {
    let c = ctx.sqlite.conn()?;
    store::clear(&c, user, alert_id)
}
