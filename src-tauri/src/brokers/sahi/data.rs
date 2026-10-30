//! Market data (web `broker/sahi/api/data.py`).
//!
//! Sahi's quote carries no open, high, low or previous close, so a single
//! quote and depth read them from the daily candles (today's candle is
//! included while the market is open). Batch quotes for options come from
//! the option chain endpoint, which does carry the previous close, and the
//! rest from the quotes endpoint, 100 ids a request; open, high and low are
//! not available in a batch.

use super::mapping::{from_paise, int, round2};
use super::ratelimit::Op;
use super::SahiBroker;
use crate::brokers::common::master_contract::parse_oa_expiry;
use crate::brokers::common::symbols::SymToken;
use crate::brokers::types::*;
use crate::error::{AppError, Result};
use chrono::{NaiveDate, NaiveTime, TimeZone};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

pub const QUOTE_BATCH: usize = 100;

/// Bound on backward pages of one history request: 2500 candles a page
/// covers about 27 sessions of 1-minute data, so 200 pages is over 20 years
/// of days (web `MAX_HISTORY_PAGES`).
pub const MAX_HISTORY_PAGES: usize = 200;

/// Time allowed for one history or batch request of several calls.
const LONG_DEADLINE: Duration = Duration::from_secs(120);

fn ist() -> chrono::FixedOffset {
    crate::clock::ist_offset()
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn ist_date(ms: i64) -> Option<NaiveDate> {
    chrono::DateTime::from_timestamp_millis(ms).map(|t| t.with_timezone(&ist()).date_naive())
}

fn ist_today() -> NaiveDate {
    chrono::Utc::now().with_timezone(&ist()).date_naive()
}

fn instrument(b: &SahiBroker, key: &QuoteKey) -> Result<SymToken> {
    b.resolver()
        .by_symbol(&key.exchange, &key.symbol)
        .filter(|r| !r.token.is_empty())
        .ok_or_else(|| {
            AppError::Validation(format!(
                "{} was not found on {} in the Sahi master contract.",
                key.symbol, key.exchange
            ))
        })
}

/// Raw quotes by instrument id; ids Sahi does not know are left out.
async fn quotes(
    b: &SahiBroker,
    auth: &AuthToken,
    ids: &[String],
    op: &Op,
) -> Result<HashMap<String, Value>> {
    let mut out = HashMap::new();
    for batch in ids.chunks(QUOTE_BATCH) {
        let query: Vec<(&str, String)> = batch.iter().map(|i| ("i", i.clone())).collect();
        let v = b.get(auth, "/v1/marketdata/quotes", &query, op).await?;
        if let Some(data) = v.get("data").and_then(Value::as_object) {
            for (id, q) in data.iter().filter(|(_, q)| q.is_object()) {
                out.insert(id.clone(), q.clone());
            }
        }
    }
    Ok(out)
}

/// The day's (open, high, low, previous close) in rupees, from up to 15
/// days of daily candles. When the latest candle is not today's (before
/// the open, a holiday) the day's open, high and low are unknown (0) and
/// the latest close is the previous close. Unreadable candles give zeros.
pub fn daily_ohlc(candles: &[Value], today: NaiveDate) -> (f64, f64, f64, f64) {
    let at = |c: &Value, i: usize| from_paise(c.get(i));
    let Some(last) = candles.last() else {
        return (0.0, 0.0, 0.0, 0.0);
    };
    if ist_date(int(last.get(0))) == Some(today) {
        let prev = candles
            .len()
            .checked_sub(2)
            .and_then(|i| candles.get(i))
            .map(|c| at(c, 4))
            .unwrap_or(0.0);
        (at(last, 1), at(last, 2), at(last, 3), prev)
    } else {
        (0.0, 0.0, 0.0, at(last, 4))
    }
}

async fn daily_candles(b: &SahiBroker, auth: &AuthToken, id: &str) -> (f64, f64, f64, f64) {
    let now = now_ms();
    let query = [
        ("instrument_id", id.to_string()),
        ("size", "1d".to_string()),
        ("from", (now - 15 * 86_400_000).to_string()),
        ("to", now.to_string()),
        ("include_partial", "true".to_string()),
    ];
    match b
        .get(auth, "/v1/marketdata/candles", &query, &Op::single())
        .await
    {
        Ok(v) => daily_ohlc(
            v.get("candles")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
            ist_today(),
        ),
        Err(e) => {
            tracing::warn!("Sahi daily candles unavailable: {}", e.code());
            (0.0, 0.0, 0.0, 0.0)
        }
    }
}

fn levels(q: &Value, side: &str) -> Vec<Value> {
    q.get(side)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// A quote from one quotes entry and the day's OHLC (`close` is the
/// previous close).
pub fn to_quote(key: &QuoteKey, q: &Value, ohlc: (f64, f64, f64, f64)) -> Quote {
    let (buys, sells) = (levels(q, "best_buys"), levels(q, "best_sells"));
    let (open, high, low, close) = ohlc;
    let mut quote = Quote {
        symbol: key.symbol.clone(),
        exchange: key.exchange.clone(),
        ltp: from_paise(q.get("last_trade_price")),
        open,
        high,
        low,
        close,
        volume: int(q.get("volume_today")),
        bid: buys
            .first()
            .map(|l| from_paise(l.get("price")))
            .unwrap_or(0.0),
        ask: sells
            .first()
            .map(|l| from_paise(l.get("price")))
            .unwrap_or(0.0),
        bid_qty: buys.first().map(|l| int(l.get("qty"))).unwrap_or(0),
        ask_qty: sells.first().map(|l| int(l.get("qty"))).unwrap_or(0),
        oi: int(q.get("open_interest")),
        ..Default::default()
    };
    derive_change(&mut quote);
    quote
}

fn derive_change(q: &mut Quote) {
    if q.close > 0.0 {
        q.change = round2(q.ltp - q.close);
        q.change_percent = round2((q.ltp - q.close) / q.close * 100.0);
    }
}

/// Five levels a side, padded with zeros; totals are the sums of every
/// level Sahi sent (web).
pub fn to_depth(key: &QuoteKey, q: &Value, ohlc: (f64, f64, f64, f64)) -> MarketDepth {
    let (buys, sells) = (levels(q, "best_buys"), levels(q, "best_sells"));
    let pad = |side: &[Value]| -> Vec<DepthLevel> {
        (0..5)
            .map(|i| {
                side.get(i)
                    .map(|l| DepthLevel {
                        price: from_paise(l.get("price")),
                        quantity: int(l.get("qty")),
                        orders: int(l.get("num_orders")),
                    })
                    .unwrap_or_default()
            })
            .collect()
    };
    let total = |side: &[Value]| side.iter().map(|l| int(l.get("qty"))).sum();
    let (open, high, low, prev_close) = ohlc;
    MarketDepth {
        symbol: key.symbol.clone(),
        exchange: key.exchange.clone(),
        bids: pad(&buys),
        asks: pad(&sells),
        ltp: from_paise(q.get("last_trade_price")),
        ltq: 0,
        open,
        high,
        low,
        prev_close,
        volume: int(q.get("volume_today")),
        oi: int(q.get("open_interest")),
        total_buy_qty: total(&buys),
        total_sell_qty: total(&sells),
    }
}

async fn one_quote(b: &SahiBroker, auth: &AuthToken, key: &QuoteKey) -> Result<(Value, String)> {
    let row = instrument(b, key)?;
    let mut got = quotes(b, auth, std::slice::from_ref(&row.token), &Op::single()).await?;
    match got.remove(&row.token) {
        Some(q) => Ok((q, row.token)),
        None => Err(AppError::Broker(format!(
            "Sahi returned no quote for {} on {}.",
            key.symbol, key.exchange
        ))),
    }
}

pub async fn get_quote(b: &SahiBroker, auth: &AuthToken, key: &QuoteKey) -> Result<Quote> {
    let (q, id) = one_quote(b, auth, key).await?;
    Ok(to_quote(key, &q, daily_candles(b, auth, &id).await))
}

pub async fn get_market_depth(
    b: &SahiBroker,
    auth: &AuthToken,
    key: &QuoteKey,
) -> Result<MarketDepth> {
    let (q, id) = one_quote(b, auth, key).await?;
    Ok(to_depth(key, &q, daily_candles(b, auth, &id).await))
}

/// A quote from one option-chain leg (previous close, no day OHLC).
pub fn chain_leg_quote(key: &QuoteKey, leg: &Value) -> Quote {
    let mut q = Quote {
        symbol: key.symbol.clone(),
        exchange: key.exchange.clone(),
        ltp: from_paise(leg.get("last_trade_price")),
        close: from_paise(leg.get("prev_close_price")),
        volume: int(leg.get("volume_today")),
        bid: from_paise(leg.get("top_bid_price")),
        ask: from_paise(leg.get("top_ask_price")),
        oi: int(leg.get("open_interest")),
        ..Default::default()
    };
    derive_change(&mut q);
    q
}

/// The underlying's instrument id an option chain is asked for: the index
/// of that name, else the stock.
fn underlying_id(b: &SahiBroker, row: &SymToken) -> Option<String> {
    let (index, cash) = match row.exchange.as_str() {
        "NFO" => ("NSE_INDEX", "NSE"),
        "BFO" => ("BSE_INDEX", "BSE"),
        _ => return None,
    };
    let name = crate::brokers::common::underlying::row_underlying(row)?;
    b.resolver()
        .by_symbol(index, &name)
        .or_else(|| b.resolver().by_symbol(cash, &name))
        .map(|r| r.token)
}

/// Option quotes from the chain endpoint, one call per underlying and
/// expiry; chains that cannot be read are left to the quotes endpoint.
async fn chain_quotes(
    b: &SahiBroker,
    auth: &AuthToken,
    options: &[(QuoteKey, SymToken)],
    op: &Op,
) -> HashMap<String, Quote> {
    let mut groups: BTreeMap<(String, String), Vec<&(QuoteKey, SymToken)>> = BTreeMap::new();
    for item in options {
        let expiry = parse_oa_expiry(&item.1.expiry);
        if let (Some(u), Some(e)) = (underlying_id(b, &item.1), expiry) {
            groups
                .entry((u, e.format("%Y-%m-%d").to_string()))
                .or_default()
                .push(item);
        }
    }
    let mut out = HashMap::new();
    for ((underlying, expiry), wanted) in groups {
        let query = [("underlying_id", underlying), ("expiry", expiry)];
        let chain = match b.get(auth, "/v1/marketdata/optionchain", &query, op).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("Sahi option chain unavailable, using quotes: {}", e.code());
                continue;
            }
        };
        let legs: HashMap<String, &Value> = chain
            .get("strikes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .flat_map(|s| [s.get("ce"), s.get("pe")])
            .flatten()
            .filter(|l| l.is_object())
            .map(|l| (int(l.get("instrument_id")).to_string(), l))
            .collect();
        for (key, row) in wanted {
            if let Some(leg) = legs.get(&row.token) {
                out.insert(row.token.clone(), chain_leg_quote(key, leg));
            }
        }
    }
    out
}

fn entry(key: &QuoteKey, data: Option<Quote>, error: Option<&str>) -> QuoteResult {
    QuoteResult {
        symbol: key.symbol.clone(),
        exchange: key.exchange.clone(),
        data,
        error: error.map(str::to_string),
    }
}

pub async fn get_multiquotes(
    b: &SahiBroker,
    auth: &AuthToken,
    keys: &[QuoteKey],
) -> Result<Vec<QuoteResult>> {
    let op = Op::within(LONG_DEADLINE);
    let rows: Vec<Option<SymToken>> = keys
        .iter()
        .map(|k| {
            b.resolver()
                .by_symbol(&k.exchange, &k.symbol)
                .filter(|r| !r.token.is_empty())
        })
        .collect();
    let options: Vec<(QuoteKey, SymToken)> = keys
        .iter()
        .zip(&rows)
        .filter_map(|(k, r)| r.clone().map(|r| (k.clone(), r)))
        .filter(|(_, r)| matches!(r.instrument_type.as_str(), "CE" | "PE"))
        .collect();
    let mut by_id = chain_quotes(b, auth, &options, &op).await;
    let mut rest: Vec<String> = rows
        .iter()
        .flatten()
        .map(|r| r.token.clone())
        .filter(|id| !by_id.contains_key(id))
        .collect();
    rest.sort();
    rest.dedup();
    let raw = quotes(b, auth, &rest, &op).await?;
    let mut out = Vec::with_capacity(keys.len());
    for (key, row) in keys.iter().zip(&rows) {
        let Some(row) = row else {
            out.push(entry(key, None, Some("Symbol not found")));
            continue;
        };
        let quote = by_id.remove(&row.token).or_else(|| {
            raw.get(&row.token)
                .map(|q| to_quote(key, q, (0.0, 0.0, 0.0, 0.0)))
        });
        match quote {
            Some(q) => {
                // A key asked twice is answered twice.
                by_id.insert(row.token.clone(), q.clone());
                out.push(entry(key, Some(q), None));
            }
            None => out.push(entry(key, None, Some("No quote returned by Sahi"))),
        }
    }
    Ok(out)
}

/// The request window in epoch ms: 00:00 IST of `start` to 23:59:59 IST of
/// `end`, never past now.
pub fn window_ms(start: NaiveDate, end: NaiveDate, now: i64) -> (i64, i64) {
    let at = |d: NaiveDate, t: NaiveTime| {
        ist()
            .from_local_datetime(&d.and_time(t))
            .single()
            .map(|x| x.timestamp_millis())
            .unwrap_or(0)
    };
    let from = at(start, NaiveTime::MIN);
    let to = NaiveTime::from_hms_opt(23, 59, 59).map_or(0, |t| at(end, t));
    (from, to.min(now))
}

/// One Sahi candle to an OpenAlgo candle. Intraday stamps are the candle's
/// epoch seconds; daily candles are stamped at 00:00 UTC of their IST date.
pub fn to_candle(c: &Value, daily: bool) -> Option<Candle> {
    let ms = int(c.get(0));
    let timestamp = if daily {
        ist_date(ms)?.and_time(NaiveTime::MIN).and_utc().timestamp()
    } else {
        ms / 1000
    };
    Some(Candle {
        timestamp,
        open: from_paise(c.get(1)),
        high: from_paise(c.get(2)),
        low: from_paise(c.get(3)),
        close: from_paise(c.get(4)),
        volume: int(c.get(5)),
        oi: int(c.get(6)),
    })
}

pub async fn get_history(
    b: &SahiBroker,
    auth: &AuthToken,
    req: &HistoryRequest,
) -> Result<Vec<Candle>> {
    let size = super::TIMEFRAME_MAP
        .iter()
        .find(|(k, _)| *k == req.interval)
        .map(|(_, v)| *v)
        .ok_or_else(|| {
            AppError::Validation(format!(
                "Interval {} is not supported by Sahi. Use one of: 1m, 3m, 5m, 15m, 30m, 1h, D.",
                req.interval
            ))
        })?;
    let row = instrument(b, &req.key)?;
    let want_oi = matches!(row.instrument_type.as_str(), "FUT" | "CE" | "PE");
    let (from, to) = window_ms(req.start, req.end, now_ms());
    let op = Op::within(LONG_DEADLINE);
    let mut pages: Vec<Vec<Value>> = Vec::new();
    // Only the first (latest) page can hold the in-progress candle.
    let mut include_partial = req.end >= ist_today();
    let mut cursor = to;
    while cursor >= from {
        if pages.len() >= MAX_HISTORY_PAGES {
            return Err(AppError::Validation(
                "The requested history is too long to fetch in one request. Ask for a shorter range.".into(),
            ));
        }
        let mut query = vec![
            ("instrument_id", row.token.clone()),
            ("size", size.to_string()),
            ("from", from.to_string()),
            ("to", cursor.to_string()),
        ];
        if std::mem::take(&mut include_partial) {
            query.push(("include_partial", "true".to_string()));
        }
        if want_oi {
            query.push(("oi", "true".to_string()));
        }
        let v = b.get(auth, "/v1/marketdata/candles", &query, &op).await?;
        let candles = v
            .get("candles")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let more = v.get("has_more").and_then(Value::as_bool).unwrap_or(false);
        let first = candles.first().map(|c| int(c.get(0)));
        pages.push(candles);
        match first {
            Some(ts) if more => cursor = ts - 1,
            _ => break,
        }
    }
    let daily = size == "1d";
    let candles: Vec<Candle> = pages
        .iter()
        .flatten()
        .filter_map(|c| to_candle(c, daily))
        .collect();
    // The page fetched first (the latest) wins a shared timestamp, as the
    // web's oldest-first list with `keep="last"`.
    let mut by_ts: BTreeMap<i64, Candle> = BTreeMap::new();
    for c in candles.into_iter().rev() {
        by_ts.insert(c.timestamp, c);
    }
    Ok(by_ts.into_values().collect())
}
