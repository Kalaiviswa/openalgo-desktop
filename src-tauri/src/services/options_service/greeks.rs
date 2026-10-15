//! Option greeks (web `option_greeks_service.py`): option symbol parsing,
//! the forward each greek is priced against, and Black-76 on it.

use super::{
    expiry_datetime, parse_compact_expiry, synthetic_future_price, time_to_expiry, BSE_INDICES,
};
use crate::analytics::black76::{self as bs, Flag};
use crate::brokers::types::QuoteKey;
use crate::services::core::{broker_handle, BrokerHandle, Reply};
use crate::services::market_data_service::fetch_quote;
use crate::state::AppState;
use chrono::{DateTime, NaiveTime, Utc};
use serde_json::{json, Value};
use std::collections::HashMap;

/// A parsed option symbol, as the web's
/// `^([A-Z0-9&-]+?)(\d{2})([A-Z]{3})(\d{2})([\d.]+)(CE|PE)\Z` (#2206).
#[derive(Debug, Clone, PartialEq)]
pub struct OptionContract {
    pub base: String,
    pub expiry: String,
    pub strike: f64,
    pub option_type: String,
}

/// `DDMMMYY` followed by a strike of digits and dots, nothing else.
fn is_expiry_and_strike(b: &[u8]) -> bool {
    b.len() > 7
        && b[0..2].iter().all(u8::is_ascii_digit)
        && b[2..5].iter().all(u8::is_ascii_uppercase)
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[7..].iter().all(|c| c.is_ascii_digit() || *c == b'.')
}

pub fn parse_option_symbol(symbol: &str) -> Result<OptionContract, String> {
    let up = symbol.to_ascii_uppercase();
    let fail = || {
        format!(
            "Failed to parse option symbol {}: Invalid option symbol format: {}",
            symbol, symbol
        )
    };
    // The type closes the symbol: trailing text, a newline included, is
    // refused rather than ignored.
    let option_type = if up.ends_with("CE") {
        "CE"
    } else if up.ends_with("PE") {
        "PE"
    } else {
        return Err(fail());
    };
    let body = &up.as_bytes()[..up.len() - 2];
    // The underlying may hold digits, `&` and `-` (M&M, BAJAJ-AUTO, 360ONE);
    // like the web's lazy group, the shortest one that leaves a valid expiry
    // and strike wins.
    let mut split = None;
    for i in 1..body.len() {
        if !matches!(body[i - 1], b'A'..=b'Z' | b'0'..=b'9' | b'&' | b'-') {
            break;
        }
        if is_expiry_and_strike(&body[i..]) {
            split = Some(i);
            break;
        }
    }
    let Some(split) = split else {
        return Err(fail());
    };
    // Every byte up to the strike's end is ASCII, so these are char boundaries.
    let base = up[..split].to_string();
    let expiry = up[split..split + 7].to_string();
    let strike: f64 = up[split + 7..up.len() - 2].parse().map_err(|_| fail())?;
    if parse_compact_expiry(&expiry).is_none() {
        return Err(format!(
            "Failed to parse option symbol {}: Invalid month in option symbol: {}",
            symbol, symbol
        ));
    }
    Ok(OptionContract {
        base,
        expiry,
        strike,
        option_type: option_type.to_string(),
    })
}

const COMMODITIES: &[&str] = &[
    "GOLD",
    "GOLDM",
    "GOLDPETAL",
    "SILVER",
    "SILVERM",
    "SILVERMIC",
    "CRUDEOIL",
    "CRUDEOILM",
    "NATURALGAS",
    "COPPER",
    "ZINC",
    "LEAD",
    "ALUMINIUM",
    "NICKEL",
    "COTTONCANDY",
    "MENTHAOIL",
];

/// The underlying's exchange when the request does not name it.
pub fn greeks_underlying_exchange(base: &str, exchange: &str) -> String {
    if [
        "NIFTY",
        "BANKNIFTY",
        "FINNIFTY",
        "MIDCPNIFTY",
        "NIFTYNXT50",
        "NIFTYIT",
        "NIFTYPHARMA",
        "NIFTYBANK",
    ]
    .contains(&base)
    {
        "NSE_INDEX".into()
    } else if BSE_INDICES.contains(&base) {
        "BSE_INDEX".into()
    } else if ["USDINR", "EURINR", "GBPINR", "JPYINR"].contains(&base) || exchange == "CDS" {
        "CDS".into()
    } else if COMMODITIES.contains(&base) || exchange == "MCX" {
        "MCX".into()
    } else if exchange == "CRYPTO" {
        "CRYPTO".into()
    } else {
        "NSE".into()
    }
}

/// `HH:MM` override of the expiry cut-off.
pub fn parse_expiry_time(v: &str) -> Result<NaiveTime, String> {
    let parts: Vec<&str> = v.split(':').collect();
    if parts.len() != 2 {
        return Err(format!(
            "Invalid expiry_time format: {}. Use HH:MM format (e.g., '15:30', '19:00')",
            v
        ));
    }
    let (Ok(h), Ok(m)) = (
        parts[0].trim().parse::<u32>(),
        parts[1].trim().parse::<u32>(),
    ) else {
        return Err(format!(
            "Failed to parse expiry_time '{}': invalid literal for int()",
            v
        ));
    };
    NaiveTime::from_hms_opt(h, m, 0).ok_or_else(|| {
        format!(
            "Invalid expiry_time values: {}. Hour must be 0-23, minute must be 0-59",
            v
        )
    })
}

fn expiry_display(c: &OptionContract) -> String {
    parse_compact_expiry(&c.expiry)
        .map(|d| d.format("%d-%b-%Y").to_string())
        .unwrap_or_default()
}

fn rate_json(rate: Option<f64>) -> Value {
    match rate {
        Some(r) => json!(bs::py_round(r, 2)),
        None => json!(0),
    }
}

/// Web `calculate_greeks`.
#[allow(clippy::too_many_arguments)]
pub fn calculate_greeks(
    symbol: &str,
    exchange: &str,
    c: &OptionContract,
    forward: f64,
    option_price: f64,
    rate: Option<f64>,
    expiry_time: Option<NaiveTime>,
    now: DateTime<Utc>,
) -> Reply {
    let Some(exp) = expiry_datetime(&c.expiry, exchange, expiry_time) else {
        return Reply::error(400, format!("Invalid expiry in {}", symbol));
    };
    let Some((days, t)) = time_to_expiry(exp, now) else {
        return Reply::error(400, format!("Option has expired on {}", expiry_display(c)));
    };
    if forward <= 0.0 || option_price <= 0.0 {
        return Reply::error(400, "Spot price and option price must be positive");
    }
    let k = c.strike;
    if k <= 0.0 {
        return Reply::error(400, "Strike price must be positive");
    }
    let call = c.option_type == "CE";
    let flag = if call { Flag::Call } else { Flag::Put };
    let r = rate.unwrap_or(0.0) / 100.0;
    let intrinsic = if call {
        (forward - k).max(0.0)
    } else {
        (k - forward).max(0.0)
    };
    let tv = option_price - intrinsic;
    let mut base = json!({
        "status": "success",
        "symbol": symbol,
        "exchange": exchange,
        "underlying": c.base,
        "strike": bs::py_round(k, 2),
        "option_type": c.option_type,
        "expiry_date": expiry_display(c),
        "days_to_expiry": bs::py_round(days, 4),
        "spot_price": bs::py_round(forward, 2),
        "option_price": bs::py_round(option_price, 2),
        "interest_rate": rate_json(rate),
    });
    let theoretical = |mut v: Value, note: &str| {
        if let Some(m) = v.as_object_mut() {
            m.insert("implied_volatility".into(), json!(0));
            m.insert("intrinsic_value".into(), json!(bs::py_round(intrinsic, 2)));
            m.insert("time_value".into(), json!(bs::py_round(tv.max(0.0), 2)));
            m.insert("note".into(), json!(note));
            m.insert(
                "greeks".into(),
                json!({"delta": if call { 1.0 } else { -1.0 }, "gamma": 0, "theta": 0, "vega": 0, "rho": 0}),
            );
        }
        Reply::ok(v)
    };
    if tv <= 0.0 || (intrinsic > 0.0 && tv < 0.01) {
        return theoretical(
            base,
            "Deep ITM option with no time value - theoretical Greeks returned",
        );
    }
    let iv = match bs::implied_volatility(option_price, forward, k, r, t, flag) {
        Ok(iv) => iv,
        Err(m) => {
            let l = m.to_ascii_lowercase();
            if l.contains("intrinsic") || l.contains("below") || l.contains("convergence") {
                return theoretical(
                    base,
                    "IV calculation not possible - theoretical deep ITM Greeks returned",
                );
            }
            return Reply::error(
                500,
                format!("Failed to calculate Implied Volatility: {}", m),
            );
        }
    };
    let g = bs::greeks(flag, forward, k, t, r, iv);
    if let Some(m) = base.as_object_mut() {
        m.insert(
            "implied_volatility".into(),
            json!(bs::py_round(iv * 100.0, 2)),
        );
        m.insert(
            "greeks".into(),
            json!({
                "delta": bs::py_round(g.delta, 4),
                "gamma": bs::py_round(g.gamma, 6),
                "theta": bs::py_round(g.theta, 4),
                "vega": bs::py_round(g.vega, 4),
                "rho": bs::py_round(g.rho, 6),
            }),
        );
    }
    Reply::ok(base)
}

/// The forward for a contract: the synthetic future when the underlying is
/// the plain index/stock, else the underlying quote.
async fn forward_for(
    ctx: &AppState,
    h: &BrokerHandle,
    c: &OptionContract,
    underlying_symbol: Option<&str>,
    underlying_exchange: Option<&str>,
    exchange: &str,
) -> Result<f64, Reply> {
    let spot_symbol = underlying_symbol.unwrap_or(&c.base).to_string();
    let spot_exchange = underlying_exchange
        .map(str::to_string)
        .unwrap_or_else(|| greeks_underlying_exchange(&c.base, exchange));
    if underlying_symbol.is_none_or(|u| u == c.base) {
        if let Ok((_, f, _)) =
            synthetic_future_price(ctx, h, &c.base, &spot_exchange, &c.expiry).await
        {
            return Ok(bs::py_round(f, 2));
        }
    }
    match fetch_quote(ctx, h, &spot_symbol, &spot_exchange).await {
        Ok(q) if q.ltp != 0.0 => Ok(q.ltp),
        Ok(_) => Err(Reply::error(404, "Underlying LTP not available")),
        Err(r) => Err(Reply::error(
            r.status,
            format!("Failed to fetch underlying price: {}", r.message()),
        )),
    }
}

/// `optiongreeks`.
pub async fn option_greeks(ctx: &AppState, req: &Value) -> Reply {
    let h = match broker_handle(ctx) {
        Ok(h) => h,
        Err(r) => return r,
    };
    let s = |k: &str| req.get(k).and_then(Value::as_str).map(str::to_string);
    let symbol = s("symbol").unwrap_or_default();
    let exchange = s("exchange").unwrap_or_default();
    let c = match parse_option_symbol(&symbol) {
        Ok(c) => c,
        Err(m) => return Reply::error(500, format!("Failed to get option Greeks: {}", m)),
    };
    let expiry_time = match s("expiry_time").map(|v| parse_expiry_time(&v)).transpose() {
        Ok(t) => t,
        Err(m) => return Reply::error(500, format!("Failed to get option Greeks: {}", m)),
    };
    let forward = match req
        .get("forward_price")
        .and_then(Value::as_f64)
        .filter(|f| *f != 0.0)
    {
        Some(f) => f,
        None => match forward_for(
            ctx,
            &h,
            &c,
            s("underlying_symbol").as_deref(),
            s("underlying_exchange").as_deref(),
            &exchange,
        )
        .await
        {
            Ok(f) => f,
            Err(r) => return r,
        },
    };
    let option_price = match fetch_quote(ctx, &h, &symbol, &exchange).await {
        Ok(q) if q.ltp != 0.0 => q.ltp,
        Ok(_) => return Reply::error(404, "Option LTP not available"),
        Err(r) => {
            return Reply::error(
                r.status,
                format!("Failed to fetch option price: {}", r.message()),
            )
        }
    };
    calculate_greeks(
        &symbol,
        &exchange,
        &c,
        forward,
        option_price,
        req.get("interest_rate").and_then(Value::as_f64),
        expiry_time,
        ctx.now(),
    )
}

/// `multioptiongreeks`.
pub async fn multi_option_greeks(ctx: &AppState, req: &Value) -> Reply {
    let h = match broker_handle(ctx) {
        Ok(h) => h,
        Err(r) => return r,
    };
    let items = req
        .get("symbols")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let rate = req.get("interest_rate").and_then(Value::as_f64);
    let expiry_time = match req
        .get("expiry_time")
        .and_then(Value::as_str)
        .map(parse_expiry_time)
        .transpose()
    {
        Ok(t) => t,
        Err(m) => return Reply::error(500, format!("Failed to get option Greeks: {}", m)),
    };
    let now = ctx.now();
    // Option prices in one call.
    let keys: Vec<QuoteKey> = items
        .iter()
        .filter_map(|i| {
            let s = i.get("symbol")?.as_str()?;
            let e = i.get("exchange")?.as_str()?;
            ctx.symbols.by_symbol(e, s).map(|_| QuoteKey::new(e, s))
        })
        .collect();
    let mut prices: HashMap<(String, String), f64> = HashMap::new();
    if !keys.is_empty() {
        if let Ok(rows) = h.broker.get_multiquotes(&h.auth, &keys).await {
            for r in rows {
                if let Some(q) = r.data.filter(|q| q.ltp != 0.0) {
                    prices.insert((r.symbol, r.exchange), q.ltp);
                }
            }
        }
    }
    let mut forwards: HashMap<(String, String, String), Option<f64>> = HashMap::new();
    let mut data = Vec::with_capacity(items.len());
    let (mut ok, mut failed) = (0i64, 0i64);
    let mut errors: Vec<String> = Vec::new();
    for item in &items {
        let sym = item
            .get("symbol")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let ex = item
            .get("exchange")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let err =
            |m: String| json!({"status": "error", "symbol": sym, "exchange": ex, "message": m});
        let c = match parse_option_symbol(&sym) {
            Ok(c) => c,
            Err(m) => {
                let m = format!("Failed to parse option symbol: {}", m);
                errors.push(m.clone());
                data.push(err(m));
                failed += 1;
                continue;
            }
        };
        let us = item
            .get("underlying_symbol")
            .and_then(Value::as_str)
            .map(str::to_string);
        let ux = item
            .get("underlying_exchange")
            .and_then(Value::as_str)
            .map(str::to_string);
        let spot_ex = ux
            .clone()
            .unwrap_or_else(|| greeks_underlying_exchange(&c.base, &ex));
        let fkey = (
            us.clone().unwrap_or_else(|| c.base.clone()),
            spot_ex,
            c.expiry.clone(),
        );
        let forward = match forwards.get(&fkey) {
            Some(f) => *f,
            None => {
                let f = forward_for(ctx, &h, &c, us.as_deref(), ux.as_deref(), &ex)
                    .await
                    .ok();
                forwards.insert(fkey.clone(), f);
                f
            }
        };
        let Some(forward) = forward else {
            let m = format!("Failed to fetch underlying price for {}", fkey.0);
            errors.push(m.clone());
            data.push(err(m));
            failed += 1;
            continue;
        };
        let Some(price) = prices.get(&(sym.clone(), ex.clone())).copied() else {
            let m = "Option LTP not available".to_string();
            errors.push(m.clone());
            data.push(err(m));
            failed += 1;
            continue;
        };
        let r = calculate_greeks(&sym, &ex, &c, forward, price, rate, expiry_time, now);
        if r.is_success() {
            ok += 1;
            data.push(r.body);
        } else if r
            .message()
            .to_ascii_lowercase()
            .contains("option has expired")
        {
            ok += 1;
            data.push(json!({
                "status": "success", "symbol": sym, "exchange": ex, "underlying": c.base,
                "strike": bs::py_round(c.strike, 2), "option_type": c.option_type,
                "expiry_date": expiry_display(&c), "days_to_expiry": 0,
                "spot_price": bs::py_round(forward, 2), "option_price": bs::py_round(price, 2),
                "intrinsic_value": bs::py_round(if c.option_type == "CE" { (forward - c.strike).max(0.0) } else { (c.strike - forward).max(0.0) }, 2),
                "time_value": 0.0,
                "interest_rate": bs::py_round(rate.unwrap_or(0.0), 2),
                "implied_volatility": 0,
                "greeks": {"delta": 0, "gamma": 0, "theta": 0, "vega": 0, "rho": 0},
                "note": "Option has expired - Greeks are no longer applicable",
            }));
        } else {
            failed += 1;
            errors.push(r.message());
            let mut b = r.body;
            if let Some(m) = b.as_object_mut() {
                m.insert("symbol".into(), json!(sym));
                m.insert("exchange".into(), json!(ex));
            }
            data.push(b);
        }
    }
    let status = if failed == 0 {
        "success"
    } else if ok > 0 {
        "partial"
    } else {
        "error"
    };
    let mut body = json!({
        "status": status,
        "data": data,
        "summary": {"total": items.len(), "success": ok, "failed": failed},
    });
    let mut uniq: Vec<String> = Vec::new();
    for e in errors {
        if !e.is_empty() && !uniq.contains(&e) {
            uniq.push(e);
        }
    }
    if failed > 0 && !uniq.is_empty() {
        let joined = uniq.iter().take(3).cloned().collect::<Vec<_>>().join("; ");
        let msg = if status == "error" {
            format!("All option Greeks calculations failed: {}", joined)
        } else {
            format!("{} option Greeks calculation(s) failed: {}", failed, joined)
        };
        if let Some(m) = body.as_object_mut() {
            m.insert("message".into(), json!(msg));
        }
    }
    Reply::ok(body)
}
