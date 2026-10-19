//! The live placement hands the request's `strategy` to the adapter (web
//! `place_order_api(data, ...)` passes the whole request, and Rupeezy's
//! `transform_data` sends `data["strategy"]` as `order_identifier`), on
//! every path that places a live order: an order, a basket leg, a smart
//! order. A sandbox order never reaches the broker.

use crate::api_v1_support::H;
use serde_json::json;

fn order(strategy: &str) -> serde_json::Value {
    json!({"strategy": strategy, "exchange": "NSE", "symbol": "SBIN", "action": "BUY",
        "quantity": 1, "pricetype": "MARKET", "product": "MIS"})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_live_order_reaches_the_broker_with_its_strategy() {
    let h = H::new().await;
    h.analyze(false);
    let (s, v) = h
        .post("/api/v1/placeorder", h.with_key(order("Momentum")))
        .await;
    assert_eq!(s.as_u16(), 200, "{}", v);
    let leg = json!({"exchange": "NSE", "symbol": "SBIN", "action": "SELL",
        "quantity": 1, "pricetype": "MARKET", "product": "MIS"});
    let basket = json!({"strategy": "Basket A", "orders": [leg]});
    let (s, v) = h.post("/api/v1/basketorder", h.with_key(basket)).await;
    assert_eq!(s.as_u16(), 200, "{}", v);
    let mut smart = order("Smart B");
    smart["position_size"] = json!(1);
    let (s, v) = h.post("/api/v1/placesmartorder", h.with_key(smart)).await;
    assert_eq!(s.as_u16(), 200, "{}", v);
    assert_eq!(
        h.mock.strategies.lock().clone(),
        ["Momentum", "Basket A", "Smart B"]
    );
    h.analyze(true);
    let (s, v) = h
        .post("/api/v1/placeorder", h.with_key(order("Sandboxed")))
        .await;
    assert_eq!(s.as_u16(), 200, "{}", v);
    assert_eq!(h.mock.strategies.lock().len(), 3);
}
