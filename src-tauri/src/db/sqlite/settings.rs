//! Settings management

use crate::db::sqlite::models::Settings;
use crate::error::Result;
use rusqlite::Connection;

/// Get settings
pub fn get_settings(conn: &Connection) -> Result<Settings> {
    let settings = conn.query_row(
        "SELECT id, theme, default_broker, default_exchange, default_product,
                order_confirm, sound_enabled, auto_logout_enabled,
                auto_logout_hour, auto_logout_minute, auto_logout_warnings, analyze_mode
         FROM settings WHERE id = 1",
        [],
        |row| {
            let warnings_json: String = row.get(10)?;
            let warnings: Vec<u32> =
                serde_json::from_str(&warnings_json).unwrap_or(vec![30, 15, 5, 1]);
            let analyze_mode: Option<i32> = row.get(11).ok();

            Ok(Settings {
                id: row.get(0)?,
                theme: row.get(1)?,
                default_broker: row.get(2)?,
                default_exchange: row.get(3)?,
                default_product: row.get(4)?,
                order_confirm: row.get::<_, i32>(5)? == 1,
                sound_enabled: row.get::<_, i32>(6)? == 1,
                auto_logout_enabled: row.get::<_, i32>(7)? == 1,
                auto_logout_hour: row.get::<_, u32>(8)?,
                auto_logout_minute: row.get::<_, u32>(9)?,
                auto_logout_warnings: warnings,
                analyze_mode: analyze_mode.map(|v| v == 1),
            })
        },
    )?;

    Ok(settings)
}
