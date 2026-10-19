//! Funds (web `api/funds.py`).
//!
//! `GET /user/funds` answers an NSE bucket (cash, F&O, currency), an MCX
//! bucket and `exchange_combined`, Vortex's own total, which is preferred.
//! Vortex reports no unrealised figure, so both P&L fields come from the
//! position book at live prices and add up to its net P&L (taking realized
//! from the buckets instead counted a partly closed position's booked part
//! twice). The buckets are the fallback only when positions cannot be
//! read.

use super::books::VortexPosition;
use super::client::{Answer, Call};
use super::data::num;
use super::orders::priced_positions;
use super::RupeezyBroker;
use crate::brokers::common::streaming::round2;
use crate::brokers::types::{AuthToken, Funds};
use crate::error::{AppError, Result};
use serde_json::Value;

/// `(realized, unrealized)` over the position book; a row whose split is
/// unknown (no price) is left out of both (web `_positions_pnl`).
pub(crate) fn positions_pnl(rows: &[VortexPosition]) -> (f64, f64) {
    rows.iter()
        .fold((0.0, 0.0), |(r, u), p| match p.pnl_split() {
            Some((pr, pu)) => (r + pr, u + pu),
            None => {
                tracing::warn!("Rupeezy funds P&L leaves out {}: no live price", p.ticker);
                (r, u)
            }
        })
}

/// The buckets to total: `exchange_combined` when Vortex sends it, else
/// NSE and MCX. `None` when the answer carries none of them.
fn buckets(body: &Value) -> Option<Vec<&Value>> {
    let combined = body.get("exchange_combined").filter(|v| v.is_object());
    if let Some(c) = combined {
        return Some(vec![c]);
    }
    let parts: Vec<&Value> = ["nse", "mcx"]
        .iter()
        .filter_map(|k| body.get(*k).filter(|v| v.is_object()))
        .collect();
    (!parts.is_empty()).then_some(parts)
}

/// Funds from a `/user/funds` body (top-level buckets, or under `data`)
/// and the position book's P&L when it could be read.
pub(crate) fn funds_from(answer: &Value, pnl: Option<(f64, f64)>) -> Option<Funds> {
    let body = answer
        .get("data")
        .filter(|d| d.is_object())
        .unwrap_or(answer);
    let parts = buckets(body)?;
    let total = |k: &str| parts.iter().map(|b| num(b.get(k))).sum::<f64>();
    let (realized, unrealized) =
        pnl.unwrap_or_else(|| (total("booked_profit") + total("mtm_and_booked_loss"), 0.0));
    // Utilisation is reported as a (negative) debit; OpenAlgo shows it
    // positive.
    let used = total("total_utilization").abs();
    Some(Funds {
        available_cash: round2(total("net_available")),
        collateral: round2(total("collateral")),
        m2m_unrealized: round2(unrealized),
        m2m_realized: round2(realized),
        utilised_debits: round2(used),
        used_margin: round2(used),
        ..Default::default()
    })
}

fn unavailable(a: &Answer) -> AppError {
    let msg = a.message();
    tracing::warn!(
        status = a.status.as_u16(),
        "Rupeezy funds unavailable: {}",
        msg
    );
    AppError::Broker(if msg.is_empty() {
        "Rupeezy did not return your funds. Try again shortly.".into()
    } else {
        msg
    })
}

pub async fn get_funds(b: &RupeezyBroker, auth: &AuthToken) -> Result<Funds> {
    let a = b.call(auth, Call::get("/user/funds")).await?;
    if a.failed() || !a.status.is_success() || funds_from(&a.body, None).is_none() {
        return Err(unavailable(&a));
    }
    let pnl = match priced_positions(b, auth).await {
        Ok(rows) => Some(positions_pnl(&rows)),
        Err(e) => {
            tracing::warn!("Rupeezy funds P&L from positions unavailable: {}", e.code());
            None
        }
    };
    funds_from(&a.body, pnl).ok_or_else(|| unavailable(&a))
}
