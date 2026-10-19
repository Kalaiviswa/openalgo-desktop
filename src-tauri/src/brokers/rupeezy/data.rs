//! Quotes, depth and history (web `api/data.py`,
//! https://vortex.rupeezy.in/docs/latest/historical/).
//!
//! * `GET /data/quotes?q=<ticker>&q=..&mode=full`: up to 1000 tickers a
//!   call, a map keyed by the ticker as sent; tickers with no quote are left
//!   out. Prices in rupees.
//! * `GET /data/history?ticker=&from=&to=&resolution=`: epoch seconds and
//!   parallel `t o h l c v` arrays under `s: ok`.

use super::client::{message_of, Call};
use super::mapping::row_ticker;
use super::orders::QUOTE_BATCH;
use super::{RupeezyBroker, TIMEFRAME_MAP};
use crate::brokers::common::history::{chunks, sort_dedupe};
use crate::brokers::common::streaming::round2;
use crate::brokers::types::*;
use crate::error::{AppError, Result};
use chrono::NaiveDate;
use serde_json::{Map, Value};
use std::collections::{BTreeSet, HashMap};

const IST_OFFSET_SECONDS: i64 = 19_800;
const DAY_SECONDS: i64 = 86_400;
/// Days per history call (the web's conservative chunks; Vortex documents
/// no cap).
const DAILY_CHUNK_DAYS: i64 = 1000;
const INTRADAY_CHUNK_DAYS: i64 = 30;

/// A number, a numeric string or anything else (0), like the web's `_f`.
pub(crate) fn num(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// The web's `_i`: `int(float(x))`.
fn int(v: Option<&Value>) -> i64 {
    num(v) as i64
}

/// One depth side -> five levels (an empty level arrives as `{}`).
fn levels(side: Option<&Value>) -> Vec<DepthLevel> {
    let rows = side.and_then(Value::as_array);
    (0..5)
        .map(|i| {
            let l = rows.and_then(|r| r.get(i));
            DepthLevel {
                price: num(l.and_then(|l| l.get("price"))),
                quantity: int(l.and_then(|l| l.get("quantity"))),
                orders: int(l.and_then(|l| l.get("orders"))),
            }
        })
        .collect()
}

fn depth_side(q: &Value, side: &str) -> Vec<DepthLevel> {
    levels(q.get("depth").and_then(|d| d.get(side)))
}

/// One Vortex quote -> OpenAlgo quote, with the top of book quantities the
/// option chain reads (web `_format_quote`).
pub(crate) fn format_quote(key: &QuoteKey, q: &Value) -> Quote {
    let (bids, asks) = (depth_side(q, "buy"), depth_side(q, "sell"));
    let mut out = Quote {
        symbol: key.symbol.clone(),
        exchange: key.exchange.clone(),
        ltp: num(q.get("last_trade_price")),
        open: num(q.get("open_price")),
        high: num(q.get("high_price")),
        low: num(q.get("low_price")),
        close: num(q.get("close_price")),
        volume: int(q.get("volume")),
        bid: bids[0].price,
        ask: asks[0].price,
        bid_qty: bids[0].quantity,
        ask_qty: asks[0].quantity,
        oi: int(q.get("open_interest")),
        ..Default::default()
    };
    if out.close > 0.0 {
        out.change = round2(out.ltp - out.close);
        out.change_percent = round2((out.ltp - out.close) / out.close * 100.0);
    }
    out
}

/// One Vortex quote -> five-level depth (web `get_depth`).
pub(crate) fn to_depth(key: &QuoteKey, q: &Value) -> MarketDepth {
    MarketDepth {
        symbol: key.symbol.clone(),
        exchange: key.exchange.clone(),
        bids: depth_side(q, "buy"),
        asks: depth_side(q, "sell"),
        ltp: num(q.get("last_trade_price")),
        ltq: int(q.get("last_trade_quantity")),
        open: num(q.get("open_price")),
        high: num(q.get("high_price")),
        low: num(q.get("low_price")),
        prev_close: num(q.get("close_price")),
        volume: int(q.get("volume")),
        oi: int(q.get("open_interest")),
        total_buy_qty: int(q.get("total_buy_quantity")),
        total_sell_qty: int(q.get("total_sell_quantity")),
    }
}

/// `GET /data/quotes` for up to 1000 tickers -> `{ticker: quote}`.
async fn fetch_quotes(
    b: &RupeezyBroker,
    auth: &AuthToken,
    tickers: &[&str],
) -> Result<Map<String, Value>> {
    let mut q: Vec<(&str, String)> = tickers.iter().map(|t| ("q", t.to_string())).collect();
    q.push(("mode", "full".into()));
    let v = b.ok(auth, Call::get_with("/data/quotes", &q)).await?;
    Ok(v.get("data")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default())
}

/// The full quote of one instrument.
async fn single_quote(b: &RupeezyBroker, auth: &AuthToken, key: &QuoteKey) -> Result<Value> {
    let ticker = row_ticker(&b.instrument(key)?);
    fetch_quotes(b, auth, &[ticker.as_str()])
        .await?
        .remove(&ticker)
        .ok_or_else(|| {
            AppError::Broker(format!(
                "Rupeezy has no quote for {}:{} right now. Try again shortly.",
                key.exchange, key.symbol
            ))
        })
}

pub async fn get_quote(b: &RupeezyBroker, auth: &AuthToken, key: &QuoteKey) -> Result<Quote> {
    Ok(format_quote(key, &single_quote(b, auth, key).await?))
}

pub async fn get_market_depth(
    b: &RupeezyBroker,
    auth: &AuthToken,
    key: &QuoteKey,
) -> Result<MarketDepth> {
    Ok(to_depth(key, &single_quote(b, auth, key).await?))
}

/// Quotes for many instruments in batches of 1000, one entry per key in
/// request order: a key that does not resolve or that Vortex left out
/// carries an error. A failed batch is reported on its keys, except a
/// session that ended, which fails the call.
pub async fn get_multiquotes(
    b: &RupeezyBroker,
    auth: &AuthToken,
    keys: &[QuoteKey],
) -> Result<Vec<QuoteResult>> {
    let resolved: Vec<std::result::Result<String, String>> = keys
        .iter()
        .map(|k| {
            b.instrument(k)
                .map(|row| row_ticker(&row))
                .map_err(|e| e.client_message())
        })
        .collect();
    let unique: Vec<&str> = resolved
        .iter()
        .filter_map(|r| r.as_deref().ok())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut quotes: HashMap<String, Value> = HashMap::new();
    for batch in unique.chunks(QUOTE_BATCH) {
        match fetch_quotes(b, auth, batch).await {
            Ok(m) => quotes.extend(m),
            Err(e @ AppError::Auth(_)) => return Err(e),
            Err(e) => tracing::warn!(
                "Rupeezy quotes for {} instruments failed: {}",
                batch.len(),
                e.code()
            ),
        }
    }
    Ok(keys
        .iter()
        .zip(resolved)
        .map(|(k, r)| {
            let quote = r.and_then(|t| {
                quotes
                    .get(&t)
                    .map(|q| format_quote(k, q))
                    .ok_or_else(|| "No quote data available".to_string())
            });
            QuoteResult {
                symbol: k.symbol.clone(),
                exchange: k.exchange.clone(),
                error: quote.as_ref().err().cloned(),
                data: quote.ok(),
            }
        })
        .collect())
}

/// Epoch seconds of `day` at `h:m:s` in IST.
fn ist_epoch(day: NaiveDate, h: u32, m: u32, s: u32) -> i64 {
    day.and_hms_opt(h, m, s)
        .map(|t| t.and_utc().timestamp() - IST_OFFSET_SECONDS)
        .unwrap_or_default()
}

/// Daily candles land on 00:00 UTC of their IST trading date, whether
/// Vortex stamps them at IST or UTC midnight (the Zerodha convention).
pub(crate) fn to_ist_date(ts: i64) -> i64 {
    (ts + IST_OFFSET_SECONDS).div_euclid(DAY_SECONDS) * DAY_SECONDS
}

/// One `s: ok` answer -> candles (no open interest on this API).
pub(crate) fn parse_candles(v: &Value, daily: bool) -> Vec<Candle> {
    let col = |k: &str| v.get(k).and_then(Value::as_array);
    let Some(t) = col("t") else {
        return Vec::new();
    };
    let at = |k: &str, i: usize| num(col(k).and_then(|c| c.get(i)));
    t.iter()
        .enumerate()
        .map(|(i, ts)| {
            let ts = int(Some(ts));
            Candle {
                timestamp: if daily { to_ist_date(ts) } else { ts },
                open: at("o", i),
                high: at("h", i),
                low: at("l", i),
                close: at("c", i),
                volume: at("v", i) as i64,
                oi: 0,
            }
        })
        .collect()
}

pub async fn get_history(
    b: &RupeezyBroker,
    auth: &AuthToken,
    req: &HistoryRequest,
) -> Result<Vec<Candle>> {
    let resolution = TIMEFRAME_MAP
        .iter()
        .find(|(k, _)| *k == req.interval)
        .map(|(_, v)| *v)
        .ok_or_else(|| {
            AppError::Validation(format!(
                "Rupeezy does not offer the {} interval.",
                req.interval
            ))
        })?;
    let ticker = row_ticker(&b.instrument(&req.key)?);
    let daily = matches!(req.interval.as_str(), "D" | "W" | "M");
    let days = if daily {
        DAILY_CHUNK_DAYS
    } else {
        INTRADAY_CHUNK_DAYS
    };
    let mut out = Vec::new();
    for (from, to) in chunks(req.start, req.end, days) {
        let q = [
            ("ticker", ticker.clone()),
            ("from", ist_epoch(from, 0, 0, 0).to_string()),
            ("to", ist_epoch(to, 23, 59, 59).to_string()),
            ("resolution", resolution.to_string()),
        ];
        let a = b.call(auth, Call::get_with("/data/history", &q)).await?;
        match a.body.get("s").and_then(Value::as_str) {
            Some("ok") => out.extend(parse_candles(&a.body, daily)),
            Some("no_data") => {}
            _ => {
                let msg = message_of(&a.body);
                tracing::warn!(
                    status = a.status.as_u16(),
                    "Rupeezy history refused: {}",
                    msg
                );
                return Err(AppError::Broker(if msg.is_empty() {
                    "Rupeezy did not return candles for this request. Try again shortly.".into()
                } else {
                    msg
                }));
            }
        }
    }
    Ok(sort_dedupe(out))
}
