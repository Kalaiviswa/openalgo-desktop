//! Orders and books (web `broker/sahi/api/order_api.py`).
//!
//! Sahi has no day-wide trade book, so the trade book is assembled from the
//! trades of every order that has filled. Positions and holdings carry no
//! last price, so it is read once per call from `/v1/marketdata/ltp`.

use super::mapping::{self, int};
use super::ratelimit::Op;
use super::{error_for, Reply, SahiBroker};
use crate::brokers::common::book_read::{self, Book};
use crate::brokers::common::mapping::{Exchange, Product};
use crate::brokers::common::{outcome, position_read};
use crate::brokers::types::*;
use crate::error::{AppError, Result};
use reqwest::{Method, StatusCode};
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

/// `/v1/marketdata/ltp` takes at most this many ids a request.
pub const LTP_BATCH: usize = 100;

/// Time allowed for one book assembled from several calls (the trade book
/// reads the trades of every filled order, four a second).
const BOOK_DEADLINE: Duration = Duration::from_secs(60);

fn first_order_id(v: &Value) -> Option<String> {
    v.get("order_ids")?
        .as_array()?
        .iter()
        .find_map(|id| match id {
            Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
}

pub async fn place_order(
    b: &SahiBroker,
    auth: &AuthToken,
    o: &ResolvedOrder,
) -> Result<OrderResponse> {
    let body = mapping::place_body(o).map_err(outcome::definite)?;
    let r = b
        .send(
            auth,
            Method::POST,
            "/v1/orders",
            &[],
            Some(&body),
            &Op::single(),
        )
        .await?;
    if r.status.is_success() {
        if let Some(id) = first_order_id(&r.body) {
            return Ok(OrderResponse {
                order_id: id,
                message: None,
            });
        }
    }
    let failed = r.body.get("code").is_some();
    if r.status.is_success() {
        tracing::warn!(
            status = r.status.as_u16(),
            "Sahi accepted an order without an id"
        );
    }
    Err(outcome::not_accepted_err(
        r.status.as_u16(),
        failed,
        error_for(r.status, &r.body, "/v1/orders"),
    ))
}

fn order_path(order_id: &str) -> String {
    format!("/v1/orders/{}", urlencoding::encode(order_id))
}

pub async fn modify_order(
    b: &SahiBroker,
    auth: &AuthToken,
    m: &ResolvedModify,
) -> Result<OrderResponse> {
    let path = order_path(&m.order_id);
    let body = mapping::modify_body(m);
    let r = b
        .send(auth, Method::PUT, &path, &[], Some(&body), &Op::single())
        .await?;
    if r.status == StatusCode::OK {
        let id = r
            .body
            .get("order_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(&m.order_id);
        return Ok(OrderResponse {
            order_id: id.to_string(),
            message: None,
        });
    }
    Err(error_for(r.status, &r.body, "/v1/orders/{id}"))
}

pub async fn cancel_order(
    b: &SahiBroker,
    auth: &AuthToken,
    order_id: &str,
) -> Result<OrderResponse> {
    let r = b
        .send(
            auth,
            Method::DELETE,
            &order_path(order_id),
            &[],
            None,
            &Op::single(),
        )
        .await?;
    // 204 No Content on success.
    if matches!(r.status, StatusCode::OK | StatusCode::NO_CONTENT) {
        return Ok(OrderResponse {
            order_id: order_id.to_string(),
            message: None,
        });
    }
    Err(error_for(r.status, &r.body, "/v1/orders/{id}"))
}

/// The rows at `key` of a book answer; anything but a success carrying a
/// list is an error, never an empty book (BR-03).
fn book_rows(r: Reply, path: &str, key: &str, book: Book) -> Result<Vec<Value>> {
    if !r.status.is_success() {
        return Err(error_for(r.status, &r.body, path));
    }
    let rows = book_read::list(&r.body, key, || book_read::unreadable("sahi", book))?;
    if rows.iter().any(|row| !row.is_object()) {
        return Err(book_read::unreadable("sahi", book));
    }
    Ok(rows.to_vec())
}

async fn read_book(
    b: &SahiBroker,
    auth: &AuthToken,
    path: &str,
    key: &str,
    book: Book,
    op: &Op,
) -> Result<Vec<Value>> {
    let r = b.send(auth, Method::GET, path, &[], None, op).await?;
    book_rows(r, path, key, book)
}

async fn raw_orders(b: &SahiBroker, auth: &AuthToken, op: &Op) -> Result<Vec<Value>> {
    read_book(b, auth, "/v1/orders", "orders", Book::Orders, op).await
}

/// Position rows, each carrying a readable `net_qty` (a row without one
/// hides a position).
async fn raw_positions(b: &SahiBroker, auth: &AuthToken) -> Result<Vec<Value>> {
    let rows = read_book(
        b,
        auth,
        "/v1/positions",
        "positions",
        Book::Positions,
        &Op::single(),
    )
    .await?;
    book_read::require_quantities(&rows, &["net_qty"], || {
        book_read::unreadable("sahi", Book::Positions)
    })?;
    Ok(rows)
}

pub async fn get_order_book(b: &SahiBroker, auth: &AuthToken) -> Result<Vec<Order>> {
    let rows = raw_orders(b, auth, &Op::single()).await?;
    Ok(rows
        .iter()
        .map(|r| mapping::map_order(r, b.resolver()))
        .collect())
}

pub async fn get_trade_book(b: &SahiBroker, auth: &AuthToken) -> Result<Vec<Trade>> {
    let op = Op::within(BOOK_DEADLINE);
    let orders = raw_orders(b, auth, &op).await?;
    let mut trades = Vec::new();
    for order in orders.iter().filter(|o| int(o.get("filled_qty")) > 0) {
        let Some(id) = order.get("order_id").and_then(Value::as_str) else {
            return Err(book_read::unreadable("sahi", Book::Trades));
        };
        let path = format!("{}/trades", order_path(id));
        let rows = read_book(b, auth, &path, "trades", Book::Trades, &op).await?;
        trades.extend(rows.iter().map(|r| mapping::map_trade(r, b.resolver())));
    }
    Ok(trades)
}

/// Last price in rupees per instrument id, 100 ids a request. A batch that
/// cannot be read leaves its prices out (web: the book is still shown).
pub(crate) async fn ltp_map(
    b: &SahiBroker,
    auth: &AuthToken,
    ids: &[String],
) -> HashMap<String, f64> {
    let mut unique: Vec<&String> = ids.iter().filter(|i| !i.is_empty()).collect();
    unique.sort();
    unique.dedup();
    let op = Op::within(BOOK_DEADLINE);
    let mut out = HashMap::new();
    for batch in unique.chunks(LTP_BATCH) {
        let query: Vec<(&str, String)> = batch.iter().map(|i| ("i", (*i).clone())).collect();
        match b.get(auth, "/v1/marketdata/ltp", &query, &op).await {
            Ok(v) => {
                if let Some(data) = v.get("data").and_then(Value::as_object) {
                    for (id, q) in data {
                        if let Some(p) = q.get("last_trade_price").filter(|p| !p.is_null()) {
                            out.insert(id.clone(), mapping::from_paise(Some(p)));
                        }
                    }
                }
            }
            Err(e) => tracing::warn!("Sahi LTP could not be read: {}", e.code()),
        }
    }
    out
}

fn id_text(v: Option<&Value>) -> String {
    let id = int(v);
    if id > 0 {
        id.to_string()
    } else {
        String::new()
    }
}

pub async fn get_positions(b: &SahiBroker, auth: &AuthToken) -> Result<Vec<Position>> {
    let rows = raw_positions(b, auth).await?;
    let ids: Vec<String> = rows
        .iter()
        .map(|r| id_text(r.get("instrument_id")))
        .collect();
    let ltps = if rows.is_empty() {
        HashMap::new()
    } else {
        ltp_map(b, auth, &ids).await
    };
    Ok(rows
        .iter()
        .zip(&ids)
        .map(|(r, id)| {
            let ltp = ltps.get(id).copied().unwrap_or(0.0);
            mapping::map_position(r, ltp, b.resolver())
        })
        .collect())
}

pub async fn get_holdings(b: &SahiBroker, auth: &AuthToken) -> Result<Vec<Holding>> {
    let rows = read_book(
        b,
        auth,
        "/v1/holdings",
        "holdings",
        Book::Holdings,
        &Op::single(),
    )
    .await?;
    let ids: Vec<String> = rows
        .iter()
        .filter_map(|r| r.get("instrument_ids").and_then(Value::as_array))
        .flatten()
        .map(|v| id_text(Some(v)))
        .collect();
    let ltps = if ids.is_empty() {
        HashMap::new()
    } else {
        ltp_map(b, auth, &ids).await
    };
    Ok(rows
        .iter()
        .filter_map(|r| mapping::map_holding(r, &ltps, b.resolver()))
        .collect())
}

/// Net quantity of one symbol and product (web `get_open_position`). A book
/// that cannot be read refuses the smart order rather than reading flat.
pub async fn get_open_position(
    b: &SahiBroker,
    auth: &AuthToken,
    symbol: &str,
    exchange: Exchange,
    product: Product,
) -> Result<i64> {
    let row = b.resolver().by_symbol(exchange.as_str(), symbol);
    let segment = mapping::segment(exchange)?;
    let brsymbol = row.as_ref().map(|r| r.br_symbol().to_string());
    let rows = raw_positions(b, auth).await.map_err(|e| {
        tracing::warn!("Sahi positions unreadable for a smart order: {}", e.code());
        position_read::unread("Sahi")
    })?;
    let code = mapping::product_code(product);
    Ok(rows
        .iter()
        .find(|p| {
            p.get("symbol").and_then(Value::as_str) == brsymbol.as_deref()
                && p.get("exchange_segment").and_then(Value::as_str) == Some(segment)
                && p.get("product").and_then(Value::as_str) == Some(code)
        })
        .map(|p| int(p.get("net_qty")))
        .unwrap_or(0))
}

/// Cancel every working order (web `cancel_all_orders_api`).
pub async fn cancel_all_orders(b: &SahiBroker, auth: &AuthToken) -> Result<CancelAllResult> {
    let rows = raw_orders(b, auth, &Op::single()).await?;
    let mut result = CancelAllResult::default();
    for row in rows {
        let status = row.get("status").and_then(Value::as_str).unwrap_or("");
        if !mapping::is_working(status) {
            continue;
        }
        let id = row
            .get("order_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        match cancel_order(b, auth, &id).await {
            Ok(_) => result.cancelled.push(id),
            Err(e) => {
                tracing::warn!("Cancel of a Sahi order failed: {}", e.code());
                result.failed.push(id);
            }
        }
    }
    Ok(result)
}

/// Square off every open position with a MARKET order (web
/// `close_all_positions`). MTF positions have no OpenAlgo product and must
/// be closed in the Sahi app.
pub async fn close_all_positions(b: &SahiBroker, auth: &AuthToken) -> Result<CloseAllResult> {
    let rows = raw_positions(b, auth).await?;
    let mut result = CloseAllResult::default();
    for row in rows.iter().filter(|r| int(r.get("net_qty")) != 0) {
        let p = mapping::map_position(row, 0.0, b.resolver());
        let label = format!("{} ({})", p.symbol, p.exchange);
        if p.product.parse::<Product>().is_err() {
            result.failed.push(format!(
                "{}: {} positions must be closed in the Sahi app",
                label, p.product
            ));
            continue;
        }
        let req = OrderRequest {
            symbol: p.symbol.clone(),
            exchange: p.exchange.clone(),
            side: if p.quantity > 0 { "SELL" } else { "BUY" }.to_string(),
            quantity: p.quantity.abs(),
            price: 0.0,
            order_type: "MARKET".to_string(),
            product: p.product.clone(),
            validity: "DAY".to_string(),
            trigger_price: None,
            disclosed_quantity: None,
            amo: false,
        };
        let placed = match ResolvedOrder::resolve(&req, b.resolver()) {
            Ok(order) => outcome::placing(place_order(b, auth, &order)).await,
            Err(e) => Err::<OrderResponse, AppError>(e),
        };
        result.record(&label, placed);
    }
    Ok(result)
}
