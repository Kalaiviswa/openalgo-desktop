//! Rupeezy adapter on the Vortex API (web `broker/rupeezy/**`, openalgo
//! 9aa6f2e30, #2193).
//!
//! What lives where:
//! * `auth`: the SSO redirect (`flow.rupeezy.in`, the code arrives as
//!   `auth`) and the `/user/session` exchange with
//!   `SHA256(application_id + auth + api_key)`.
//! * `client`: the one REST path. Bearer token plus the app's `x-api-key`,
//!   the web's per-category pacing, and for reads only a bounded retry (a
//!   dead pooled connection once, a 429 twice); writes are sent once.
//! * `mapping`: exchanges, tickers (`NSE:SBIN`), products, varieties,
//!   statuses and the MCX lot/unit conversion.
//! * `books`: order, trade, position and holding rows to OpenAlgo shapes.
//! * `orders`, `funds`, `data`, `gtt`: the endpoints.
//! * `master_contract`: the public `master.csv`.
//! * `streaming`: the little-endian `wire.rupeezy.in` price feed.
//!
//! Credentials (Broker Configuration): the API key is the Vortex
//! `application_id`; the API secret is the Vortex API key, sent as
//! `x-api-key` and part of the session checksum; the client id is the
//! Rupeezy account each sign-in must come back for.

mod auth;
mod books;
mod client;
mod data;
mod funds;
mod gtt;
pub mod mapping;
pub mod master_contract;
mod orders;
pub mod streaming;
#[cfg(test)]
mod tests;

pub use auth::checksum;
pub use client::order_path;

use crate::brokers::common::http;
use crate::brokers::common::mapping::{Exchange, Product};
use crate::brokers::common::ratelimit::Pacer;
use crate::brokers::common::streaming::BrokerFeed;
use crate::brokers::common::symbols::SymbolResolver;
use crate::brokers::types::*;
use crate::brokers::{AuthResponse, Broker, BrokerCredentials};
use crate::error::{AppError, Result};
use crate::security::Secret;
use async_trait::async_trait;
use parking_lot::Mutex;
use std::time::Duration;

/// REST host (web `api/baseurl.py`).
pub const REST_URL: &str = "https://vortex-api.rupeezy.in/v2";
/// Public instrument master, no sign-in needed.
pub const MASTER_URL: &str = "https://static.rupeezy.in/master.csv";
/// Price feed; the access token rides on the query string.
pub const WS_URL: &str = "wss://wire.rupeezy.in/ws";

/// web `plugin.json` supported_exchanges.
pub const SUPPORTED_EXCHANGES: &[Exchange] = &[
    Exchange::Nse,
    Exchange::Bse,
    Exchange::Nfo,
    Exchange::Bfo,
    Exchange::Mcx,
    Exchange::NseIndex,
    Exchange::BseIndex,
];

/// web `BrokerData.timeframe_map`: OpenAlgo interval -> Vortex resolution.
pub const TIMEFRAME_MAP: &[(&str, &str)] = &[
    ("1m", "1"),
    ("2m", "2"),
    ("3m", "3"),
    ("5m", "5"),
    ("10m", "10"),
    ("15m", "15"),
    ("30m", "30"),
    ("45m", "45"),
    ("1h", "60"),
    ("2h", "120"),
    ("3h", "180"),
    ("4h", "240"),
    ("D", "1D"),
    ("W", "1W"),
    ("M", "1M"),
];

/// Hosts the adapter talks to (tests point them at local fakes).
#[derive(Debug, Clone)]
pub struct RupeezyUrls {
    pub rest: String,
    pub master: String,
    pub ws: String,
}

impl Default for RupeezyUrls {
    fn default() -> Self {
        Self {
            rest: REST_URL.into(),
            master: MASTER_URL.into(),
            ws: WS_URL.into(),
        }
    }
}

impl RupeezyUrls {
    /// REST and master on one HTTP base, the feed on a socket base (tests).
    pub fn rebased(http_base: &str, ws_base: &str) -> Self {
        Self {
            rest: http_base.into(),
            master: format!("{}/master.csv", http_base),
            ws: ws_base.into(),
        }
    }
}

/// The web's per-category pacing (`api/rate_limiter.py`): Vortex allows one
/// call a second on quotes, history and `/user/*`, ten on trading; each
/// runs a little under its cap.
pub(crate) struct Pacers {
    quotes: Pacer,
    history: Pacer,
    account: Pacer,
    trading: Pacer,
}

impl Default for Pacers {
    fn default() -> Self {
        let one_a_second = || Pacer::with_interval(Duration::from_millis(1050));
        Self {
            quotes: one_a_second(),
            history: one_a_second(),
            account: one_a_second(),
            trading: Pacer::with_interval(Duration::from_millis(110)),
        }
    }
}

pub struct RupeezyBroker {
    http: reqwest::Client,
    urls: RupeezyUrls,
    symbols: SymbolResolver,
    pacers: Pacers,
    /// The app's Vortex API key (the saved API secret), sent as `x-api-key`
    /// on every authenticated call. Set by a sign-in or a resume, dropped at
    /// logout: the stored session token is the bare access token.
    api_key: Mutex<Option<Secret>>,
}

impl RupeezyBroker {
    pub fn new(symbols: SymbolResolver) -> Self {
        Self::with_base_url(symbols, RupeezyUrls::default())
    }

    /// Point the adapter at other hosts (tests run a local fake Vortex).
    pub fn with_base_url(symbols: SymbolResolver, urls: RupeezyUrls) -> Self {
        Self {
            http: http::client(),
            urls,
            symbols,
            pacers: Pacers::default(),
            api_key: Mutex::new(None),
        }
    }

    pub(crate) fn resolver(&self) -> &SymbolResolver {
        &self.symbols
    }

    fn remember_api_key(&self, key: Option<&str>) {
        *self.api_key.lock() = key
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(Secret::new);
    }

    pub(crate) fn api_key(&self) -> Option<Secret> {
        self.api_key.lock().clone()
    }

    /// The master row of an OpenAlgo instrument.
    pub(crate) fn instrument(&self, key: &QuoteKey) -> Result<SymbolData> {
        self.symbols
            .by_symbol(&key.exchange, &key.symbol)
            .ok_or_else(|| {
                AppError::Validation(format!(
                    "Symbol {} was not found on {}. Check the symbol, or download the master contract again from the broker page.",
                    key.symbol, key.exchange
                ))
            })
    }
}

#[async_trait]
impl Broker for RupeezyBroker {
    fn id(&self) -> &'static str {
        "rupeezy"
    }

    fn name(&self) -> &'static str {
        "Rupeezy"
    }

    fn logo(&self) -> &'static str {
        "/logos/rupeezy.svg"
    }

    fn login_kind(&self) -> LoginKind {
        LoginKind::Redirect { param: "auth" }
    }

    fn supported_exchanges(&self) -> &'static [Exchange] {
        SUPPORTED_EXCHANGES
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            history: true,
            multiquotes_batch: true,
            margin: false,
            gtt: true,
            streaming: true,
            order_feed: false,
            depth_levels: &[5],
        }
    }

    fn timeframe_map(&self) -> &'static [(&'static str, &'static str)] {
        TIMEFRAME_MAP
    }

    fn symbols(&self) -> Option<&SymbolResolver> {
        Some(&self.symbols)
    }

    async fn authenticate(&self, credentials: BrokerCredentials) -> Result<AuthResponse> {
        auth::authenticate(self, credentials).await
    }

    /// After a restart the `x-api-key` is not in memory: it comes back from
    /// the saved API secret.
    fn restore_session(&self, credentials: &BrokerCredentials) {
        self.remember_api_key(credentials.api_secret.as_deref());
    }

    async fn on_logout(&self) {
        self.remember_api_key(None);
    }

    async fn place_order(&self, auth: &AuthToken, order: &ResolvedOrder) -> Result<OrderResponse> {
        orders::place_order(self, auth, order).await
    }

    async fn modify_order(
        &self,
        auth: &AuthToken,
        order: &ResolvedModify,
    ) -> Result<OrderResponse> {
        orders::modify_order(self, auth, order).await
    }

    async fn cancel_order(&self, auth: &AuthToken, order_id: &str) -> Result<OrderResponse> {
        orders::cancel_order(self, auth, order_id).await
    }

    async fn cancel_all_orders(&self, auth: &AuthToken) -> Result<CancelAllResult> {
        orders::cancel_all_orders(self, auth).await
    }

    async fn close_all_positions(&self, auth: &AuthToken) -> Result<CloseAllResult> {
        orders::close_all_positions(self, auth).await
    }

    async fn get_open_position(
        &self,
        auth: &AuthToken,
        symbol: &str,
        exchange: Exchange,
        product: Product,
    ) -> Result<i64> {
        orders::get_open_position(self, auth, symbol, exchange, product).await
    }

    async fn get_order_book(&self, auth: &AuthToken) -> Result<Vec<Order>> {
        orders::get_order_book(self, auth).await
    }

    async fn get_trade_book(&self, auth: &AuthToken) -> Result<Vec<Trade>> {
        orders::get_trade_book(self, auth).await
    }

    async fn get_positions(&self, auth: &AuthToken) -> Result<Vec<Position>> {
        orders::get_positions(self, auth).await
    }

    async fn get_holdings(&self, auth: &AuthToken) -> Result<Vec<Holding>> {
        orders::get_holdings(self, auth).await
    }

    async fn get_funds(&self, auth: &AuthToken) -> Result<Funds> {
        funds::get_funds(self, auth).await
    }

    async fn get_quote(&self, auth: &AuthToken, key: &QuoteKey) -> Result<Quote> {
        data::get_quote(self, auth, key).await
    }

    async fn get_multiquotes(
        &self,
        auth: &AuthToken,
        keys: &[QuoteKey],
    ) -> Result<Vec<QuoteResult>> {
        data::get_multiquotes(self, auth, keys).await
    }

    async fn get_market_depth(&self, auth: &AuthToken, key: &QuoteKey) -> Result<MarketDepth> {
        data::get_market_depth(self, auth, key).await
    }

    async fn get_history(&self, auth: &AuthToken, req: &HistoryRequest) -> Result<Vec<Candle>> {
        data::get_history(self, auth, req).await
    }

    async fn place_gtt(&self, auth: &AuthToken, req: &GttRequest) -> Result<GttResponse> {
        gtt::place_gtt(self, auth, req).await
    }

    async fn modify_gtt(
        &self,
        auth: &AuthToken,
        trigger_id: &str,
        req: &GttRequest,
    ) -> Result<GttResponse> {
        gtt::modify_gtt(self, auth, trigger_id, req).await
    }

    async fn cancel_gtt(&self, auth: &AuthToken, trigger_id: &str) -> Result<GttResponse> {
        gtt::cancel_gtt(self, auth, trigger_id).await
    }

    async fn get_gtt_book(&self, auth: &AuthToken, include_history: bool) -> Result<Vec<GttOrder>> {
        gtt::get_gtt_book(self, auth, include_history).await
    }

    async fn download_master_contract(&self, _auth: &AuthToken) -> Result<Vec<SymbolData>> {
        master_contract::download(self).await
    }

    fn create_feed(&self, auth: &AuthToken) -> Result<Box<dyn BrokerFeed>> {
        Ok(Box::new(streaming::RupeezyFeed::new(
            &self.urls.ws,
            auth.raw(),
        )))
    }
}
