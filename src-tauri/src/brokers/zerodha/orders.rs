//! Orders and books (web `api/order_api.py`, `mapping/transform_data.py`).

use super::mapping::{self, KiteHolding, KiteOrder, KiteTrade};
use super::{Body, Category, ZerodhaBroker};
use crate::brokers::common::book_read::{self, Book};
use crate::brokers::common::mapping::{Exchange, Product};
use crate::brokers::common::master_contract::format_strike;
use crate::brokers::common::outcome;
use crate::brokers::common::position_read::unread;
use crate::brokers::types::*;
use crate::error::{AppError, Result};
use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
struct OrderId {
    #[serde(deserialize_with = "crate::brokers::common::de::string_lenient")]
    order_id: String,
}

/// Number as the web sends it in a form (`"0"`, `"1500.5"`).
pub(crate) fn num(v: f64) -> String {
    format_strike(v)
}

fn mcx_lot(inst: &crate::brokers::common::SymToken) -> Option<i64> {
    (inst.lot_size > 0).then_some(i64::from(inst.lot_size))
}

/// The form `place_order_api` posts (web `transform_data`). `tag` is the
/// order's own client tag. The web sends the constant `openalgo`; a tag per
/// order is what lets an order whose placement had no definite answer be
/// found in the order book (LOG-08, a deliberate departure).
pub fn place_order_form(o: &ResolvedOrder, tag: &str) -> Result<Vec<(&'static str, String)>> {
    let br = o.brsymbol().to_string();
    let ex = o.exchange.as_str();
    let lot = mcx_lot(&o.instrument);
    let qty = mapping::to_kite_quantity(o.quantity, &br, ex, lot, "Quantity")?;
    let dq = mapping::to_kite_quantity(o.disclosed_quantity, &br, ex, lot, "Disclosed quantity")?;
    Ok(vec![
        ("tradingsymbol", br.clone()),
        ("exchange", o.brexchange().to_string()),
        ("transaction_type", o.action.as_str().to_string()),
        ("order_type", o.pricetype.as_str().to_string()),
        ("quantity", qty.to_string()),
        ("product", o.product.as_str().to_string()),
        ("price", num(o.price)),
        ("trigger_price", num(o.trigger_price)),
        ("disclosed_quantity", dq.to_string()),
        ("validity", "DAY".to_string()),
        ("market_protection", "-1".to_string()),
        ("tag", tag.to_string()),
    ])
}

/// The form `modify_order` puts (web `transform_modify_order_data`).
pub fn modify_order_form(m: &ResolvedModify) -> Result<Vec<(&'static str, String)>> {
    let br = m.brsymbol().to_string();
    let ex = m.exchange.as_str();
    let lot = mcx_lot(&m.instrument);
    let qty = mapping::to_kite_quantity(m.quantity, &br, ex, lot, "Quantity")?;
    let dq = mapping::to_kite_quantity(m.disclosed_quantity, &br, ex, lot, "Disclosed quantity")?;
    let mut form = vec![
        ("order_type", m.pricetype.as_str().to_string()),
        ("quantity", qty.to_string()),
        (
            "price",
            if m.price != 0.0 {
                num(m.price)
            } else {
                "0".into()
            },
        ),
        ("disclosed_quantity", dq.to_string()),
        ("validity", "DAY".to_string()),
    ];
    if m.trigger_price != 0.0 {
        form.push(("trigger_price", num(m.trigger_price)));
    }
    Ok(form)
}

pub async fn place_order(
    b: &ZerodhaBroker,
    auth: &AuthToken,
    o: &ResolvedOrder,
) -> Result<OrderResponse> {
    let tag = outcome::new_client_tag();
    let form = place_order_form(o, &tag)?;
    let variety = if o.amo { "amo" } else { "regular" };
    let placed = b
        .call::<OrderId>(
            Method::POST,
            &format!("/orders/{}", variety),
            auth,
            Body::Form(&form),
            Category::Order,
        )
        .await
        .map(|r| OrderResponse {
            order_id: r.order_id,
            message: None,
        });
    outcome::with_client_tag(placed, &tag)
}

/// The order book with each order's tag (the reconciler's lookup).
pub async fn get_order_book_tagged(
    b: &ZerodhaBroker,
    auth: &AuthToken,
) -> Result<Vec<TaggedOrder>> {
    let rows = raw_orders(b, auth).await?;
    let tags: Vec<Option<String>> = rows
        .iter()
        .map(|r| Some(r.tag.trim().to_string()).filter(|t| !t.is_empty()))
        .collect();
    Ok(mapping::map_orders(rows, b.resolver())
        .into_iter()
        .zip(tags)
        .map(|(order, client_tag)| TaggedOrder { order, client_tag })
        .collect())
}

pub async fn modify_order(
    b: &ZerodhaBroker,
    auth: &AuthToken,
    m: &ResolvedModify,
) -> Result<OrderResponse> {
    let form = modify_order_form(m)?;
    let r: OrderId = b
        .call(
            Method::PUT,
            &format!("/orders/regular/{}", urlencoding::encode(&m.order_id)),
            auth,
            Body::Form(&form),
            Category::Order,
        )
        .await?;
    Ok(OrderResponse {
        order_id: r.order_id,
        message: None,
    })
}

pub async fn cancel_order(
    b: &ZerodhaBroker,
    auth: &AuthToken,
    order_id: &str,
) -> Result<OrderResponse> {
    let r: OrderId = b
        .call(
            Method::DELETE,
            &format!("/orders/regular/{}", urlencoding::encode(order_id)),
            auth,
            Body::None,
            Category::Order,
        )
        .await?;
    Ok(OrderResponse {
        order_id: r.order_id,
        message: None,
    })
}

/// The order list. BR-03: a success reply without the list is an order
/// book that was not read, never an empty one (Kite sends `[]` for no
/// orders).
async fn raw_orders(b: &ZerodhaBroker, auth: &AuthToken) -> Result<Vec<KiteOrder>> {
    read_orders(b, auth, || book_read::unreadable("zerodha", Book::Orders)).await
}

/// The order list, read with `unread` when it cannot be.
async fn read_orders(
    b: &ZerodhaBroker,
    auth: &AuthToken,
    unread: impl Fn() -> AppError,
) -> Result<Vec<KiteOrder>> {
    let env = b
        .call_raw::<Value>(Method::GET, "/orders", auth, Body::None, Category::Other)
        .await?;
    book_read::rows(&env_data(env), "list", unread)
}

/// Kite's `data` under one key, so the book readers read every list the
/// same way (a missing `data` reads as a missing list).
fn env_data(env: mapping::KiteEnvelope<Value>) -> Value {
    json!({ "list": env.data.unwrap_or(Value::Null) })
}

/// Net positions, read with `unread` when the book cannot be: `data` and
/// `data.net` must both be there and every row must carry its quantity
/// (BR-03). Kite sends `{"net": [], "day": []}` for no positions; the web
/// raises on a missing `net` too.
async fn net_positions(
    b: &ZerodhaBroker,
    auth: &AuthToken,
    unread: impl Fn() -> AppError,
) -> Result<Vec<mapping::KitePosition>> {
    let env = b
        .call_raw::<Value>(
            Method::GET,
            "/portfolio/positions",
            auth,
            Body::None,
            Category::Other,
        )
        .await?;
    let data = env.data.unwrap_or(Value::Null);
    let rows = book_read::list(&data, "net", &unread)?;
    book_read::require_quantities(rows, &["quantity"], &unread)?;
    book_read::typed(rows, unread)
}

/// Raw Kite net positions (MCX quantities in contracts), as the Positions
/// page and the funds P&L read them.
pub(crate) async fn raw_positions(
    b: &ZerodhaBroker,
    auth: &AuthToken,
) -> Result<Vec<mapping::KitePosition>> {
    net_positions(b, auth, || {
        book_read::unreadable("zerodha", Book::Positions)
    })
    .await
}

/// BR-03: net positions for a decision to trade (smart-order sizing, close
/// all): a book that was not read refuses with the web's position-read
/// sentence.
async fn strict_positions(
    b: &ZerodhaBroker,
    auth: &AuthToken,
) -> Result<Vec<mapping::KitePosition>> {
    net_positions(b, auth, || unread("Zerodha")).await
}

/// web `cancel_all_orders`. BR-03: a success reply without the order list
/// is an order book that was not read, never "nothing to cancel" (Kite
/// sends `[]` for no orders).
pub async fn cancel_all_orders(b: &ZerodhaBroker, auth: &AuthToken) -> Result<CancelAllResult> {
    let orders = read_orders(b, auth, || {
        AppError::Broker(
            "Zerodha did not return your order book, so no order was cancelled. Check your orders and try again."
                .into(),
        )
    })
    .await?;
    let mut result = CancelAllResult::default();
    for o in orders {
        // web filters Kite's own strings
        if o.status != "OPEN" && o.status != "TRIGGER PENDING" {
            continue;
        }
        match cancel_order(b, auth, &o.order_id).await {
            Ok(_) => result.cancelled.push(o.order_id),
            Err(e) => {
                tracing::warn!("Cancel of order {} failed: {}", o.order_id, e.code());
                result.failed.push(o.order_id)
            }
        }
    }
    Ok(result)
}

pub async fn get_open_position(
    b: &ZerodhaBroker,
    auth: &AuthToken,
    symbol: &str,
    exchange: Exchange,
    product: Product,
) -> Result<i64> {
    let ex = exchange.as_str();
    let row = b.resolver().by_symbol(ex, symbol);
    let br = row
        .as_ref()
        .map(|r| r.br_symbol().to_string())
        .unwrap_or_else(|| symbol.to_string());
    let lot = row.as_ref().and_then(mcx_lot);
    for p in strict_positions(b, auth).await? {
        if p.tradingsymbol == br && p.exchange == ex && p.product == product.as_str() {
            // This value decides whether to trade, so an unknown MCX size is
            // an error, never a factor of 1.
            if mapping::units_per_contract(&br, ex, lot).is_none() {
                return Err(AppError::Validation(format!(
                    "Cannot read the open position in {}: MCX has revised its contract size and the master contract has no row for this expiry. Re-download the master contract, then retry.",
                    br
                )));
            }
            return Ok(mapping::from_kite_quantity(p.quantity, &br, ex, lot));
        }
    }
    Ok(0)
}

pub async fn close_all_positions(b: &ZerodhaBroker, auth: &AuthToken) -> Result<CloseAllResult> {
    let symbols = b.resolver().clone();
    let mut result = CloseAllResult::default();
    for p in strict_positions(b, auth).await? {
        if p.quantity == 0 {
            continue;
        }
        let lot = symbols
            .by_brsymbol(&p.exchange, &p.tradingsymbol)
            .and_then(|r| mcx_lot(&r));
        let qty = mapping::from_kite_quantity(p.quantity, &p.tradingsymbol, &p.exchange, lot).abs();
        let symbol = symbols.oa_symbol_or_raw(&p.tradingsymbol, &p.exchange);
        let label = format!("{} ({})", symbol, p.exchange);
        let req = OrderRequest {
            symbol,
            exchange: p.exchange.clone(),
            side: if p.quantity > 0 { "SELL" } else { "BUY" }.into(),
            quantity: i32::try_from(qty).unwrap_or(i32::MAX),
            price: 0.0,
            order_type: "MARKET".into(),
            product: p.product.clone(),
            validity: "DAY".into(),
            trigger_price: None,
            disclosed_quantity: None,
            amo: false,
        };
        let placed = match ResolvedOrder::resolve(&req, &symbols) {
            Ok(o) => crate::brokers::common::outcome::placing(place_order(b, auth, &o)).await,
            Err(e) => Err(e),
        };
        result.record(&label, placed);
    }
    Ok(result)
}

pub async fn get_order_book(b: &ZerodhaBroker, auth: &AuthToken) -> Result<Vec<Order>> {
    Ok(mapping::map_orders(
        raw_orders(b, auth).await?,
        b.resolver(),
    ))
}

pub async fn get_trade_book(b: &ZerodhaBroker, auth: &AuthToken) -> Result<Vec<Trade>> {
    let env = b
        .call_raw::<Value>(Method::GET, "/trades", auth, Body::None, Category::Other)
        .await?;
    let rows: Vec<KiteTrade> = book_read::rows(&env_data(env), "list", || {
        book_read::unreadable("zerodha", Book::Trades)
    })?;
    Ok(mapping::map_trades(rows, b.resolver()))
}

pub async fn get_positions(b: &ZerodhaBroker, auth: &AuthToken) -> Result<Vec<Position>> {
    Ok(mapping::map_positions(
        raw_positions(b, auth).await?,
        b.resolver(),
    ))
}

pub async fn get_holdings(b: &ZerodhaBroker, auth: &AuthToken) -> Result<Vec<Holding>> {
    let env = b
        .call_raw::<Value>(
            Method::GET,
            "/portfolio/holdings",
            auth,
            Body::None,
            Category::Other,
        )
        .await?;
    let rows: Vec<KiteHolding> = book_read::rows(&env_data(env), "list", || {
        book_read::unreadable("zerodha", Book::Holdings)
    })?;
    Ok(mapping::map_holdings(rows, b.resolver()))
}
