//! SQLite database models

use serde::{Deserialize, Serialize};

/// Settings model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub id: i64,
    pub theme: String,
    pub default_broker: Option<String>,
    pub default_exchange: String,
    pub default_product: String,
    pub order_confirm: bool,
    pub sound_enabled: bool,
    // Auto-logout configuration
    pub auto_logout_enabled: bool,
    pub auto_logout_hour: u32,
    pub auto_logout_minute: u32,
    pub auto_logout_warnings: Vec<u32>,
    // Analyze mode (sandbox/paper trading)
    pub analyze_mode: Option<bool>,
}
