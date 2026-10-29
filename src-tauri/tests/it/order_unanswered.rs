//! Web test/test_unanswered_order_writes.py (openalgo#2305): an order sent
//! but never answered is not reported as failed. placeorder,
//! placesmartorder and the legs of basketorder and splitorder answer it
//! with the web's 502 and "it may have been placed, check the order book";
//! the order is never sent twice. An order that never left fails plainly,
//! and an answered order is untouched. The mock's lost answers are real
//! transport failures (`brokers::mock::after_send_error`,
//! `never_sent_error`).

use crate::api_v1_support::H;
use openalgo_desktop_lib::brokers::common::outcome::UNANSWERED_ORDER_MESSAGE;
use openalgo_desktop_lib::brokers::mock::{AfterSend, MockCall};
use serde_json::{json, Value};

fn order() -> Value {
    json!({"strategy": "t", "exchange": "NSE", "symbol": "SBIN", "action": "SELL",
        "quantity": 1, "pricetype": "MARKET", "product": "MIS"})
}

fn unanswered() -> Value {
    json!({"status": "error", "message": UNANSWERED_ORDER_MESSAGE})
}

async fn live() -> H {
    let h = H::new().await;
    h.analyze(false);
    h
}

async fn call(h: &H, path: &str, body: Value) -> (u16, Value) {
    let (s, v) = h.post(path, h.with_key(body)).await;
    (s.as_u16(), v)
}

fn placements(h: &H) -> usize {
    h.mock
        .calls
        .lock()
        .iter()
        .filter(|c| matches!(c, MockCall::PlaceOrder(_)))
        .count()
}

/// test_an_unanswered_order_says_it_may_have_been_placed: dropped after
/// sending, or no answer within the timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unanswered_order_says_it_may_have_been_placed() {
    for kind in [AfterSend::Dropped, AfterSend::TimedOut] {
        let h = live().await;
        h.mock.after_send.lock().push_back((kind, "open"));
        let (s, v) = call(&h, "/api/v1/placeorder", order()).await;
        assert_eq!((s, v), (502, unanswered()));
        assert_eq!(placements(&h), 1, "the order must never be sent twice");
        h.shutdown().await;
    }
}

/// test_an_order_that_never_left_still_fails_plainly.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_order_that_never_left_still_fails_plainly() {
    let h = live().await;
    *h.mock.never_sent.lock() = 1;
    let (s, v) = call(&h, "/api/v1/placeorder", order()).await;
    assert_eq!(s, 500);
    assert_eq!(
        v,
        json!({"status": "error", "message": "Failed to place order due to internal error"})
    );
    h.shutdown().await;
}

/// test_an_answered_order_is_untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_answered_order_is_untouched() {
    let h = live().await;
    h.mock.order_ids.lock().push_back(Ok("1".into()));
    let (s, v) = call(&h, "/api/v1/placeorder", order()).await;
    assert_eq!((s, v), (200, json!({"status": "success", "orderid": "1"})));
    h.shutdown().await;
}

/// test_basket_and_split_legs_say_it_too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn basket_and_split_legs_say_it_too() {
    let h = live().await;
    h.mock
        .after_send
        .lock()
        .push_back((AfterSend::Dropped, "open"));
    let basket = json!({"strategy": "t", "orders": [
        {"exchange": "NSE", "symbol": "SBIN", "action": "SELL", "quantity": 1}]});
    let (s, v) = call(&h, "/api/v1/basketorder", basket).await;
    assert_eq!(s, 200, "{}", v);
    assert_eq!(v["results"][0]["status"], "error");
    assert_eq!(v["results"][0]["message"], UNANSWERED_ORDER_MESSAGE);
    assert_eq!(placements(&h), 1);
    h.shutdown().await;

    let h = live().await;
    h.mock
        .after_send
        .lock()
        .push_back((AfterSend::Dropped, "open"));
    let mut split = order();
    split["quantity"] = json!(2);
    split["splitsize"] = json!(1);
    let (s, v) = call(&h, "/api/v1/splitorder", split).await;
    assert_eq!(s, 200, "{}", v);
    assert_eq!(v["results"][0]["status"], "error");
    assert_eq!(v["results"][0]["message"], UNANSWERED_ORDER_MESSAGE);
    h.shutdown().await;
}

/// test_a_smart_order_says_it_too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_smart_order_says_it_too() {
    let h = live().await;
    h.mock
        .after_send
        .lock()
        .push_back((AfterSend::Dropped, "open"));
    let mut smart = order();
    smart["action"] = json!("BUY");
    smart["position_size"] = json!(1);
    let (s, v) = call(&h, "/api/v1/placesmartorder", smart).await;
    assert_eq!((s, v), (502, unanswered()));
    assert_eq!(placements(&h), 1);
    h.shutdown().await;
}
