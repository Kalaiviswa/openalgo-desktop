//! What the `/whatsapp` page reads and changes in openalgo.db (web
//! `blueprints/whatsapp.py` with `database/whatsapp_db.py`): the settings,
//! the linked users and the command statistics.

use super::db::{self, WaUser};
use crate::error::Result;
use crate::state::AppState;
use serde_json::{Map, Value};

/// Change the settings named in `updates` (the page's field names).
pub fn update_config(ctx: &AppState, updates: &Map<String, Value>) -> Result<()> {
    let c = ctx.sqlite.conn()?;
    db::update_config(&c, updates, ctx.now())
}

/// Every linked user.
pub fn users(ctx: &AppState) -> Result<Vec<WaUser>> {
    let c = ctx.sqlite.conn()?;
    db::all_users(&c, None, None)
}

/// The WhatsApp user linked to an OpenAlgo account.
pub fn user_by_username(ctx: &AppState, username: &str) -> Result<Option<WaUser>> {
    let c = ctx.sqlite.conn()?;
    db::get_user_by_username(&c, username)
}

/// Unlink a user. `false`: no such user.
pub fn unlink_user(ctx: &AppState, jid: &str) -> Result<bool> {
    let c = ctx.sqlite.conn()?;
    db::delete_user(&c, jid, ctx.now())
}

/// Bot commands of the last `days`, in the page's shape.
pub fn command_stats(ctx: &AppState, days: i64) -> Result<Value> {
    let c = ctx.sqlite.conn()?;
    db::command_stats(&c, days, ctx.now())
}
