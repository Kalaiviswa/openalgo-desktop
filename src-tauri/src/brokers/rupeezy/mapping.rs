//! OpenAlgo <-> Vortex vocabulary (web `mapping/exchange.py`,
//! `mapping/transform_data.py`, the status map of `mapping/order_data.py`).
//!
//! Vortex exchanges are `NSE_EQ`, `BSE_EQ`, `NSE_FO`, `BSE_FO`, `MCX_FO`;
//! indices sit inside `NSE_EQ` / `BSE_EQ`. Every call takes a ticker,
//! `<NSE|BSE|MCX>:<symbol>`; the master stores the part after the colon as
//! `brsymbol` and the Vortex exchange as `brexchange`.
//!
//! Quantities: OpenAlgo counts units everywhere (one CRUDEOIL lot is 100).
//! Vortex takes units on NSE_FO and BSE_FO but lots on MCX_FO, and reports
//! MCX book quantities in lots.

use crate::brokers::common::mapping::{PriceType, Product};
use crate::brokers::common::symbols::SymToken;
use crate::brokers::lower_status;
use crate::error::{AppError, Result};

/// Vortex exchange of a book row -> OpenAlgo exchange. Book rows are never
/// indices; an unknown exchange passes through.
pub fn oa_exchange(vortex: &str) -> String {
    match vortex.trim().to_ascii_uppercase().as_str() {
        "NSE_EQ" => "NSE",
        "BSE_EQ" => "BSE",
        "NSE_FO" => "NFO",
        "BSE_FO" => "BFO",
        "MCX_FO" => "MCX",
        _ => return vortex.trim().to_string(),
    }
    .to_string()
}

/// A master row's ticker, e.g. `NSE_FO` + `NIFTY26OCTFUT` ->
/// `NSE:NIFTY26OCTFUT`.
pub fn ticker(brexchange: &str, brsymbol: &str) -> String {
    let prefix: String = brexchange.chars().take(3).collect();
    format!("{}:{}", prefix, brsymbol)
}

/// The master row's ticker.
pub fn row_ticker(row: &SymToken) -> String {
    ticker(row.br_exchange(), row.br_symbol())
}

/// The symbol part of a ticker (`NSE:SBIN` -> `SBIN`).
pub fn split_ticker(ticker: &str) -> &str {
    ticker.split_once(':').map(|(_, s)| s).unwrap_or(ticker)
}

/// Whether Vortex counts this exchange's quantities in lots.
fn lot_denominated(brexchange: &str) -> bool {
    brexchange.eq_ignore_ascii_case("MCX_FO")
}

/// OpenAlgo units -> the quantity Vortex takes. An MCX quantity that is not
/// a whole number of lots is refused before anything is sent.
pub fn to_vortex_quantity(units: i64, brexchange: &str, lot_size: i64) -> Result<i64> {
    if lot_denominated(brexchange) && lot_size > 1 {
        if units % lot_size != 0 {
            return Err(AppError::Validation(format!(
                "Quantity {} is not a multiple of the lot size {}.",
                units, lot_size
            )));
        }
        return Ok(units / lot_size);
    }
    Ok(units)
}

/// A Vortex book quantity -> OpenAlgo units.
pub fn from_vortex_quantity(quantity: i64, brexchange: &str, lot_size: i64) -> i64 {
    if lot_denominated(brexchange) {
        quantity.saturating_mul(lot_size.max(1))
    } else {
        quantity
    }
}

/// OpenAlgo price type -> Vortex `variety`.
pub fn variety(p: PriceType) -> &'static str {
    match p {
        PriceType::Market => "RL-MKT",
        PriceType::Limit => "RL",
        PriceType::Sl => "SL",
        PriceType::SlM => "SL-MKT",
    }
}

/// Vortex `variety` -> OpenAlgo price type; unknown values pass through.
pub fn price_type_from_variety(v: &str) -> String {
    match v.trim().to_ascii_uppercase().as_str() {
        "RL-MKT" => "MARKET",
        "RL" => "LIMIT",
        "SL" => "SL",
        "SL-MKT" => "SL-M",
        _ => return v.to_string(),
    }
    .to_string()
}

/// Market varieties go with price 0.
pub fn is_market(variety: &str) -> bool {
    matches!(variety, "RL-MKT" | "SL-MKT")
}

/// Stop varieties carry a trigger.
pub fn is_stop(variety: &str) -> bool {
    matches!(variety, "SL" | "SL-MKT")
}

/// OpenAlgo product -> Vortex product. Vortex has one carry product,
/// `DELIVERY`, for cash and F&O alike.
pub fn product(p: Product) -> &'static str {
    match p {
        Product::Cnc | Product::Nrml => "DELIVERY",
        Product::Mis => "INTRADAY",
    }
}

/// Vortex product -> OpenAlgo product: `DELIVERY` (and the cash-only
/// `MTF` / `BTST`) is CNC on NSE/BSE and NRML on derivatives.
pub fn product_from_vortex(oa_exchange: &str, vortex: &str) -> String {
    let p = vortex.trim().to_ascii_uppercase();
    match p.as_str() {
        "INTRADAY" => "MIS".into(),
        "DELIVERY" | "MTF" | "BTST" => {
            if matches!(oa_exchange, "NSE" | "BSE" | "NSE_EQ" | "BSE_EQ") {
                "CNC".into()
            } else {
                "NRML".into()
            }
        }
        _ => p,
    }
}

/// Vortex order status -> the web's lowercase status. `OMSXMITTED` is "sent
/// to the exchange, not yet acknowledged".
pub fn map_status(status: &str) -> String {
    match status.trim().to_ascii_uppercase().as_str() {
        "EXECUTED" | "COMPLETE" => "complete".into(),
        "PENDING" | "OPEN" | "OMSXMITTED" => "open".into(),
        "TRIGGER PENDING" | "TRIGGER_PENDING" => "trigger pending".into(),
        "CANCELLED" => "cancelled".into(),
        "REJECTED" => "rejected".into(),
        _ => lower_status(status),
    }
}

/// An order a cancel-all touches.
pub fn is_open(status: &str) -> bool {
    matches!(map_status(status).as_str(), "open" | "trigger pending")
}
