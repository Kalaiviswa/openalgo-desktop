//! The WhatsApp settings, linked users and command statistics in
//! openalgo.db (web `blueprints/whatsapp.py` with
//! `database/whatsapp_db.py`): what the `/whatsapp` page reads and changes,
//! and the settings the bot reads.

use super::db::{self, WaUser};
use super::WhatsAppService;
use crate::error::Result;
use crate::state::AppState;
use serde_json::{Map, Value};

impl WhatsAppService {
    /// The settings; the defaults when they cannot be read (logged).
    pub fn config(ctx: &AppState) -> db::WaConfig {
        ctx.sqlite
            .conn()
            .and_then(|c| db::get_config(&c))
            .unwrap_or_else(|e| {
                tracing::error!("Could not read the WhatsApp settings: {}", e);
                db::WaConfig::default()
            })
    }

    /// Whether a WhatsApp account is paired.
    pub fn is_paired(ctx: &AppState) -> bool {
        Self::config(ctx).is_paired
    }
}

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
