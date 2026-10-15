//! SQLite database module (main database, `openalgo.db`).
//!
//! All access goes through an r2d2 pool. A connection is checked out for the
//! duration of one synchronous call and returned when it drops; no method
//! holds a connection across an `.await`.

pub mod action_center;
pub mod alert_log;
pub mod api_keys;
pub mod auth;
pub mod credentials;
pub mod data_migrations;
pub mod key_vault;
pub mod logs;
mod market;
pub mod market_calendar;
pub mod master_contract_status;
pub(crate) mod migrations;
pub mod models;
pub mod monitor;
pub mod oauth_state;
pub mod rate_plan;
pub mod sandbox;
mod settings;
mod strategy;
pub mod strategy_portfolio;
pub mod symbol;
pub mod user;
pub mod watchlist;
pub mod webui;

use crate::error::Result;
use crate::state::SymbolInfo;
pub use market::{CreateHolidayRequest, MarketHoliday, MarketTiming, UpdateTimingRequest};
use models::*;
pub use models::{AutoLogoutConfig, SandboxFunds, SandboxHolding, WebhookConfig};
use r2d2::{Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;
use std::path::Path;
use std::time::Duration;

pub type DbConn = PooledConnection<SqliteConnectionManager>;

/// Build a pool with the pragmas every connection needs.
pub fn open_pool(path: &Path, max_size: u32) -> Result<Pool<SqliteConnectionManager>> {
    let manager = SqliteConnectionManager::file(path).with_init(|c| {
        c.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; \
             PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;",
        )
    });
    let pool = Pool::builder()
        .max_size(max_size)
        .min_idle(Some(1))
        .connection_timeout(Duration::from_secs(10))
        .idle_timeout(Some(Duration::from_secs(300)))
        .build(manager)?;
    crate::security::fsperm::restrict_db_files(path)?;
    Ok(pool)
}

/// SQLite database wrapper
pub struct SqliteDb {
    pool: Pool<SqliteConnectionManager>,
    /// Analyzer mode as last read or written here, its only writer. Every
    /// order path routes on it, so it is kept in memory: a busy pool must not
    /// turn the read into a default. `None` until the database has answered.
    analyze_mode: parking_lot::Mutex<Option<bool>>,
}

impl SqliteDb {
    /// Open (creating if needed) the main database and run schema migrations.
    pub fn new(path: &Path) -> Result<Self> {
        let db = Self {
            pool: open_pool(path, 8)?,
            analyze_mode: parking_lot::Mutex::new(None),
        };
        db.run_migrations()?;
        crate::security::fsperm::restrict_db_files(path)?;
        // Read once now, so no order waits on the pool for it later. A
        // failure leaves it unknown, and order paths refuse until it is read.
        if let Err(e) = db.get_analyze_mode() {
            tracing::error!("Could not read analyzer mode at start-up: {}", e);
        }
        Ok(db)
    }

    /// Check out a pooled connection. Drop it before any network await.
    pub fn conn(&self) -> Result<DbConn> {
        Ok(self.pool.get()?)
    }

    /// Open and idle pooled connections (health monitor).
    pub fn pool_state(&self) -> (u32, u32) {
        let st = self.pool.state();
        (st.connections, st.idle_connections)
    }

    /// Run database migrations
    fn run_migrations(&self) -> Result<()> {
        let conn = self.conn()?;
        migrations::run_migrations(&conn)
    }

    /// Whether a named migration (schema or data) has been recorded.
    pub fn migration_applied(&self, name: &str) -> Result<bool> {
        let conn = self.conn()?;
        migrations::is_applied(&conn, name)
    }
    // ========== Symbol Methods ==========

    /// Store `broker`'s symbols in database, with their owner.
    pub fn store_symbols(
        &self,
        broker: &str,
        now: chrono::DateTime<chrono::Utc>,
        symbols: &[SymbolInfo],
    ) -> Result<()> {
        let mut conn = self.conn()?;
        symbol::store_symbols(&mut conn, broker, now, symbols)
    }

    /// Load all symbols from database
    pub fn load_symbols(&self) -> Result<Vec<SymbolInfo>> {
        let conn = self.conn()?;
        symbol::load_symbols(&conn)
    }

    /// Store `broker`'s master download with its contract multipliers
    /// (crypto) and its owner.
    pub fn store_master(
        &self,
        broker: &str,
        now: chrono::DateTime<chrono::Utc>,
        master: &crate::brokers::types::MasterContract,
    ) -> Result<()> {
        let mut conn = self.conn()?;
        symbol::store_master(&mut conn, broker, now, master)
    }

    /// Load the master with its contract multipliers.
    pub fn load_master(&self) -> Result<crate::brokers::types::MasterContract> {
        let conn = self.conn()?;
        symbol::load_master(&conn)
    }

    // ========== Strategy Methods ==========

    /// Get all strategies
    pub fn get_strategies(&self) -> Result<Vec<Strategy>> {
        let conn = self.conn()?;
        strategy::get_strategies(&conn)
    }

    /// Create a new strategy
    pub fn create_strategy(&self, strategy: &Strategy) -> Result<Strategy> {
        let conn = self.conn()?;
        strategy::create_strategy(&conn, strategy)
    }

    /// Update a strategy
    #[allow(clippy::too_many_arguments)]
    pub fn update_strategy(
        &self,
        id: i64,
        name: Option<String>,
        exchange: Option<String>,
        symbol: Option<String>,
        product: Option<String>,
        quantity: Option<i32>,
        enabled: Option<bool>,
    ) -> Result<Strategy> {
        let conn = self.conn()?;
        strategy::update_strategy(
            &conn, id, name, exchange, symbol, product, quantity, enabled,
        )
    }

    /// Delete a strategy
    pub fn delete_strategy(&self, id: i64) -> Result<()> {
        let conn = self.conn()?;
        strategy::delete_strategy(&conn, id)
    }

    /// Get strategy by webhook_id (for webhook handler)
    pub fn get_strategy_by_webhook_id(
        &self,
        webhook_id: &str,
    ) -> Result<Option<crate::webhook::handlers::Strategy>> {
        let conn = self.conn()?;
        strategy::get_strategy_by_webhook_id(&conn, webhook_id)
    }

    /// Get symbol mapping for a strategy (for webhook handler)
    pub fn get_symbol_mapping(
        &self,
        strategy_id: &i64,
        symbol: &str,
    ) -> Result<Option<crate::webhook::handlers::SymbolMapping>> {
        let conn = self.conn()?;
        strategy::get_symbol_mapping(&conn, *strategy_id, symbol)
    }

    // ========== Settings Methods ==========

    /// Get settings
    pub fn get_settings(&self) -> Result<Settings> {
        let conn = self.conn()?;
        settings::get_settings(&conn)
    }

    /// Update settings
    pub fn update_settings(
        &self,
        theme: Option<String>,
        default_broker: Option<String>,
        default_exchange: Option<String>,
        default_product: Option<String>,
        order_confirm: Option<bool>,
        sound_enabled: Option<bool>,
    ) -> Result<Settings> {
        let conn = self.conn()?;
        settings::update_settings(
            &conn,
            theme,
            default_broker,
            default_exchange,
            default_product,
            order_confirm,
            sound_enabled,
        )
    }

    /// Get auto-logout configuration
    pub fn get_auto_logout_config(&self) -> Result<AutoLogoutConfig> {
        let conn = self.conn()?;
        settings::get_auto_logout_config(&conn)
    }

    /// Update auto-logout configuration
    pub fn update_auto_logout_config(
        &self,
        enabled: Option<bool>,
        hour: Option<u32>,
        minute: Option<u32>,
        warnings: Option<Vec<u32>>,
    ) -> Result<AutoLogoutConfig> {
        let conn = self.conn()?;
        settings::update_auto_logout_config(&conn, enabled, hour, minute, warnings)
    }

    /// Get webhook configuration
    pub fn get_webhook_config(&self) -> Result<WebhookConfig> {
        let conn = self.conn()?;
        settings::get_webhook_config(&conn)
    }

    /// Update webhook configuration
    pub fn update_webhook_config(
        &self,
        enabled: Option<bool>,
        port: Option<u16>,
        host: Option<String>,
        ngrok_url: Option<String>,
        webhook_secret: Option<String>,
    ) -> Result<WebhookConfig> {
        let conn = self.conn()?;
        settings::update_webhook_config(&conn, enabled, port, host, ngrok_url, webhook_secret)
    }

    /// Get rate limit configuration
    pub fn get_rate_limit_config(&self) -> Result<models::RateLimitConfig> {
        let conn = self.conn()?;
        settings::get_rate_limit_config(&conn)
    }

    /// Update rate limit configuration
    pub fn update_rate_limit_config(
        &self,
        api_rate_limit: Option<u32>,
        order_rate_limit: Option<u32>,
        smart_order_rate_limit: Option<u32>,
        smart_order_delay: Option<f64>,
    ) -> Result<models::RateLimitConfig> {
        let conn = self.conn()?;
        settings::update_rate_limit_config(
            &conn,
            api_rate_limit,
            order_rate_limit,
            smart_order_rate_limit,
            smart_order_delay,
        )
    }

    // ========== Sandbox Methods ==========

    /// Get sandbox positions
    pub fn get_sandbox_positions(&self) -> Result<Vec<SandboxPosition>> {
        let conn = self.conn()?;
        sandbox::get_positions(&conn)
    }

    /// Get sandbox orders
    pub fn get_sandbox_orders(&self) -> Result<Vec<SandboxOrder>> {
        let conn = self.conn()?;
        sandbox::get_orders(&conn)
    }

    /// Place sandbox order
    #[allow(clippy::too_many_arguments)]
    pub fn place_sandbox_order(
        &self,
        symbol: &str,
        exchange: &str,
        side: &str,
        quantity: i32,
        price: f64,
        order_type: &str,
        product: &str,
    ) -> Result<SandboxOrder> {
        let conn = self.conn()?;
        sandbox::place_order(
            &conn, symbol, exchange, side, quantity, price, order_type, product,
        )
    }

    /// Reset sandbox
    pub fn reset_sandbox(&self) -> Result<()> {
        let conn = self.conn()?;
        sandbox::reset(&conn)
    }

    /// Get sandbox holdings
    pub fn get_sandbox_holdings(&self) -> Result<Vec<SandboxHolding>> {
        let conn = self.conn()?;
        sandbox::get_holdings(&conn)
    }

    /// Get sandbox funds
    pub fn get_sandbox_funds(&self) -> Result<SandboxFunds> {
        let conn = self.conn()?;
        sandbox::get_funds(&conn)
    }

    /// Update sandbox LTP and recalculate P&L
    pub fn update_sandbox_ltp(&self, exchange: &str, symbol: &str, ltp: f64) -> Result<()> {
        let conn = self.conn()?;
        sandbox::update_position_ltp(&conn, exchange, symbol, ltp)
    }

    /// Cancel sandbox order
    pub fn cancel_sandbox_order(&self, order_id: &str) -> Result<bool> {
        let conn = self.conn()?;
        sandbox::cancel_order(&conn, order_id)
    }

    /// Get sandbox configuration
    pub fn get_sandbox_config(&self) -> Result<sandbox::SandboxConfig> {
        let conn = self.conn()?;
        sandbox::get_config(&conn)
    }

    /// Update sandbox configuration
    pub fn update_sandbox_config(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn()?;
        sandbox::update_config(&conn, key, value)
    }

    /// Get sandbox trades
    pub fn get_sandbox_trades(&self) -> Result<Vec<sandbox::SandboxTrade>> {
        let conn = self.conn()?;
        sandbox::get_trades(&conn)
    }

    /// Get sandbox daily P&L history
    pub fn get_sandbox_daily_pnl(&self) -> Result<Vec<sandbox::SandboxDailyPnl>> {
        let conn = self.conn()?;
        sandbox::get_daily_pnl(&conn)
    }

    /// Get consolidated sandbox P&L data
    pub fn get_sandbox_pnl(&self) -> Result<sandbox::SandboxPnlData> {
        let conn = self.conn()?;
        sandbox::get_pnl_data(&conn)
    }

    // ========== Market Holiday Methods ==========

    /// Create a market holiday
    pub fn create_market_holiday(&self, req: &CreateHolidayRequest) -> Result<MarketHoliday> {
        let conn = self.conn()?;
        market::create_holiday(&conn, req)
    }

    /// Get holidays by year
    pub fn get_market_holidays_by_year(&self, year: i32) -> Result<Vec<MarketHoliday>> {
        let conn = self.conn()?;
        market::get_holidays_by_year(&conn, year)
    }

    /// Get holidays by exchange
    pub fn get_market_holidays_by_exchange(
        &self,
        exchange: &str,
        year: Option<i32>,
    ) -> Result<Vec<MarketHoliday>> {
        let conn = self.conn()?;
        market::get_holidays_by_exchange(&conn, exchange, year)
    }

    /// Check if a date is a holiday
    pub fn is_market_holiday(&self, exchange: &str, date: &str) -> Result<bool> {
        let conn = self.conn()?;
        market::is_holiday(&conn, exchange, date)
    }

    /// Delete a market holiday
    pub fn delete_market_holiday(&self, id: i64) -> Result<bool> {
        let conn = self.conn()?;
        market::delete_holiday(&conn, id)
    }

    // ========== Market Timing Methods ==========

    /// Get all market timings
    pub fn get_all_market_timings(&self) -> Result<Vec<MarketTiming>> {
        let conn = self.conn()?;
        market::get_all_timings(&conn)
    }

    /// Get timing for an exchange
    pub fn get_market_timing(&self, exchange: &str) -> Result<Option<MarketTiming>> {
        let conn = self.conn()?;
        market::get_timing_by_exchange(&conn, exchange)
    }

    /// Update market timing
    pub fn update_market_timing(
        &self,
        exchange: &str,
        req: &UpdateTimingRequest,
    ) -> Result<MarketTiming> {
        let conn = self.conn()?;
        market::update_timing(&conn, exchange, req)
    }

    /// Create market timing
    pub fn create_market_timing(&self, timing: &MarketTiming) -> Result<MarketTiming> {
        let conn = self.conn()?;
        market::create_timing(&conn, timing)
    }

    /// Check if market is open
    pub fn is_market_open(&self, exchange: &str) -> Result<bool> {
        let conn = self.conn()?;
        market::is_market_open(&conn, exchange)
    }

    // ========== Analyze Mode Methods ==========

    /// Whether analyzer (sandbox) mode is on: the value held in memory, read
    /// from the database only until it has answered once. An error means it
    /// has never been readable, and callers that route orders refuse on it.
    pub fn get_analyze_mode(&self) -> Result<bool> {
        let mut known = self.analyze_mode.lock();
        if let Some(on) = *known {
            return Ok(on);
        }
        let on = self.get_settings()?.analyze_mode.unwrap_or(false);
        *known = Some(on);
        Ok(on)
    }

    /// Set analyzer mode. The write and the value in memory change under one
    /// lock, after the database has taken it, so a reader never sees a mode
    /// the database does not hold.
    pub fn set_analyze_mode(&self, enabled: bool) -> Result<()> {
        let mut known = self.analyze_mode.lock();
        let conn = self.conn()?;
        conn.execute("UPDATE settings SET analyze_mode = ?1", [enabled])?;
        *known = Some(enabled);
        Ok(())
    }

    // ========== Configured Brokers Methods (Keychain Optimization) ==========

    /// Mark a broker as configured (called when credentials are saved)
    pub fn mark_broker_configured(&self, broker_id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO configured_brokers (broker_id) VALUES (?1)",
            [broker_id],
        )?;
        Ok(())
    }

    /// Remove broker from configured list (called when credentials are deleted)
    pub fn unmark_broker_configured(&self, broker_id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM configured_brokers WHERE broker_id = ?1",
            [broker_id],
        )?;
        Ok(())
    }

    /// Get list of configured broker IDs
    pub fn get_configured_brokers(&self) -> Result<Vec<String>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare("SELECT broker_id FROM configured_brokers")?;
        let brokers = stmt
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?;
        Ok(brokers)
    }

    /// Check if a broker is configured
    pub fn is_broker_configured(&self, broker_id: &str) -> Result<bool> {
        let conn = self.conn()?;
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM configured_brokers WHERE broker_id = ?1)",
            [broker_id],
            |row| row.get(0),
        )?;
        Ok(exists)
    }
}
