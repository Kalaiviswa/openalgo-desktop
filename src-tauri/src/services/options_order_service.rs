//! Options orders (web `place_options_order_service.py`,
//! `options_multiorder_service.py`): resolve the contract from an offset,
//! then place through the order service (sandbox or live, decided once).

use super::batch_order_service::{
    buy_then_sell, split_chunks, SplitRows, MAX_SPLIT_ORDERS, SPLIT_PAUSE,
};
use super::core::{
    analyzer_request, broker_handle, i, meta, mode_of, num, publish, s, safe_request, BrokerHandle,
    Reply,
};
use super::options_service::{resolve_option, OptionPick, ResolvedOption};
use super::order_service::{place_order_with, route_to_pending, uncertain_legs, Route};
use crate::events::Event;
use crate::state::AppState;
use serde_json::{json, Map, Value};

/// The order body for one resolved contract.
fn order_body(req: &Value, leg: &Value, opt: &ResolvedOption, quantity: i64) -> Value {
    let pick = |k: &str| leg.get(k).cloned().unwrap_or(Value::Null);
    json!({
        "apikey": req.get("apikey").cloned().unwrap_or(Value::Null),
        "strategy": s(req, "strategy"),
        "exchange": opt.exchange,
        "symbol": opt.symbol,
        "action": s(leg, "action").to_ascii_uppercase(),
        "pricetype": pick("pricetype"),
        "product": pick("product"),
        "price": pick("price"),
        "trigger_price": pick("trigger_price"),
        "disclosed_quantity": pick("disclosed_quantity"),
        "underlying_ltp": opt.underlying_ltp,
        "quantity": quantity,
    })
}

/// Split one leg into chunks and place them one after another. After a
/// chunk with no definite answer the rest are held back and answered as
/// `splitorder` answers them (LOG-08 P1-5; the web keeps sending).
async fn place_split(
    ctx: &AppState,
    route: Route,
    req: &Value,
    leg: &Value,
    opt: &ResolvedOption,
    quantity: i64,
    size: i64,
) -> SplitRows {
    let chunks = split_chunks(quantity, size);
    let mut rows = SplitRows::new(chunks.len());
    for (k, qty) in chunks.iter().enumerate() {
        let order_num = k + 1;
        if rows.hold(order_num, *qty) {
            continue;
        }
        if k > 0 {
            tokio::time::sleep(SPLIT_PAUSE).await;
        }
        let r = place_order_with(ctx, &order_body(req, leg, opt, *qty), route, false).await;
        rows.record(order_num, *qty, &r);
    }
    rows
}

/// `optionsorder`.
pub async fn options_order(ctx: &AppState, req: &Value, route: Route) -> Reply {
    if let Some(r) = route_to_pending(ctx, "optionsorder", req, route) {
        return r;
    }
    let h = match broker_handle(ctx) {
        Ok(h) => h,
        Err(r) => return r,
    };
    let analyze = match route.analyze(ctx) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let opt = match resolve_option(ctx, &h, &OptionPick::from_request(req)).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    let quantity = i(req, "quantity");
    let splitsize = i(req, "splitsize");
    let common = |m: &mut Map<String, Value>| {
        m.insert("symbol".into(), json!(opt.symbol));
        m.insert("exchange".into(), json!(opt.exchange));
        m.insert("underlying".into(), json!(s(req, "underlying")));
        m.insert("underlying_ltp".into(), num(opt.underlying_ltp));
        m.insert("offset".into(), json!(s(req, "offset")));
        m.insert(
            "option_type".into(),
            json!(s(req, "option_type").to_ascii_uppercase()),
        );
    };
    if splitsize > 0 {
        if split_chunks(quantity, splitsize).len() as i64 > MAX_SPLIT_ORDERS {
            return Reply::error(
                400,
                format!(
                    "Total number of orders would exceed maximum limit of {}",
                    MAX_SPLIT_ORDERS
                ),
            );
        }
        let split = place_split(ctx, route, req, req, &opt, quantity, splitsize).await;
        let successful = split
            .results
            .iter()
            .filter(|r| r["status"] == "success")
            .count() as i64;
        let total = split.results.len() as i64;
        let mut m = Map::new();
        m.insert("status".into(), json!("success"));
        common(&mut m);
        m.insert("total_quantity".into(), json!(quantity));
        m.insert("split_size".into(), json!(splitsize));
        m.insert("results".into(), json!(split.results));
        if analyze {
            m.insert("mode".into(), json!("analyze"));
        }
        let mut reply = Reply::ok(Value::Object(m));
        reply.placement = uncertain_legs(&split.uncertain);
        publish(
            ctx,
            Event::OptionsCompleted {
                meta: meta(
                    mode_of(analyze),
                    "optionsorder",
                    analyzer_request(req, "optionsorder"),
                    &reply.body,
                ),
                symbol: opt.symbol.clone(),
                action: s(req, "action").to_ascii_uppercase(),
                exchange: opt.exchange.clone(),
                pricetype: Some(s(req, "pricetype")),
                product: Some(s(req, "product")),
                successful,
                total,
            },
        );
        return reply;
    }
    let r = place_order_with(ctx, &order_body(req, req, &opt, quantity), route, true).await;
    if !r.is_success() {
        return r;
    }
    let mut m = Map::new();
    m.insert("status".into(), json!("success"));
    m.insert(
        "orderid".into(),
        r.body.get("orderid").cloned().unwrap_or(Value::Null),
    );
    common(&mut m);
    if let Some(mode) = r.body.get("mode") {
        m.insert("mode".into(), mode.clone());
    }
    Reply::new(r.status, Value::Object(m))
}

/// `optionsmultiorder`.
pub async fn options_multi_order(ctx: &AppState, req: &Value, route: Route) -> Reply {
    if let Some(r) = route_to_pending(ctx, "optionsmultiorder", req, route) {
        return r;
    }
    let h = match broker_handle(ctx) {
        Ok(h) => h,
        Err(r) => return r,
    };
    let analyze = match route.analyze(ctx) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let legs = numbered_legs(req);
    if legs.is_empty() {
        let msg = "No legs provided in the request";
        return if analyze {
            super::core::analyzer_error(ctx, "optionsmultiorder", req, msg, 400)
        } else {
            Reply::error(400, msg)
        };
    }
    let shared_ltp = shared_underlying_ltp(ctx, &h, req, &legs).await;
    let order = MultiOrder {
        ctx,
        h: &h,
        route,
        req,
        analyze,
        shared_ltp,
    };
    let mut results: Vec<(i64, Value)> = Vec::with_capacity(legs.len());
    let has_split = legs.iter().any(|l| i(l, "splitsize") > 0);
    for (n, leg) in buy_then_sell(&legs).iter().enumerate() {
        if n > 0 && has_split {
            tokio::time::sleep(SPLIT_PAUSE).await;
        }
        results.push((i(leg, "_leg"), order.leg(leg).await));
    }
    multi_order_reply(ctx, req, results, shared_ltp, analyze)
}

/// The legs of a multi-order, each numbered in `_leg` (from 1) in request
/// order.
fn numbered_legs(req: &Value) -> Vec<Value> {
    req.get("legs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(n, mut l)| {
            if let Some(m) = l.as_object_mut() {
                m.insert("_leg".into(), json!(n + 1));
            }
            l
        })
        .collect()
}

/// The underlying's LTP once, shared by every leg: from the ATM call of the
/// request's expiry, else the first leg's.
async fn shared_underlying_ltp(
    ctx: &AppState,
    h: &BrokerHandle,
    req: &Value,
    legs: &[Value],
) -> Option<f64> {
    let expiry = req.get("expiry_date").and_then(Value::as_str).or_else(|| {
        legs.iter()
            .find_map(|l| l.get("expiry_date").and_then(Value::as_str))
    });
    let named = OptionPick::from_request(req);
    let atm = OptionPick::atm(named.underlying, named.exchange, expiry, "CE");
    resolve_option(ctx, h, &atm)
        .await
        .ok()
        .map(|o| o.underlying_ltp)
}

/// One `optionsmultiorder` request: what every leg shares.
struct MultiOrder<'a> {
    ctx: &'a AppState,
    h: &'a BrokerHandle,
    route: Route,
    req: &'a Value,
    analyze: bool,
    /// The underlying's LTP, resolved once.
    shared_ltp: Option<f64>,
}

/// What a leg's reply entry repeats from the leg.
struct LegHead {
    leg_no: i64,
    offset: String,
    ot: String,
    action: String,
}

impl MultiOrder<'_> {
    /// Resolve and place one leg; its entry in the reply.
    async fn leg(&self, leg: &Value) -> Value {
        let head = LegHead {
            leg_no: i(leg, "_leg"),
            offset: s(leg, "offset"),
            ot: s(leg, "option_type").to_ascii_uppercase(),
            action: s(leg, "action").to_ascii_uppercase(),
        };
        let expiry = leg
            .get("expiry_date")
            .and_then(Value::as_str)
            .or_else(|| self.req.get("expiry_date").and_then(Value::as_str));
        let pick = OptionPick {
            expiry_date: expiry,
            offset: &head.offset,
            option_type: &head.ot,
            known_ltp: self.shared_ltp,
            ..OptionPick::from_request(self.req)
        };
        let (leg_no, offset, ot, action) = (head.leg_no, &head.offset, &head.ot, &head.action);
        match resolve_option(self.ctx, self.h, &pick).await {
            Ok(opt) if i(leg, "splitsize") > 0 => self.place_split_leg(leg, &opt, &head).await,
            Ok(opt) => self.place_leg(leg, &opt, &head).await,
            Err(r) => json!({
                "leg": leg_no, "offset": offset, "strike": null, "option_type": ot,
                "action": action, "status": "error", "message": r.message(),
            }),
        }
    }

    /// One leg in chunks of its `splitsize`.
    async fn place_split_leg(&self, leg: &Value, opt: &ResolvedOption, head: &LegHead) -> Value {
        let (leg_no, offset, ot, action) = (head.leg_no, &head.offset, &head.ot, &head.action);
        let qty = i(leg, "quantity");
        let size = i(leg, "splitsize");
        if split_chunks(qty, size).len() as i64 > MAX_SPLIT_ORDERS {
            return json!({
                "leg": leg_no, "symbol": opt.symbol, "exchange": opt.exchange, "offset": offset,
                "strike": null, "option_type": ot, "action": action, "status": "error",
                "message": "Split orders would exceed maximum limit of 100 per leg",
            });
        }
        let split = place_split(self.ctx, self.route, self.req, leg, opt, qty, size)
            .await
            .results;
        let any_ok = split.iter().any(|r| r["status"] == "success");
        json!({
            "leg": leg_no, "symbol": opt.symbol, "exchange": opt.exchange,
            "product": s(leg, "product"), "offset": offset, "strike": null,
            "option_type": ot, "action": action,
            "status": if any_ok { "success" } else { "error" },
            "total_quantity": qty, "split_size": size, "split_results": split,
            "mode": if self.analyze { "analyze" } else { "live" },
        })
    }

    /// One leg as one order.
    async fn place_leg(&self, leg: &Value, opt: &ResolvedOption, head: &LegHead) -> Value {
        let (leg_no, offset, ot, action) = (head.leg_no, &head.offset, &head.ot, &head.action);
        let body = order_body(self.req, leg, opt, i(leg, "quantity"));
        let r = place_order_with(self.ctx, &body, self.route, false).await;
        if r.is_success() {
            return json!({
                "leg": leg_no, "symbol": opt.symbol, "exchange": opt.exchange,
                "product": s(leg, "product"), "offset": offset, "strike": null,
                "option_type": ot, "action": action, "status": "success",
                "orderid": r.body.get("orderid").cloned().unwrap_or(Value::Null),
                "mode": r.body.get("mode").cloned().unwrap_or(json!("live")),
            });
        }
        let msg = r.message();
        json!({
            "leg": leg_no, "symbol": opt.symbol, "exchange": opt.exchange, "offset": offset,
            "strike": null, "option_type": ot, "action": action, "status": "error",
            "message": if msg.is_empty() { "Order placement failed".to_string() } else { msg },
        })
    }
}

/// The `optionsmultiorder` reply, legs in request order, and its event.
fn multi_order_reply(
    ctx: &AppState,
    req: &Value,
    mut results: Vec<(i64, Value)>,
    shared_ltp: Option<f64>,
    analyze: bool,
) -> Reply {
    results.sort_by_key(|(n, _)| *n);
    let results: Vec<Value> = results.into_iter().map(|(_, v)| v).collect();
    let successful = results.iter().filter(|r| r["status"] == "success").count() as i64;
    let total = results.len() as i64;
    let mut m = Map::new();
    m.insert("status".into(), json!("success"));
    m.insert("underlying".into(), json!(s(req, "underlying")));
    m.insert(
        "underlying_ltp".into(),
        shared_ltp.map(num).unwrap_or(Value::Null),
    );
    m.insert("results".into(), json!(results));
    if analyze {
        m.insert("mode".into(), json!("analyze"));
    }
    let reply = Reply::ok(Value::Object(m));
    let request = if analyze {
        analyzer_request(req, "optionsmultiorder")
    } else {
        safe_request(req)
    };
    publish(
        ctx,
        Event::MultiOrderCompleted {
            meta: meta(mode_of(analyze), "optionsmultiorder", request, &reply.body),
            underlying: s(req, "underlying"),
            strategy: Some(s(req, "strategy")),
            exchange: s(req, "exchange"),
            successful_legs: successful,
            failed_legs: total - successful,
            total,
        },
    );
    reply
}
