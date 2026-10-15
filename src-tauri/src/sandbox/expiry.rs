//! Contract expiry (web `sandbox/position_manager.py` expiry handling): the
//! expiry read from the symbol or the master contract, the exchange's
//! close on expiry day, the settlement price, and settling and clearing
//! expired positions.

use super::clock::ts;
use super::config::{ExpiryTiming, OptionSettlement, SandboxConfig};
use super::core::Core;
use super::db::PositionRow;
use super::funds;
use super::positions::{claim_for_settlement, get_by_id, list_open, realized_pnl, save};
use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use rusqlite::Connection;
use rust_decimal::Decimal;

const MONTHS: [&str; 12] = [
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
];

/// First `DDMMMYY` in an F&O symbol (web `parse_expiry_from_symbol`).
pub fn parse_expiry_from_symbol(symbol: &str, exchange: &str) -> Option<NaiveDate> {
    if !["NFO", "BFO", "MCX", "CDS", "BCD", "NCDEX", "CRYPTO"].contains(&exchange) {
        return None;
    }
    let b = symbol.as_bytes();
    if b.len() < 7 {
        return None;
    }
    for i in 0..=b.len() - 7 {
        let w = &b[i..i + 7];
        if w[0].is_ascii_digit()
            && w[1].is_ascii_digit()
            && w[2].is_ascii_uppercase()
            && w[3].is_ascii_uppercase()
            && w[4].is_ascii_uppercase()
            && w[5].is_ascii_digit()
            && w[6].is_ascii_digit()
        {
            let day: u32 = ((w[0] - b'0') * 10 + (w[1] - b'0')) as u32;
            let mon = std::str::from_utf8(&w[2..5]).ok()?;
            let year: i32 = 2000 + ((w[5] - b'0') * 10 + (w[6] - b'0')) as i32;
            let month = MONTHS.iter().position(|m| *m == mon)? as u32 + 1;
            return NaiveDate::from_ymd_opt(year, month, day);
        }
    }
    None
}

/// Contract expiry: from the symbol, else from the symbol master.
pub(crate) fn contract_expiry(core: &Core, symbol: &str, exchange: &str) -> Option<NaiveDate> {
    parse_expiry_from_symbol(symbol, exchange)
        .or_else(|| core.symbol(symbol, exchange).and_then(|m| m.expiry))
}

/// Exchange close used for expiry-day settlement (web `EXCHANGE_CLOSE_TIMES`).
pub fn exchange_close(exchange: &str) -> NaiveTime {
    let (h, m) = match exchange {
        "NFO" | "BFO" => (15, 40),
        "CDS" | "BCD" => (17, 0),
        "MCX" => (23, 30),
        "NCDEX" => (17, 0),
        _ => (15, 30),
    };
    NaiveTime::from_hms_opt(h, m, 0).unwrap_or(NaiveTime::MIN)
}

/// Web `is_contract_expired_now`.
pub fn is_contract_expired_now(
    expiry: Option<NaiveDate>,
    exchange: &str,
    now: NaiveDateTime,
    cfg: &SandboxConfig,
) -> bool {
    let Some(expiry) = expiry else {
        return false;
    };
    let today = now.date();
    if today > expiry {
        return true;
    }
    if today < expiry {
        return false;
    }
    if cfg.expiry_settlement_timing != ExpiryTiming::ExpiryDayClose {
        return false;
    }
    now.time() >= exchange_close(exchange)
}

/// Web `get_expiry_settlement_price`.
pub fn expiry_settlement_price(p: &PositionRow, cfg: &SandboxConfig) -> Decimal {
    let ltp = p.ltp.unwrap_or(Decimal::ZERO);
    if p.symbol.ends_with("CE") || p.symbol.ends_with("PE") {
        if cfg.option_expiry_settlement == OptionSettlement::Ltp && ltp > Decimal::ZERO {
            return ltp;
        }
        return Decimal::ZERO;
    }
    if ltp > Decimal::ZERO {
        ltp
    } else {
        p.average_price
    }
}

/// Settle one expired position in the caller's transaction (web
/// `_settle_expired_position`). The row is hidden from the session view by
/// stamping `updated_at` with the expiry date.
pub(crate) fn settle_expired(
    conn: &Connection,
    core: &Core,
    cfg: &SandboxConfig,
    p: &PositionRow,
    now: &str,
) -> rusqlite::Result<bool> {
    if !claim_for_settlement(conn, p.id, p.quantity)? {
        return Ok(false);
    }
    let Some(mut p) = get_by_id(conn, p.id)? else {
        return Ok(false);
    };
    if p.quantity == 0 {
        return Ok(false);
    }
    let cv = core.contract_value(&p.symbol, &p.exchange);
    let settle = expiry_settlement_price(&p, cfg);
    let close_pnl = realized_pnl(p.quantity, p.average_price, p.quantity.abs(), settle, cv);
    let margin = p.margin_blocked;
    let outcome = funds::apply(conn, &p.user_id, cfg.starting_capital, now, |f| {
        funds::release(f, margin.min(f.used_margin), close_pnl, true)
    })?;
    if let Err(r) = outcome {
        tracing::warn!("Expired {} not settled yet: {}", p.symbol, r.0);
        return Ok(false);
    }
    let total = p.accumulated_realized_pnl + close_pnl;
    p.quantity = 0;
    p.ltp = Some(settle);
    p.pnl = total;
    p.accumulated_realized_pnl = total;
    p.margin_blocked = Decimal::ZERO;
    p.updated_at = match contract_expiry(core, &p.symbol, &p.exchange) {
        Some(d) => ts(d.and_hms_opt(0, 0, 0).unwrap_or_default()),
        None => now.to_string(),
    };
    save(conn, &p)?;
    tracing::info!(
        "Settled expired sandbox position {} {} at {} (P&L {})",
        p.symbol,
        p.exchange,
        settle,
        close_pnl
    );
    Ok(true)
}

/// Settle every expired F&O position (all users). Returns how many.
pub(crate) fn cleanup_expired_contracts(
    conn: &Connection,
    core: &Core,
    cfg: &SandboxConfig,
    now: NaiveDateTime,
) -> rusqlite::Result<usize> {
    let now_s = ts(now);
    let mut n = 0;
    for p in list_open(conn, None)? {
        if !["NFO", "BFO", "MCX", "CDS", "BCD", "NCDEX", "CRYPTO"].contains(&p.exchange.as_str()) {
            continue;
        }
        let expiry = contract_expiry(core, &p.symbol, &p.exchange);
        if is_contract_expired_now(expiry, &p.exchange, now, cfg)
            && settle_expired(conn, core, cfg, &p, &now_s)?
        {
            n += 1;
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiry_is_parsed_from_the_symbol() {
        assert_eq!(
            parse_expiry_from_symbol("NIFTY06OCT2622400CE", "NFO"),
            NaiveDate::from_ymd_opt(2026, 10, 6)
        );
        assert_eq!(
            parse_expiry_from_symbol("CRUDEOIL19OCT26FUT", "MCX"),
            NaiveDate::from_ymd_opt(2026, 10, 19)
        );
        assert_eq!(parse_expiry_from_symbol("RELIANCE", "NSE"), None);
        assert_eq!(parse_expiry_from_symbol("NIFTY31XYZ26FUT", "NFO"), None);
    }

    #[test]
    fn expiry_day_close_timing() {
        let cfg = SandboxConfig::default();
        let d = NaiveDate::from_ymd_opt(2026, 10, 6);
        let at = |h, m| d.unwrap().and_hms_opt(h, m, 0).unwrap();
        assert!(!is_contract_expired_now(d, "NFO", at(15, 39), &cfg));
        assert!(is_contract_expired_now(d, "NFO", at(15, 40), &cfg));
        assert!(!is_contract_expired_now(d, "MCX", at(23, 29), &cfg));
        let mut next_day = cfg.clone();
        next_day.expiry_settlement_timing = ExpiryTiming::NextDay;
        assert!(!is_contract_expired_now(d, "NFO", at(23, 59), &next_day));
        let tomorrow = NaiveDate::from_ymd_opt(2026, 10, 7)
            .unwrap()
            .and_hms_opt(0, 0, 1)
            .unwrap();
        assert!(is_contract_expired_now(d, "NFO", tomorrow, &next_day));
    }
}
