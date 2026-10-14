//! Option-chain quotes from Fyers' native `/data/options-chain-v3`
//! (TOOLS-04, TOOLS-05, BRL-02).
//!
//! Fyers' batch `/data/quotes` carries no open interest, so the generic
//! path fetched it with one `/data/depth` call per option (up to 100), and
//! not at all above 100 options, where every leg read OI 0. The native
//! chain answers every strike's LTP, bid/ask, volume and OI in one
//! request.
//!
//! The web's fast path (`services/option_chain_service.py`) sends no
//! expiry and maps the answer by strike and type, so a later expiry got
//! the nearest expiry's prices. Here the requested expiry is sent (its
//! `timestamp` from the answer's `expiryData`, remembered per underlying)
//! and every returned contract is looked up by its Fyers symbol in the
//! master: its expiry must be the one requested, or the answer is not
//! used. Legs the native answer does not cover, and every leg when it
//! cannot be used, are quoted through `/data/quotes` and reported without
//! open interest, never with a zero that looks like a reading.

use super::data::multiquotes_with;
use super::FyersBroker;
use crate::brokers::common::de::{f64_lenient, i64_lenient};
use crate::brokers::common::master_contract::parse_oa_expiry;
use crate::brokers::common::streaming::round2;
use crate::brokers::types::*;
use crate::error::{AppError, Result};
use chrono::NaiveDate;
use reqwest::Method;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Fyers' cap on `strikecount`.
pub const MAX_STRIKECOUNT: usize = 50;
/// Strikes asked for beyond the chain's own window on each side: Fyers
/// places ATM itself and may be a strike off ours (the web's +2).
const SPARE_STRIKES: usize = 2;
/// Underlying-and-expiry entries remembered (`timestamp` per expiry).
const EXPIRY_CACHE_CAP: usize = 256;

/// One entry of `data.optionsChain`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct NativeLeg {
    pub symbol: String,
    pub option_type: String,
    #[serde(deserialize_with = "f64_lenient")]
    pub strike_price: f64,
    #[serde(deserialize_with = "f64_lenient")]
    pub ltp: f64,
    #[serde(deserialize_with = "f64_lenient")]
    pub ltpch: f64,
    #[serde(deserialize_with = "f64_lenient")]
    pub ltpchp: f64,
    #[serde(deserialize_with = "f64_lenient")]
    pub bid: f64,
    #[serde(deserialize_with = "f64_lenient")]
    pub ask: f64,
    #[serde(deserialize_with = "i64_lenient")]
    pub volume: i64,
    #[serde(deserialize_with = "i64_lenient")]
    pub oi: i64,
}

/// One entry of `data.expiryData`: `date` as `DD-MM-YYYY`, `expiry` the
/// value Fyers takes back as `timestamp`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct NativeExpiry {
    pub date: String,
    pub expiry: String,
}

/// A parsed native chain answer.
#[derive(Debug, Clone, Default)]
pub struct NativeChain {
    pub legs: Vec<NativeLeg>,
    pub expiries: Vec<(NaiveDate, String)>,
}

pub fn parse_native(v: &Value) -> NativeChain {
    let data = v.get("data");
    let legs = data
        .and_then(|d| d.get("optionsChain"))
        .and_then(|x| serde_json::from_value::<Vec<NativeLeg>>(x.clone()).ok())
        .unwrap_or_default()
        .into_iter()
        // The underlying itself rides along (no option type, strike -1).
        .filter(|l| matches!(l.option_type.as_str(), "CE" | "PE") && !l.symbol.is_empty())
        .collect();
    let expiries = data
        .and_then(|d| d.get("expiryData"))
        .and_then(|x| serde_json::from_value::<Vec<NativeExpiry>>(x.clone()).ok())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| {
            let d = NaiveDate::parse_from_str(e.date.trim(), "%d-%m-%Y").ok()?;
            (!e.expiry.trim().is_empty()).then(|| (d, e.expiry.trim().to_string()))
        })
        .collect();
    NativeChain { legs, expiries }
}

/// `timestamp` values per (Fyers underlying, expiry), bounded.
#[derive(Debug, Default)]
pub struct ExpiryTimestamps {
    map: parking_lot::Mutex<HashMap<(String, NaiveDate), String>>,
}

impl ExpiryTimestamps {
    fn get(&self, underlying: &str, expiry: NaiveDate) -> Option<String> {
        self.map
            .lock()
            .get(&(underlying.to_string(), expiry))
            .cloned()
    }

    fn remember(&self, underlying: &str, expiries: &[(NaiveDate, String)]) {
        let mut m = self.map.lock();
        if m.len() + expiries.len() > EXPIRY_CACHE_CAP {
            m.clear();
        }
        for (d, ts) in expiries {
            m.insert((underlying.to_string(), *d), ts.clone());
        }
    }

    fn forget(&self, underlying: &str, expiry: NaiveDate) {
        self.map.lock().remove(&(underlying.to_string(), expiry));
    }

    #[cfg(test)]
    pub fn entries(&self) -> usize {
        self.map.lock().len()
    }
}

async fn fetch(
    b: &FyersBroker,
    auth: &AuthToken,
    underlying: &str,
    strikecount: usize,
    timestamp: Option<&str>,
) -> Result<NativeChain> {
    let mut path = format!(
        "/data/options-chain-v3?symbol={}&strikecount={}",
        urlencoding::encode(underlying),
        strikecount
    );
    if let Some(ts) = timestamp {
        path.push_str("&timestamp=");
        path.push_str(&urlencoding::encode(ts));
    }
    let v = b.call(Method::GET, &path, auth, None).await?;
    Ok(parse_native(&v))
}

/// The native legs keyed by Fyers symbol when every leg the master knows
/// belongs to `expiry` (and at least one does); `None` when the answer is
/// another expiry's.
fn verified(
    b: &FyersBroker,
    chain: &NativeChain,
    options_exchange: &str,
    expiry: NaiveDate,
) -> Option<HashMap<String, NativeLeg>> {
    let mut out = HashMap::new();
    for l in &chain.legs {
        let Some(row) = b.resolver().by_brsymbol(options_exchange, &l.symbol) else {
            continue;
        };
        if parse_oa_expiry(&row.expiry) != Some(expiry) {
            return None;
        }
        out.insert(l.symbol.clone(), l.clone());
    }
    (!out.is_empty()).then_some(out)
}

/// The native chain for `req`, checked against its expiry.
async fn native(
    b: &FyersBroker,
    auth: &AuthToken,
    req: &ChainQuoteRequest,
) -> Result<HashMap<String, NativeLeg>> {
    let each_side = req
        .strikes_each_side
        .filter(|n| n + SPARE_STRIKES <= MAX_STRIKECOUNT)
        .ok_or(AppError::Unsupported("whole_chain"))?;
    let options_exchange = req
        .keys
        .first()
        .map(|k| k.exchange.clone())
        .ok_or(AppError::Unsupported("empty_chain"))?;
    let underlying = b.lookup(&req.underlying)?.br_symbol().to_string();
    let count = each_side + SPARE_STRIKES;
    let known = b.expiry_timestamps().get(&underlying, req.expiry);
    let first = fetch(b, auth, &underlying, count, known.as_deref()).await?;
    b.expiry_timestamps().remember(&underlying, &first.expiries);
    if let Some(legs) = verified(b, &first, &options_exchange, req.expiry) {
        return Ok(legs);
    }
    if known.is_some() {
        // A remembered timestamp that now answers another expiry is stale.
        b.expiry_timestamps().forget(&underlying, req.expiry);
    }
    let ts = first
        .expiries
        .iter()
        .find(|(d, _)| *d == req.expiry)
        .map(|(_, ts)| ts.clone())
        .filter(|ts| known.as_deref() != Some(ts.as_str()))
        .ok_or_else(|| {
            AppError::Broker(format!(
                "Fyers' option chain does not list the {} expiry",
                req.expiry
            ))
        })?;
    let second = fetch(b, auth, &underlying, count, Some(&ts)).await?;
    verified(b, &second, &options_exchange, req.expiry).ok_or_else(|| {
        AppError::Broker(format!(
            "Fyers' option chain answered another expiry than {}",
            req.expiry
        ))
    })
}

fn leg_quote(key: &QuoteKey, l: &NativeLeg) -> Quote {
    let close = if l.ltp > 0.0 {
        round2(l.ltp - l.ltpch)
    } else {
        0.0
    };
    Quote {
        symbol: key.symbol.clone(),
        exchange: key.exchange.clone(),
        ltp: l.ltp,
        bid: l.bid,
        ask: l.ask,
        volume: l.volume,
        oi: l.oi,
        close,
        change: l.ltpch,
        change_percent: l.ltpchp,
        ..Default::default()
    }
}

/// Quotes for a chain's legs: the native chain where it can be used,
/// `/data/quotes` without open interest for the rest.
pub async fn get_chain_quotes(
    b: &FyersBroker,
    auth: &AuthToken,
    req: &ChainQuoteRequest,
) -> Result<ChainQuotes> {
    let legs = match native(b, auth, req).await {
        Ok(legs) => legs,
        Err(e @ AppError::Auth(_)) => return Err(e),
        Err(e) => {
            tracing::warn!(
                "Fyers native option chain not used for {} {}: {}",
                req.underlying.symbol,
                req.expiry,
                e
            );
            HashMap::new()
        }
    };
    let mut quotes: Vec<Option<QuoteResult>> = Vec::with_capacity(req.keys.len());
    let mut rest: Vec<QuoteKey> = Vec::new();
    for k in &req.keys {
        let hit = b
            .resolver()
            .by_symbol(&k.exchange, &k.symbol)
            .and_then(|row| legs.get(row.br_symbol()).cloned());
        match hit {
            Some(l) => quotes.push(Some(QuoteResult {
                symbol: k.symbol.clone(),
                exchange: k.exchange.clone(),
                data: Some(leg_quote(k, &l)),
                error: None,
            })),
            None => {
                rest.push(k.clone());
                quotes.push(None);
            }
        }
    }
    let mut oi_missing = HashSet::new();
    if !rest.is_empty() {
        let fetched = multiquotes_with(b, auth, &rest, false).await?;
        let mut by_key: HashMap<(String, String), QuoteResult> = fetched
            .into_iter()
            .map(|r| ((r.exchange.clone(), r.symbol.clone()), r))
            .collect();
        for (slot, k) in quotes.iter_mut().zip(req.keys.iter()) {
            if slot.is_some() {
                continue;
            }
            oi_missing.insert(k.symbol.clone());
            *slot = by_key.remove(&(k.exchange.clone(), k.symbol.clone()));
        }
    }
    Ok(ChainQuotes {
        quotes: quotes
            .into_iter()
            .zip(req.keys.iter())
            .map(|(q, k)| {
                q.unwrap_or_else(|| QuoteResult {
                    symbol: k.symbol.clone(),
                    exchange: k.exchange.clone(),
                    data: None,
                    error: Some("No quote data available".into()),
                })
            })
            .collect(),
        oi_missing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn native_answer_parses_legs_and_expiries() {
        let v = json!({
            "s": "ok", "code": 200,
            "data": {
                "expiryData": [
                    {"date": "13-10-2026", "expiry": "1760349600"},
                    {"date": "20-10-2026", "expiry": "1760954400"},
                    {"date": "bad", "expiry": "1"},
                ],
                "optionsChain": [
                    {"symbol": "NSE:NIFTY50-INDEX", "option_type": "", "strike_price": -1, "ltp": 25010},
                    {"symbol": "NSE:NIFTY26O1325000CE", "option_type": "CE", "strike_price": 25000,
                     "ltp": 120.5, "ltpch": -4.5, "ltpchp": -3.6, "bid": 120, "ask": 121, "volume": 900, "oi": 15000},
                ]
            }
        });
        let c = parse_native(&v);
        assert_eq!(c.legs.len(), 1);
        assert_eq!(c.legs[0].oi, 15_000);
        assert_eq!(
            c.expiries,
            vec![
                (
                    NaiveDate::from_ymd_opt(2026, 10, 13).unwrap(),
                    "1760349600".into()
                ),
                (
                    NaiveDate::from_ymd_opt(2026, 10, 20).unwrap(),
                    "1760954400".into()
                ),
            ]
        );
        let q = leg_quote(&QuoteKey::new("NFO", "NIFTY13OCT2625000CE"), &c.legs[0]);
        assert_eq!(
            (q.ltp, q.close, q.oi, q.bid, q.ask),
            (120.5, 125.0, 15_000, 120.0, 121.0)
        );
    }

    #[test]
    fn the_timestamp_cache_is_bounded() {
        let t = ExpiryTimestamps::default();
        let d = |i: i64| NaiveDate::from_ymd_opt(2026, 1, 1).unwrap() + chrono::Duration::days(i);
        for u in 0..100 {
            let entries: Vec<(NaiveDate, String)> = (0..5).map(|i| (d(i), i.to_string())).collect();
            t.remember(&format!("U{}", u), &entries);
            assert!(t.entries() <= EXPIRY_CACHE_CAP);
        }
    }
}
