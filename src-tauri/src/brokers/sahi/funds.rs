//! Funds and margin (web `broker/sahi/api/funds.py`, `api/margin_api.py`).
//!
//! `GET /v1/funds` reports one available figure per order kind plus the
//! components behind them, in paise. OpenAlgo's available cash is the cash
//! left after everything blocked against it: the intraday figure with the
//! pledge collateral taken back out (collateral is reported on its own).
//! Utilised debits are the utilised block plus net option premium paid.
//! Realised and unrealised M2M come from the position book, since the
//! funds answer carries only a net loss figure.

use super::mapping::{self, from_paise};
use super::orders;
use super::ratelimit::Op;
use super::{error_for, SahiBroker};
use crate::brokers::common::book_read::{self, Book};
use crate::brokers::types::*;
use crate::error::{AppError, Result};
use reqwest::Method;
use serde_json::{json, Value};

/// The utilised components summed into utilised debits (web).
pub const UTILISED_FIELDS: &[&str] = &[
    "span",
    "exposure",
    "delivery_margin",
    "intraday_margin",
    "delivery_buy",
    "charges",
    "mtm_loss",
];

/// A basket margin call takes at most this many legs.
pub const BASKET_LEG_CAP: usize = 10;

/// Funds from the `/v1/funds` answer and the day's realised and unrealised
/// P&L.
pub fn funds_from(v: &Value, realized: f64, unrealized: f64) -> Result<Funds> {
    let available = book_read::object(v.get("available"), "sahi", Book::Funds)?;
    let empty = json!({});
    let section = |k: &str| v.get(k).filter(|x| x.is_object()).unwrap_or(&empty);
    let (balance, utilised, premium) = (
        section("balance"),
        section("utilised"),
        section("option_premium"),
    );
    let collateral = from_paise(balance.get("collateral"));
    let available_cash =
        mapping::round2(from_paise(available.get("intraday_and_fno_selling")) - collateral);
    let used: f64 = UTILISED_FIELDS
        .iter()
        .map(|f| from_paise(utilised.get(*f)))
        .sum::<f64>()
        + from_paise(premium.get("paid"));
    Ok(Funds {
        available_cash,
        used_margin: mapping::round2(used),
        total_margin: mapping::round2(available_cash + used),
        opening_balance: from_paise(balance.get("opening_balance")),
        payin: from_paise(balance.get("payin")),
        payout: from_paise(balance.get("payout")),
        span: from_paise(utilised.get("span")),
        exposure: from_paise(utilised.get("exposure")),
        collateral,
        m2m_unrealized: mapping::round2(unrealized),
        m2m_realized: mapping::round2(realized),
        utilised_debits: mapping::round2(used),
    })
}

pub async fn get_funds(b: &SahiBroker, auth: &AuthToken) -> Result<Funds> {
    let v = b.get(auth, "/v1/funds", &[], &Op::single()).await?;
    // web: positions that cannot be read leave M2M at 0 rather than failing
    // the funds view.
    let (realized, unrealized) = match orders::get_positions(b, auth).await {
        Ok(rows) => rows.iter().fold((0.0, 0.0), |(r, u), p| {
            (r + p.realized_pnl, u + p.unrealized_pnl)
        }),
        Err(e) => {
            tracing::warn!("Sahi positions unreadable for M2M: {}", e.code());
            (0.0, 0.0)
        }
    };
    funds_from(&v, realized, unrealized)
}

/// The place-order body of one margin leg.
fn leg_body(b: &SahiBroker, leg: &MarginLeg) -> Result<Value> {
    let req = OrderRequest {
        symbol: leg.key.symbol.clone(),
        exchange: leg.key.exchange.clone(),
        side: leg.action.as_str().to_string(),
        quantity: i32::try_from(leg.quantity)
            .map_err(|_| AppError::Validation("Quantity is too large.".into()))?,
        price: leg.price,
        order_type: leg.pricetype.as_str().to_string(),
        product: leg.product.as_str().to_string(),
        validity: "DAY".to_string(),
        trigger_price: Some(leg.trigger_price),
        disclosed_quantity: None,
        amo: false,
    };
    mapping::place_body(&ResolvedOrder::resolve(&req, b.resolver())?)
}

/// The required margin of one call, in rupees.
async fn margin_call(b: &SahiBroker, auth: &AuthToken, path: &str, body: &Value) -> Result<f64> {
    let r = b
        .send(auth, Method::POST, path, &[], Some(body), &Op::single())
        .await?;
    match r.body.get("required_margin") {
        Some(m) if r.status.is_success() => Ok(from_paise(Some(m))),
        _ => Err(error_for(r.status, &r.body, path)),
    }
}

/// One leg to `/v1/margins/order`; more to `/v1/margins/basket` (Sahi nets
/// hedged legs) in baskets of at most 10, whose requirements are added.
/// Sahi reports one required figure, so SPAN and exposure are 0 (web).
pub async fn calculate_margin(
    b: &SahiBroker,
    auth: &AuthToken,
    legs: &[MarginLeg],
) -> Result<MarginResult> {
    if legs.is_empty() {
        return Err(AppError::Validation(
            "No positions to calculate margin for.".into(),
        ));
    }
    let bodies = legs
        .iter()
        .map(|l| leg_body(b, l))
        .collect::<Result<Vec<_>>>()?;
    let mut required = 0.0;
    if let [one] = bodies.as_slice() {
        required = margin_call(b, auth, "/v1/margins/order", one).await?;
    } else {
        if bodies.len() > BASKET_LEG_CAP {
            tracing::info!(
                "Sahi basket margin of {} legs priced in parts of {}",
                bodies.len(),
                BASKET_LEG_CAP
            );
        }
        for part in bodies.chunks(BASKET_LEG_CAP) {
            let body = json!({ "legs": part });
            required += margin_call(b, auth, "/v1/margins/basket", &body).await?;
        }
    }
    Ok(MarginResult {
        total_margin_required: mapping::round2(required),
        span_margin: 0.0,
        exposure_margin: 0.0,
    })
}
