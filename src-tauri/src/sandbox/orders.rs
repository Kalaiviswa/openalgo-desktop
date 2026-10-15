//! Order placement, modification, cancellation and the order book (web
//! `sandbox/order_manager.py`, `services/sandbox_service.py`).

use super::clock::{display_seconds, ts};
use super::config::SandboxConfig;
use super::core::{blocking, Core, EngineCmd};
use super::db::OrderRow;
use super::events::{order_update, Outbox};
use super::funds;
use super::replies::{
    CancelAllReply, FailedCancellation, OrderMessage, OrderStatistics, OrderStatusData,
    OrderStatusReply, OrderbookData, OrderbookReply, OrderbookRow, ANALYZE, SUCCESS,
};
use super::types::{
    dec_to_db, money, Action, OrderStatus, PriceType, Product, SandboxError, SbResult, SymbolKey,
    LOT_SIZE_EXCHANGES, VALID_EXCHANGES,
};
use chrono::{NaiveDateTime, Timelike};
use rand::Rng;
use rusqlite::{params, Connection, OptionalExtension};
use rust_decimal::Decimal;
use std::sync::Arc;

/// A new order, as the API layer hands it over (web `sandbox_order_data`).
/// Strings are taken as sent; validation and upper-casing happen here so the
/// error messages match the web's.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OrderRequest {
    pub symbol: String,
    pub exchange: String,
    pub action: String,
    pub quantity: i64,
    /// `None` or zero means "not given".
    pub price: Option<Decimal>,
    /// `None` or zero means "not given".
    pub trigger_price: Option<Decimal>,
    pub price_type: String,
    pub product: String,
    pub strategy: String,
}

/// Fields a modify may change (web `sandbox_modify_order`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModifyRequest {
    pub quantity: Option<i64>,
    pub price: Option<Decimal>,
    pub trigger_price: Option<Decimal>,
}

/// A smart order: reach `position_size` (web `sandbox_place_smart_order`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SmartOrderRequest {
    pub symbol: String,
    pub exchange: String,
    pub product: String,
    pub action: String,
    pub quantity: i64,
    pub position_size: i64,
    pub price: Option<Decimal>,
    pub trigger_price: Option<Decimal>,
    /// Defaults to MARKET when empty.
    pub price_type: String,
    pub strategy: String,
}

fn given(v: Option<Decimal>) -> Option<Decimal> {
    v.filter(|d| !d.is_zero())
}

/// Web `_validate_order`: `Err(message)` on the first problem.
pub fn validate(req: &OrderRequest) -> Result<(), String> {
    let missing = |f: &str| Err(format!("Missing required field: {f}"));
    if req.symbol.trim().is_empty() {
        return missing("symbol");
    }
    if req.exchange.trim().is_empty() {
        return missing("exchange");
    }
    if req.action.trim().is_empty() {
        return missing("action");
    }
    if req.quantity == 0 {
        return missing("quantity");
    }
    if req.price_type.trim().is_empty() {
        return missing("price_type");
    }
    if req.product.trim().is_empty() {
        return missing("product");
    }
    if Action::parse(&req.action).is_none() {
        return Err("Invalid action. Must be BUY or SELL".to_string());
    }
    let Some(pt) = PriceType::parse(&req.price_type) else {
        return Err("Invalid price_type. Must be MARKET, LIMIT, SL, or SL-M".to_string());
    };
    let Some(product) = Product::parse(&req.product) else {
        return Err("Invalid product. Must be CNC, NRML, or MIS".to_string());
    };
    let exchange = req.exchange.trim().to_ascii_uppercase();
    if (exchange == "NSE" || exchange == "BSE") && product == Product::Nrml {
        return Err(format!(
            "NRML product not allowed for {exchange} equity segment. Use CNC for delivery or MIS for intraday."
        ));
    }
    if ["NFO", "BFO", "MCX", "CDS", "BCD", "NCDEX", "CRYPTO"].contains(&exchange.as_str())
        && product == Product::Cnc
    {
        return Err(format!(
            "CNC product not allowed for {exchange} derivatives segment. Use NRML for carryforward or MIS for intraday."
        ));
    }
    if req.quantity <= 0 {
        return Err("Quantity must be positive".to_string());
    }
    if pt.has_price() {
        match given(req.price) {
            None => return Err(format!("{} orders require price", req.price_type)),
            Some(p) if p <= Decimal::ZERO => return Err("Price must be positive".to_string()),
            _ => {}
        }
    }
    if pt.has_trigger() {
        match given(req.trigger_price) {
            None => return Err(format!("{} orders require trigger_price", req.price_type)),
            Some(p) if p <= Decimal::ZERO => {
                return Err("Trigger price must be positive".to_string())
            }
            _ => {}
        }
    }
    if !VALID_EXCHANGES.contains(&exchange.as_str()) {
        return Err(format!(
            "Invalid exchange. Must be one of {}",
            VALID_EXCHANGES.join(", ")
        ));
    }
    Ok(())
}

/// A validated, normalised order.
#[derive(Debug, Clone)]
pub(crate) struct Normalized {
    pub symbol: String,
    pub exchange: String,
    pub action: Action,
    pub quantity: i64,
    pub price: Option<Decimal>,
    pub trigger_price: Option<Decimal>,
    pub price_type: PriceType,
    pub product: Product,
    pub strategy: String,
}

pub(super) fn normalize(core: &Core, req: &OrderRequest) -> SbResult<Normalized> {
    validate(req).map_err(SandboxError::bad_request)?;
    let exchange = req.exchange.trim().to_ascii_uppercase();
    let price_type = PriceType::parse(&req.price_type).unwrap_or(PriceType::Market);
    let mut price = given(req.price);
    let mut trigger = given(req.trigger_price);
    if matches!(price_type, PriceType::Market | PriceType::SlM) {
        price = None;
    }
    if matches!(price_type, PriceType::Market | PriceType::Limit) {
        trigger = None;
    }
    let symbol = req.symbol.trim().to_string();
    let Some(meta) = core.symbol(&symbol, &exchange) else {
        return Err(SandboxError::bad_request(format!(
            "Symbol {symbol} not found on {exchange}"
        )));
    };
    let quantity = req.quantity;
    if LOT_SIZE_EXCHANGES.contains(&exchange.as_str()) {
        let lot = if meta.lotsize > 0 { meta.lotsize } else { 1 };
        if quantity % lot != 0 {
            return Err(SandboxError::bad_request(format!(
                "Quantity must be in multiples of lot size {lot}"
            )));
        }
    }
    Ok(Normalized {
        symbol,
        exchange,
        action: Action::parse(&req.action).unwrap_or(Action::Buy),
        quantity,
        price,
        trigger_price: trigger,
        price_type,
        product: Product::parse(&req.product).unwrap_or(Product::Mis),
        strategy: req.strategy.clone(),
    })
}

// ---------------------------------------------------------------------------
// SQL
// ---------------------------------------------------------------------------

pub fn get_by_orderid(
    conn: &Connection,
    user: &str,
    orderid: &str,
) -> rusqlite::Result<Option<OrderRow>> {
    conn.query_row(
        &format!(
            "SELECT {} FROM sandbox_orders WHERE orderid = ?1 AND user_id = ?2",
            OrderRow::COLUMNS
        ),
        params![orderid, user],
        OrderRow::from_row,
    )
    .optional()
}

pub fn get_by_id(conn: &Connection, id: i64) -> rusqlite::Result<Option<OrderRow>> {
    conn.query_row(
        &format!(
            "SELECT {} FROM sandbox_orders WHERE id = ?1",
            OrderRow::COLUMNS
        ),
        params![id],
        OrderRow::from_row,
    )
    .optional()
}

/// Pending orders (all users), optionally for one symbol.
pub fn pending(conn: &Connection, key: Option<&SymbolKey>) -> rusqlite::Result<Vec<OrderRow>> {
    let mut out = Vec::new();
    match key {
        Some(k) => {
            let mut stmt = conn.prepare_cached(&format!(
                "SELECT {} FROM sandbox_orders WHERE symbol = ?1 AND exchange = ?2
                   AND order_status IN ('open','trigger pending') ORDER BY id",
                OrderRow::COLUMNS
            ))?;
            for r in stmt.query_map(params![k.symbol, k.exchange], OrderRow::from_row)? {
                out.push(r?);
            }
        }
        None => {
            let mut stmt = conn.prepare_cached(&format!(
                "SELECT {} FROM sandbox_orders WHERE order_status IN ('open','trigger pending') ORDER BY id",
                OrderRow::COLUMNS
            ))?;
            for r in stmt.query_map([], OrderRow::from_row)? {
                out.push(r?);
            }
        }
    }
    Ok(out)
}

/// The user's orders since `since`, newest first.
pub fn orders_since(conn: &Connection, user: &str, since: &str) -> rusqlite::Result<Vec<OrderRow>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {} FROM sandbox_orders WHERE user_id = ?1 AND order_timestamp >= ?2
         ORDER BY order_timestamp DESC, id DESC",
        OrderRow::COLUMNS
    ))?;
    let rows = stmt.query_map(params![user, since], OrderRow::from_row)?;
    rows.collect()
}

/// Web order id: `YYMMDD` + 6-digit microseconds + 2 random digits.
pub fn generate_order_id(now: NaiveDateTime, attempt: u32) -> String {
    let mut rng = rand::thread_rng();
    let prefix = now.format("%y%m%d");
    if attempt == 0 {
        let micro = now.nanosecond() / 1000 % 1_000_000;
        format!("{prefix}{micro:06}{:02}", rng.gen_range(0..100))
    } else {
        format!("{prefix}{:08}", rng.gen_range(0..100_000_000u32))
    }
}

pub(super) struct NewOrder<'a> {
    pub(super) user: &'a str,
    pub(super) n: &'a Normalized,
    pub(super) price: Option<Decimal>,
    pub(super) status: OrderStatus,
    pub(super) pending_quantity: i64,
    pub(super) rejection_reason: Option<&'a str>,
    pub(super) margin: Decimal,
    pub(super) gtt_leg_id: Option<i64>,
    pub(super) now: NaiveDateTime,
}

pub(super) fn insert_order(conn: &Connection, o: &NewOrder) -> rusqlite::Result<OrderRow> {
    let now_s = ts(o.now);
    for attempt in 0..20 {
        let orderid = generate_order_id(o.now, attempt);
        let r = conn.execute(
            "INSERT INTO sandbox_orders (orderid, user_id, strategy, symbol, exchange, action,
               quantity, price, trigger_price, price_type, product, order_status, average_price,
               filled_quantity, pending_quantity, rejection_reason, margin_blocked, gtt_leg_id,
               order_timestamp, update_timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, NULL, 0, ?13, ?14, ?15, ?16, ?17, ?17)",
            params![
                orderid,
                o.user,
                o.n.strategy,
                o.n.symbol,
                o.n.exchange,
                o.n.action.as_str(),
                o.n.quantity,
                o.price.map(dec_to_db),
                o.n.trigger_price.map(dec_to_db),
                o.n.price_type.as_str(),
                o.n.product.as_str(),
                o.status.as_str(),
                o.pending_quantity,
                o.rejection_reason,
                dec_to_db(o.margin),
                o.gtt_leg_id,
                now_s
            ],
        );
        match r {
            Ok(_) => {
                let id = conn.last_insert_rowid();
                if let Some(row) = get_by_id(conn, id)? {
                    return Ok(row);
                }
            }
            Err(rusqlite::Error::SqliteFailure(e, msg))
                if e.code == rusqlite::ErrorCode::ConstraintViolation
                    && msg
                        .as_deref()
                        .map(|m| m.contains("orderid"))
                        .unwrap_or(true) =>
            {
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    Err(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT),
        Some("could not allocate a unique sandbox order id".into()),
    ))
}

// ---------------------------------------------------------------------------
// Modify / cancel
// ---------------------------------------------------------------------------

pub(crate) async fn modify(
    core: &Arc<Core>,
    orderid: &str,
    m: ModifyRequest,
) -> SbResult<OrderMessage> {
    let orderid = orderid.to_string();
    blocking(core, move |c| {
        c.db.with_tx(|tx| {
            let Some(order) = get_by_orderid(tx, c.user(), &orderid)? else {
                return Err(SandboxError::not_found(format!(
                    "Order {orderid} not found"
                )));
            };
            if !order.order_status.is_pending() {
                return Err(SandboxError::bad_request(format!(
                    "Cannot modify order in {} status",
                    order.order_status.as_str()
                )));
            }
            let mut quantity = order.quantity;
            if let Some(q) = m.quantity {
                if q <= 0 {
                    return Err(SandboxError::bad_request("Quantity must be positive"));
                }
                if LOT_SIZE_EXCHANGES.contains(&order.exchange.as_str()) {
                    if let Some(meta) = c.symbol(&order.symbol, &order.exchange) {
                        let lot = if meta.lotsize > 0 { meta.lotsize } else { 1 };
                        if q % lot != 0 {
                            return Err(SandboxError::bad_request(format!(
                                "Quantity must be in multiples of lot size {lot}"
                            )));
                        }
                    }
                }
                quantity = q;
            }
            let mut price = order.price;
            if let Some(p) = given(m.price) {
                if !order.price_type.has_price() {
                    return Err(SandboxError::bad_request(format!(
                        "{} orders do not accept a price",
                        order.price_type.as_str()
                    )));
                }
                price = Some(p);
            }
            let mut trigger = order.trigger_price;
            if let Some(t) = given(m.trigger_price) {
                if !order.price_type.has_trigger() {
                    return Err(SandboxError::bad_request(format!(
                        "{} orders do not accept a trigger_price",
                        order.price_type.as_str()
                    )));
                }
                trigger = Some(t);
            }
            let n = tx.execute(
                "UPDATE sandbox_orders SET quantity = ?1, pending_quantity = ?1, price = ?2,
                   trigger_price = ?3, update_timestamp = ?4
                 WHERE id = ?5 AND order_status IN ('open','trigger pending')",
                params![
                    quantity,
                    price.map(dec_to_db),
                    trigger.map(dec_to_db),
                    c.now_ts(),
                    order.id
                ],
            )?;
            if n != 1 {
                return Err(SandboxError::bad_request(format!(
                    "Cannot modify order in {} status",
                    order.order_status.as_str()
                )));
            }
            Ok(OrderMessage::new(
                order.orderid,
                "Order modified successfully",
            ))
        })
    })
    .await
}

/// Cancel inside a transaction: conditional status change plus the margin
/// release, together.
pub(crate) fn cancel_in_tx(
    tx: &Connection,
    core: &Core,
    order: &OrderRow,
    outbox: &mut Outbox,
) -> SbResult<()> {
    if !order.order_status.is_pending() {
        return Err(SandboxError::bad_request(format!(
            "Cannot cancel order in {} status",
            order.order_status.as_str()
        )));
    }
    let now = core.now_ts();
    let n = tx.execute(
        "UPDATE sandbox_orders SET order_status = 'cancelled', update_timestamp = ?1
         WHERE id = ?2 AND order_status IN ('open','trigger pending')",
        params![now, order.id],
    )?;
    if n != 1 {
        let status = get_by_id(tx, order.id)?
            .map(|o| o.order_status.as_str())
            .unwrap_or("unknown");
        return Err(SandboxError::bad_request(format!(
            "Cannot cancel order in {status} status"
        )));
    }
    if order.margin_blocked > Decimal::ZERO {
        let cfg = SandboxConfig::load(tx)?;
        let amount = order.margin_blocked;
        let r = funds::apply(tx, &order.user_id, cfg.starting_capital, &now, |f| {
            funds::release(f, amount, Decimal::ZERO, true)
        })?;
        if let Err(r) = r {
            return Err(SandboxError::conflict(format!(
                "Cannot cancel order: {}. The order remains open; check its status before trying again.",
                r.0
            )));
        }
    }
    outbox.push(order_update(order, OrderStatus::Cancelled, ""));
    Ok(())
}

pub(crate) async fn cancel(core: &Arc<Core>, orderid: &str) -> SbResult<OrderMessage> {
    let orderid = orderid.to_string();
    blocking(core, move |c| {
        let mut outbox = Outbox::new();
        let r = c.db.with_tx(|tx| {
            let Some(order) = get_by_orderid(tx, c.user(), &orderid)? else {
                return Err(SandboxError::not_found(format!(
                    "Order {orderid} not found"
                )));
            };
            cancel_in_tx(tx, c, &order, &mut outbox)?;
            Ok(OrderMessage::new(
                order.orderid,
                "Order cancelled successfully",
            ))
        });
        if r.is_ok() {
            c.publish(outbox);
            // The symbol may no longer need ticks.
            c.notify_engine(EngineCmd::Rebuild);
        }
        r
    })
    .await
}

/// Cancel every resting order of the current session: `open` and
/// `trigger pending`. (The web looked for `"trigger_pending"` and so never
/// cancelled a resting SL or SL-M order.)
pub(crate) async fn cancel_all(core: &Arc<Core>) -> SbResult<CancelAllReply> {
    let ids = blocking(core, |c| {
        let since = ts(c.session_start());
        let rows =
            c.db.with_conn(|conn| orders_since(conn, c.user(), &since))?;
        Ok(rows
            .into_iter()
            .filter(|o| o.order_status.is_pending())
            .map(|o| o.orderid)
            .collect::<Vec<_>>())
    })
    .await?;
    if ids.is_empty() {
        return Ok(CancelAllReply {
            status: SUCCESS,
            message: "No open orders to cancel".to_string(),
            canceled_orders: vec![],
            failed_cancellations: vec![],
            mode: ANALYZE,
        });
    }
    let mut canceled = Vec::new();
    let mut failed = Vec::new();
    for id in ids {
        match cancel(core, &id).await {
            Ok(_) => canceled.push(id),
            Err(e) => failed.push(FailedCancellation {
                orderid: id,
                message: e.message,
            }),
        }
    }
    Ok(CancelAllReply {
        status: SUCCESS,
        message: format!(
            "Canceled {} orders. Failed to cancel {} orders.",
            canceled.len(),
            failed.len()
        ),
        canceled_orders: canceled,
        failed_cancellations: failed,
        mode: ANALYZE,
    })
}

// ---------------------------------------------------------------------------
// Books
// ---------------------------------------------------------------------------

fn statistics(orders: &[OrderRow]) -> OrderStatistics {
    let mut s = OrderStatistics::default();
    for o in orders {
        match o.action {
            Action::Buy => s.total_buy_orders += 1,
            Action::Sell => s.total_sell_orders += 1,
        }
        match o.order_status {
            OrderStatus::Complete => s.total_completed_orders += 1,
            OrderStatus::Open => s.total_open_orders += 1,
            OrderStatus::Rejected => s.total_rejected_orders += 1,
            OrderStatus::TriggerPending => s.total_trigger_pending_orders += 1,
            OrderStatus::Cancelled => {}
        }
    }
    s
}

pub fn orderbook_reply(orders: &[OrderRow]) -> OrderbookReply {
    let rows = orders
        .iter()
        .map(|o| OrderbookRow {
            orderid: o.orderid.clone(),
            symbol: o.symbol.clone(),
            exchange: o.exchange.clone(),
            action: o.action.as_str().to_string(),
            quantity: o.quantity,
            price: money(o.price.unwrap_or(Decimal::ZERO)),
            trigger_price: money(o.trigger_price.unwrap_or(Decimal::ZERO)),
            pricetype: o.price_type.as_str().to_string(),
            product: o.product.as_str().to_string(),
            order_status: o.order_status.as_str().to_string(),
            average_price: money(o.average_price.unwrap_or(Decimal::ZERO)),
            filled_quantity: o.filled_quantity,
            pending_quantity: o.pending_quantity,
            rejection_reason: o.rejection_reason.clone().unwrap_or_default(),
            timestamp: display_seconds(&o.order_timestamp),
            strategy: o.strategy.clone().unwrap_or_default(),
        })
        .collect();
    OrderbookReply {
        status: SUCCESS,
        data: OrderbookData {
            orders: rows,
            statistics: statistics(orders),
        },
        mode: ANALYZE,
    }
}

pub fn order_status_reply(o: &OrderRow) -> OrderStatusReply {
    OrderStatusReply {
        status: SUCCESS,
        data: OrderStatusData {
            orderid: o.orderid.clone(),
            symbol: o.symbol.clone(),
            exchange: o.exchange.clone(),
            action: o.action.as_str().to_string(),
            quantity: o.quantity,
            price: money(o.price.unwrap_or(Decimal::ZERO)),
            trigger_price: money(o.trigger_price.unwrap_or(Decimal::ZERO)),
            price_type: o.price_type.as_str().to_string(),
            product: o.product.as_str().to_string(),
            order_status: o.order_status.as_str().to_string(),
            average_price: money(o.average_price.unwrap_or(Decimal::ZERO)),
            filled_quantity: o.filled_quantity,
            pending_quantity: o.pending_quantity,
            timestamp: display_seconds(&o.order_timestamp),
            strategy: o.strategy.clone().unwrap_or_default(),
        },
        mode: ANALYZE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(action: &str, pt: &str, product: &str, exchange: &str) -> OrderRequest {
        OrderRequest {
            symbol: "SBIN".into(),
            exchange: exchange.into(),
            action: action.into(),
            quantity: 1,
            price: None,
            trigger_price: None,
            price_type: pt.into(),
            product: product.into(),
            strategy: String::new(),
        }
    }

    #[test]
    fn validation_messages_match_the_web() {
        assert_eq!(
            validate(&req("BUY", "LIMIT", "MIS", "NSE")).unwrap_err(),
            "LIMIT orders require price"
        );
        assert_eq!(
            validate(&OrderRequest {
                price: Some(Decimal::from(900)),
                ..req("BUY", "SL", "MIS", "NSE")
            })
            .unwrap_err(),
            "SL orders require trigger_price"
        );
        assert!(validate(&req("BUY", "MARKET", "NRML", "NSE"))
            .unwrap_err()
            .starts_with("NRML product not allowed for NSE"));
        assert!(validate(&req("BUY", "MARKET", "CNC", "NFO"))
            .unwrap_err()
            .starts_with("CNC product not allowed for NFO"));
        assert_eq!(
            validate(&req("HOLD", "MARKET", "MIS", "NSE")).unwrap_err(),
            "Invalid action. Must be BUY or SELL"
        );
        assert_eq!(
            validate(&OrderRequest {
                quantity: 0,
                ..req("BUY", "MARKET", "MIS", "NSE")
            })
            .unwrap_err(),
            "Missing required field: quantity"
        );
        assert!(validate(&req("buy", "market", "mis", "nse")).is_ok());
        assert!(validate(&req("BUY", "MARKET", "MIS", "XYZ"))
            .unwrap_err()
            .starts_with("Invalid exchange"));
    }

    #[test]
    fn order_ids_are_fourteen_digits_with_the_date_prefix() {
        let now = chrono::NaiveDate::from_ymd_opt(2026, 10, 3)
            .unwrap()
            .and_hms_micro_opt(9, 42, 28, 123456)
            .unwrap();
        for attempt in 0..3 {
            let id = generate_order_id(now, attempt);
            assert_eq!(id.len(), 14, "{id}");
            assert!(id.starts_with("261003"));
            assert!(id.chars().all(|c| c.is_ascii_digit()));
        }
        assert!(generate_order_id(now, 0).starts_with("261003123456"));
    }
}
