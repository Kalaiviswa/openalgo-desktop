//! An order's shape as the matcher compares it (P1-4): the price type it
//! was sent with and its limit and trigger prices, read the way brokers
//! show them after market price protection.

use super::Unconfirmed;
use crate::strategy::dispatch::BookOrder;

/// `SL-M`, `SL_M`, `slm` -> `SLM`.
fn price_kind(t: &str) -> String {
    t.trim()
        .chars()
        .filter(|c| !matches!(c, '-' | '_' | ' '))
        .collect::<String>()
        .to_ascii_uppercase()
}

/// Whether a book row has the order's shape: the price type it was sent
/// with (a MARKET order may show as LIMIT, an SL-M as SL, after market
/// price protection), and its limit and trigger prices where they apply.
/// A side that does not say is not compared.
pub(super) fn same_shape(u: &Unconfirmed<'_>, r: &BookOrder) -> bool {
    let Some(sent) = u.pricetype.map(price_kind).filter(|k| !k.is_empty()) else {
        return true;
    };
    let shown = price_kind(&r.pricetype);
    if shown.is_empty() {
        return true;
    }
    let compatible =
        sent == shown || (sent == "MARKET" && shown == "LIMIT") || (sent == "SLM" && shown == "SL");
    if !compatible {
        return false;
    }
    let close = |a: f64, b: f64| (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1.0);
    if sent == shown && (sent == "LIMIT" || sent == "SL") {
        if let Some(p) = u.price.filter(|p| *p > 0.0) {
            if r.price > 0.0 && !close(r.price, p) {
                return false;
            }
        }
    }
    if sent == "SL" || sent == "SLM" {
        if let Some(t) = u.trigger_price.filter(|t| *t > 0.0) {
            if r.trigger_price > 0.0 && !close(r.trigger_price, t) {
                return false;
            }
        }
    }
    true
}
