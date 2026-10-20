//! Smart orders (web `place_smart_order_service.py` and the brokers'
//! `place_smartorder_api`): read the open position, decide the order that
//! takes it to `position_size`, and place it, under the position's lock.

use super::{action, broker_error_reply, fractional_refusal, opt_dec, place_live, placed_event};
use super::{route_to_pending, Route};
use crate::brokers::common::mapping::{Exchange, Product};
use crate::events::{Event, Mode};
use crate::services::core::{
    analyzer_request, broker_handle, f, i, meta, order_failed, publish, s, safe_request, Reply,
};
use crate::state::AppState;
use serde_json::{json, Value};
use std::sync::OnceLock;

/// What a smart order should do to reach `position_size`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmartDecision {
    Place { action: String, quantity: i64 },
    NoAction(&'static str),
}

pub const NO_OPEN_POSITION: &str = "No OpenPosition Found. Not placing Exit order.";
pub const ALREADY_MATCHED: &str = "Positions Already Matched. No Action needed.";

/// The web's smart-order decision table (broker `place_smartorder_api`).
pub fn smart_decision(current: i64, target: i64, quantity: i64, action: &str) -> SmartDecision {
    if target == 0 && current == 0 && quantity != 0 {
        return SmartDecision::Place {
            action: action.to_ascii_uppercase(),
            quantity,
        };
    }
    if target == current {
        return SmartDecision::NoAction(if quantity == 0 {
            NO_OPEN_POSITION
        } else {
            ALREADY_MATCHED
        });
    }
    let (a, q) = if target == 0 && current > 0 {
        ("SELL", current.abs())
    } else if target == 0 && current < 0 {
        ("BUY", current.abs())
    } else if current == 0 {
        (if target > 0 { "BUY" } else { "SELL" }, target.abs())
    } else if target > current {
        ("BUY", target - current)
    } else {
        ("SELL", current - target)
    };
    SmartDecision::Place {
        action: a.to_string(),
        quantity: q,
    }
}

/// Striped locks so two smart orders on one position never both read the
/// same open quantity (bounded: a fixed number of stripes).
const STRIPES: usize = 64;

pub(crate) fn stripe(key: &str) -> &'static tokio::sync::Mutex<()> {
    static LOCKS: OnceLock<Vec<tokio::sync::Mutex<()>>> = OnceLock::new();
    let locks = LOCKS.get_or_init(|| (0..STRIPES).map(|_| tokio::sync::Mutex::new(())).collect());
    let mut h: u64 = 1469598103934665603;
    for b in key.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(1099511628211);
    }
    &locks[(h % STRIPES as u64) as usize]
}

fn no_action_event(mode: Mode, req: &Value, request: Value, reply: &Reply) -> Event {
    Event::OrderNoAction {
        meta: meta(mode, "placesmartorder", request, &reply.body),
        symbol: s(req, "symbol"),
        exchange: s(req, "exchange"),
        message: reply.message(),
    }
}

/// `placesmartorder` in analyzer mode: the sandbox decides and places,
/// with the web's analyze-mode events.
async fn smart_order_in_sandbox(ctx: &AppState, req: &Value, target: i64) -> Reply {
    let sreq = crate::sandbox::SmartOrderRequest {
        symbol: s(req, "symbol"),
        exchange: s(req, "exchange"),
        product: s(req, "product"),
        action: action(req),
        quantity: i(req, "quantity"),
        position_size: target,
        price: opt_dec(f(req, "price")),
        trigger_price: opt_dec(f(req, "trigger_price")),
        price_type: s(req, "pricetype"),
        strategy: s(req, "strategy"),
    };
    let request = analyzer_request(req, "placesmartorder");
    match ctx.sandbox.place_smart_order(sreq).await {
        Ok(crate::sandbox::replies::SmartOrderReply::Placed(p)) => {
            let reply = Reply::from_ser(&p);
            publish(
                ctx,
                placed_event(Mode::Analyze, "placesmartorder", req, request, &reply),
            );
            reply
        }
        Ok(crate::sandbox::replies::SmartOrderReply::NoAction(m)) => {
            let reply = Reply::from_ser(&m);
            publish(ctx, no_action_event(Mode::Analyze, req, request, &reply));
            reply
        }
        Err(e) => {
            let reply = Reply::sandbox(&e);
            publish(
                ctx,
                placed_event(Mode::Analyze, "placesmartorder", req, request, &reply),
            );
            reply
        }
    }
}

/// `placesmartorder`.
pub async fn place_smart_order(ctx: &AppState, req: &Value, route: Route) -> Reply {
    if let Some(r) =
        route_to_pending(ctx, "smartorder", req, route).or_else(|| fractional_refusal(req))
    {
        return r;
    }
    let target = f(req, "position_size").trunc() as i64;
    let analyze = match route.analyze(ctx) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if analyze {
        return smart_order_in_sandbox(ctx, req, target).await;
    }
    let h = match broker_handle(ctx) {
        Ok(h) => h,
        Err(r) => return r,
    };
    let symbol = s(req, "symbol");
    let (Ok(exchange), Ok(product)) = (
        s(req, "exchange").parse::<Exchange>(),
        s(req, "product").parse::<Product>(),
    ) else {
        return Reply::error(400, "Invalid exchange or product");
    };
    // Decide once, under the position's lock: read, decide, dispatch.
    let key = format!("{}:{}:{}", exchange, symbol, product);
    let _guard = stripe(&key).lock().await;
    let current = match h
        .broker
        .get_open_position(&h.auth, &symbol, exchange, product)
        .await
    {
        Ok(q) => q,
        Err(e) => {
            let reply = broker_error_reply(&e, "Failed to place smart order due to internal error");
            order_failed(
                ctx,
                "placesmartorder",
                req,
                &reply,
                &symbol,
                exchange.as_str(),
            );
            return reply;
        }
    };
    match smart_decision(current, target, i(req, "quantity"), &action(req)) {
        SmartDecision::NoAction(msg) => {
            let reply = Reply::ok(json!({"status": "success", "message": msg}));
            publish(
                ctx,
                no_action_event(Mode::Live, req, safe_request(req), &reply),
            );
            reply
        }
        SmartDecision::Place { action, quantity } => {
            let mut order = req.clone();
            if let Some(m) = order.as_object_mut() {
                m.insert("action".into(), json!(action));
                m.insert("quantity".into(), json!(quantity));
            }
            let reply = place_live(&h, ctx, &order).await;
            if reply.is_success() {
                publish(
                    ctx,
                    placed_event(
                        Mode::Live,
                        "placesmartorder",
                        req,
                        safe_request(req),
                        &reply,
                    ),
                );
            } else {
                order_failed(
                    ctx,
                    "placesmartorder",
                    req,
                    &reply,
                    &symbol,
                    exchange.as_str(),
                );
            }
            reply
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(a: &str, q: i64) -> SmartDecision {
        SmartDecision::Place {
            action: a.into(),
            quantity: q,
        }
    }

    /// The web's smart-order table, row by row.
    #[test]
    fn smart_order_decision_table() {
        // (current, target, quantity, action) -> decision
        type Row = ((i64, i64, i64, &'static str), SmartDecision);
        let rows: Vec<Row> = vec![
            ((0, 0, 5, "buy"), place("BUY", 5)),
            ((0, 0, 0, "BUY"), SmartDecision::NoAction(NO_OPEN_POSITION)),
            ((5, 5, 0, "BUY"), SmartDecision::NoAction(NO_OPEN_POSITION)),
            (
                (-3, -3, 3, "SELL"),
                SmartDecision::NoAction(ALREADY_MATCHED),
            ),
            ((0, 10, 10, "BUY"), place("BUY", 10)),
            ((0, -3, 3, "SELL"), place("SELL", 3)),
            ((10, 15, 5, "BUY"), place("BUY", 5)),
            ((15, 5, 10, "SELL"), place("SELL", 10)),
            ((5, 0, 0, "SELL"), place("SELL", 5)),
            ((-4, 0, 0, "BUY"), place("BUY", 4)),
            ((-4, 2, 6, "BUY"), place("BUY", 6)),
            ((4, -2, 6, "SELL"), place("SELL", 6)),
        ];
        for ((c, t, q, a), want) in rows {
            assert_eq!(
                smart_decision(c, t, q, a),
                want,
                "current {} target {}",
                c,
                t
            );
        }
    }

    #[test]
    fn stripes_are_stable_and_bounded() {
        let a = stripe("NSE:SBIN:MIS") as *const _;
        let b = stripe("NSE:SBIN:MIS") as *const _;
        assert_eq!(a, b);
    }
}
