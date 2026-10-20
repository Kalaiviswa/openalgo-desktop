//! Market data received and what its readers missed (NEW-REL-05), for
//! health (`/health/api/current`, `market_data`) and Run diagnostics.
//!
//! Per broker feed, since its current connection began: the updates
//! received and, per reader, the updates it skipped because it had fallen
//! behind the feed's channel. For the apps on the market data feed (port
//! 8765), since OpenAlgo started: frames a slow app's queue folded into a
//! newer one for the same instrument (it gets the latest price, by
//! design), frames dropped past its queue's cap, and updates the feed
//! server skipped. The strategy module's own folded and missed prices are
//! in `strategy_ticks`.

use crate::feed::MarketDataSource;
use crate::state::AppState;
use crate::websocket::{Reader, SinceConnect, WebSocketManager};
use serde_json::{json, Map, Value};
use std::sync::atomic::Ordering;

/// The broker feeds health reports: the market data feed always, the order
/// update socket while it runs (other brokers relay order updates on the
/// market feed or have the order book read).
fn feeds(ctx: &AppState) -> Vec<(&'static str, &WebSocketManager)> {
    let mut out = vec![("market_data", ctx.websocket.as_ref())];
    if ctx.runtime.order_ws.is_running() {
        out.push(("order_updates", ctx.runtime.order_ws.as_ref()));
    }
    out
}

fn feed_json(name: &str, feed: &WebSocketManager) -> Value {
    let s = feed.stats().since_connect();
    let connected = feed.is_connected();
    let skipped: Map<String, Value> = s
        .skipped
        .iter()
        .map(|(r, n)| (r.name().to_string(), json!(n)))
        .collect();
    json!({
        "name": name,
        "connected": connected,
        "connected_for_s": s.connected_for.filter(|_| connected).map(|d| d.as_secs()),
        "last_update_age_s": feed.stats().last_event_age().map(|d| d.as_secs()),
        "received_since_connect": s.received,
        "skipped_since_connect": skipped,
        "skipped_total_since_connect": s.skipped_total(),
    })
}

/// The `market_data` health block; `status` is `warn` while anything was
/// skipped since a feed connected, or apps' frames were dropped or skipped.
pub fn json(ctx: &AppState) -> Value {
    let broker_feeds: Vec<Value> = feeds(ctx).iter().map(|(n, f)| feed_json(n, f)).collect();
    let feed_skips = broker_feeds
        .iter()
        .any(|f| f["skipped_total_since_connect"].as_u64().unwrap_or(0) > 0);
    let (coalesced, dropped, lagged) = match ctx.bridge.client_losses() {
        Some(l) => (
            l.coalesced.load(Ordering::Relaxed),
            l.dropped.load(Ordering::Relaxed),
            l.lagged.load(Ordering::Relaxed),
        ),
        None => (0, 0, 0),
    };
    let warn = feed_skips || dropped > 0 || lagged > 0;
    json!({
        "status": if warn { "warn" } else { "pass" },
        "broker_feeds": broker_feeds,
        "feed_clients": {"coalesced": coalesced, "dropped": dropped, "skipped": lagged},
    })
}

/// What a trader reads about a reader that fell behind.
fn reader_words(r: Reader) -> &'static str {
    match r {
        Reader::Strategy => "strategies",
        Reader::Scalping => "scalping stops",
        Reader::Sandbox => "sandbox orders",
        Reader::FeedClients => "apps on the market data feed",
        Reader::OrderRelay => "order updates",
    }
}

/// The diagnostics sentence about a connected feed's updates since it
/// connected; empty before the first.
pub fn sentence(s: &SinceConnect) -> String {
    if s.received == 0 {
        return String::new();
    }
    let updates = if s.received == 1 { "update" } else { "updates" };
    let skipped = s.skipped_total();
    if skipped == 0 {
        return format!(
            "{} {} since it connected, none missed.",
            s.received, updates
        );
    }
    let who: Vec<String> = s
        .skipped
        .iter()
        .filter(|(_, n)| *n > 0)
        .map(|(r, n)| format!("{} {}", reader_words(*r), n))
        .collect();
    format!(
        "{} {} since it connected; {} reached a reader that had fallen behind and were skipped ({}). Each acts on the next update.",
        s.received,
        updates,
        skipped,
        who.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn since(received: u64, skipped: &[(Reader, u64)]) -> SinceConnect {
        SinceConnect {
            connected_for: Some(Duration::from_secs(60)),
            received,
            skipped: Reader::ALL
                .iter()
                .map(|r| {
                    let n = skipped.iter().find(|(x, _)| x == r).map(|(_, n)| *n);
                    (*r, n.unwrap_or(0))
                })
                .collect(),
        }
    }

    #[test]
    fn the_sentence_names_who_skipped_and_how_many() {
        assert_eq!(sentence(&since(0, &[])), "");
        assert_eq!(
            sentence(&since(12_345, &[])),
            "12345 updates since it connected, none missed."
        );
        assert_eq!(
            sentence(&since(
                500,
                &[(Reader::Scalping, 30), (Reader::FeedClients, 10)]
            )),
            "500 updates since it connected; 40 reached a reader that had fallen behind and \
             were skipped (scalping stops 30, apps on the market data feed 10). Each acts on \
             the next update."
        );
    }
}
