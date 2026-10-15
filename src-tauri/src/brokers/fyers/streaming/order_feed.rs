//! The order-update socket (web `fyers_order_adapter.py`,
//! `wss://socket.fyers.in/trade/v3`): order statuses mapped to OpenAlgo's.

use super::super::mapping::exchange_name;
use super::request_with_auth;
use crate::brokers::common::streaming::{
    BrokerFeed, FeedEvent, FeedSubscription, Message, OrderUpdate, WsRequest,
};
use crate::brokers::common::symbols::SymbolResolver;
use crate::brokers::types::AuthToken;
use crate::error::Result;
use crate::security::Secret;
use serde_json::{json, Value};
use std::time::Duration;

/// web `_STATUS_MAP` (order socket): transit and pending are both open.
pub fn order_feed_status(code: i64) -> String {
    match code {
        1 => "cancelled".into(),
        2 => "complete".into(),
        4 | 6 => "open".into(),
        5 => "rejected".into(),
        7 => "expired".into(),
        other => other.to_string(),
    }
}

fn order_feed_exchange(d: &Value) -> String {
    let n = |k: &str| d.get(k).and_then(Value::as_i64).unwrap_or(-1);
    let pair = (n("exchange"), n("segment"));
    let mapped = match pair {
        (12, 12) => Some("BCD"),
        (11, 11) => Some("MCX"),
        (e, s) => exchange_name(e, s),
    };
    match mapped {
        Some(m) => m.to_string(),
        None => {
            let sym = d.get("symbol").and_then(Value::as_str).unwrap_or("");
            match sym.split_once(':') {
                Some((ex, _)) => ex.to_string(),
                None => d
                    .get("exchange")
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default(),
            }
        }
    }
}

fn pick<'a>(d: &'a Value, names: &[&str]) -> Option<&'a Value> {
    names
        .iter()
        .filter_map(|n| d.get(*n))
        .find(|v| !v.is_null())
}

/// web `_unwrap_order_record` + `normalize`.
pub fn parse_order_update(text: &str, symbols: &SymbolResolver) -> Option<OrderUpdate> {
    let msg: Value = serde_json::from_str(text).ok()?;
    if !msg.is_object() {
        return None;
    }
    let status_keys = ["org_ord_status", "status", "ord_status"];
    let is_record = |c: &Value| {
        c.is_object()
            && c.get("id").is_some_and(|v| !v.is_null())
            && pick(c, &status_keys).is_some()
    };
    let d = ["orders", "d", "data"]
        .iter()
        .filter_map(|k| msg.get(*k))
        .chain(std::iter::once(&msg))
        .find(|c| is_record(c))?;
    let num = |names: &[&str]| {
        pick(d, names)
            .map(super::super::mapping::num)
            .unwrap_or(0.0)
    };
    let text_of = |v: Option<&Value>| match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    };
    let raw_status = pick(d, &status_keys);
    let status = match raw_status.and_then(Value::as_i64) {
        Some(c) => order_feed_status(c),
        None => text_of(raw_status),
    };
    let qty = num(&["qty"]) as i64;
    let filled = num(&["qty_filled", "filledQty"]) as i64;
    let exchange = order_feed_exchange(d);
    let br = text_of(pick(d, &["symbol"]));
    let side = pick(d, &["tran_side", "side"]);
    let pt = pick(d, &["ord_type", "type"]);
    Some(OrderUpdate {
        orderid: text_of(pick(d, &["id"])),
        symbol: symbols.oa_symbol_or_raw(&br, &exchange),
        exchange,
        action: match side.and_then(Value::as_i64) {
            Some(1) => "BUY".into(),
            Some(-1) => "SELL".into(),
            _ => text_of(side),
        },
        quantity: qty,
        price: num(&["price_limit", "limitPrice"]),
        trigger_price: num(&["price_stop", "stopPrice"]),
        pricetype: match pt.and_then(Value::as_i64) {
            Some(c @ 1..=4) => super::super::mapping::pricetype_name(c),
            _ => text_of(pt),
        },
        // The web passes the Fyers product code through unchanged here.
        product: text_of(pick(d, &["product_type", "productType"])),
        rejection_reason: if raw_status.and_then(Value::as_i64) == Some(5) {
            text_of(pick(d, &["oms_msg", "message", "status_msg"]))
        } else {
            String::new()
        },
        order_status: status,
        filled_quantity: filled,
        pending_quantity: (qty - filled).max(0),
        average_price: num(&["price_traded", "tradedPrice"]),
    })
}

/// The order-update socket.
pub struct OrderFeed {
    url: String,
    authorization: Secret,
    symbols: SymbolResolver,
}

impl OrderFeed {
    pub fn new(url: &str, auth: &AuthToken, symbols: SymbolResolver) -> Result<Self> {
        auth.pair().ok_or_else(super::super::session_expired)?;
        Ok(Self {
            url: url.to_string(),
            authorization: Secret::new(auth.raw()),
            symbols,
        })
    }
}

impl BrokerFeed for OrderFeed {
    fn broker(&self) -> &'static str {
        "fyers"
    }

    fn ws_request(&self) -> Result<WsRequest> {
        request_with_auth(&self.url, "Authorization", self.authorization.expose())
    }

    fn on_connected(&mut self) -> Vec<Message> {
        vec![Message::Text(
            json!({"T": "SUB_ORD", "SLIST": ["orders"], "SUB_T": 1}).to_string(),
        )]
    }

    fn subscribe_frames(&mut self, _subs: &[FeedSubscription]) -> Vec<Message> {
        Vec::new()
    }

    fn unsubscribe_frames(&mut self, _subs: &[FeedSubscription]) -> Vec<Message> {
        Vec::new()
    }

    fn parse(&mut self, msg: &Message) -> Vec<FeedEvent> {
        match msg {
            Message::Text(t) => parse_order_update(t, &self.symbols)
                .map(|u| vec![FeedEvent::OrderUpdate(u)])
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    fn heartbeat(&self) -> Option<(Duration, Message)> {
        // web base order adapter: protocol ping every 20 s.
        Some((Duration::from_secs(20), Message::Ping(Vec::new())))
    }
}
