//! App settings kept in openalgo.db that belong to no other feature: the
//! server's ports and addresses (Profile, the start-up page), and the one
//! leverage value applied to crypto futures orders (web
//! `blueprints/leverage.py`).

use crate::config::ServerConfigUpdate;
use crate::db::sqlite::webui;
use crate::error::Result;
use crate::state::AppState;

/// Save server settings. The caller reloads them
/// (`AppState::reload_config`) when it needs the new values now.
pub fn save_server(state: &AppState, update: &ServerConfigUpdate) -> Result<()> {
    let conn = state.sqlite.conn()?;
    crate::config::save(&conn, update)
}

/// The leverage applied to crypto futures orders; 0 means the broker's
/// default.
pub fn leverage(state: &AppState) -> Result<f64> {
    let conn = state.sqlite.conn()?;
    webui::leverage(&conn)
}

/// Save the leverage applied to crypto futures orders.
pub fn set_leverage(state: &AppState, value: i64) -> Result<()> {
    let conn = state.sqlite.conn()?;
    webui::set_leverage(&conn, value)
}
