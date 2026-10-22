//! PROD-TR-04: one options order goes to one side. `optionsorder` and
//! `optionsmultiorder` read analyzer mode once, but each of their orders
//! read it again, so the trader's toggle while a contract was resolved or a
//! chunk was sent moved the rest to the other side, while the reply and its
//! event still named the first. The Semi-Auto order mode was read again the
//! same way, queueing the rest of an order already sent in part.

use crate::api_v1_support::H;
use openalgo_desktop_lib::brokers::mock::{MockCall, PlaceGate};
use openalgo_desktop_lib::db::sqlite::action_center;
use openalgo_desktop_lib::events::{Mode, Topic};
use openalgo_desktop_lib::services::apikey_service::ApiKeyService;
use serde_json::{json, Value};
use std::sync::Arc;

/// 195 NIFTY ATM calls in three chunks of 65.
fn options_split() -> Value {
    json!({"strategy": "t", "underlying": "NIFTY", "exchange": "NSE_INDEX",
        "expiry_date": "06OCT26", "offset": "ATM", "option_type": "CE", "action": "BUY",
        "quantity": 195, "splitsize": 65, "product": "NRML"})
}

/// A call bought whole and a put sold in two chunks: three orders.
fn multi_order() -> Value {
    json!({"strategy": "t", "underlying": "NIFTY", "exchange": "NSE_INDEX",
        "expiry_date": "06OCT26", "legs": [
            {"offset": "ATM", "option_type": "CE", "action": "BUY", "quantity": 65,
             "product": "NRML"},
            {"offset": "ATM", "option_type": "PE", "action": "SELL", "quantity": 130,
             "splitsize": 65, "product": "NRML"}]})
}

fn placed(h: &H) -> usize {
    h.mock
        .calls()
        .iter()
        .filter(|c| matches!(c, MockCall::PlaceOrder(_)))
        .count()
}

/// The sandbox's orders, read without touching the toggle.
async fn sandbox_orders(h: &H) -> usize {
    let book = serde_json::to_value(h.ctx.sandbox.orderbook().await.unwrap()).unwrap();
    book["data"]["orders"].as_array().map_or(0, Vec::len)
}

async fn post(h: &H, path: &str, body: Value) -> (u16, Value) {
    let (s, v) = h.post(path, h.with_key(body)).await;
    (s.as_u16(), v)
}

/// Every order of the request, its reply and every event it published are
/// on the side it started on.
async fn all_on(h: &H, sandbox: bool, reply: &Value, orders: usize, done: Topic) {
    let side = if sandbox { "Sandbox" } else { "Live" };
    assert_eq!(
        placed(h),
        if sandbox { 0 } else { orders },
        "{side}: {reply}"
    );
    assert_eq!(
        sandbox_orders(h).await,
        if sandbox { orders } else { 0 },
        "{side}: {reply}"
    );
    assert_eq!(
        reply.get("mode").and_then(Value::as_str),
        sandbox.then_some("analyze"),
        "{side}: {reply}"
    );
    for _ in 0..200 {
        if h.recorder.topics().contains(&done.as_str()) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(h.recorder.topics().contains(&done.as_str()), "{side}");
    let mode = if sandbox { Mode::Analyze } else { Mode::Live };
    let events = h.recorder.events.lock().clone();
    for e in events.iter().filter_map(|e| e.meta()) {
        assert_eq!(e.mode, mode, "{side}: {} {}", e.api_type, e.response_data);
    }
}

/// The toggle flips while the contract is resolved (the underlying's quote
/// is held), in each direction: every order still goes to the side the
/// request started on, for `optionsorder` and `optionsmultiorder` alike.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_options_order_stays_on_its_side_when_the_toggle_flips_while_it_resolves() {
    for (path, body, done) in [
        (
            "/api/v1/optionsorder",
            options_split(),
            Topic::OptionsCompleted,
        ),
        (
            "/api/v1/optionsmultiorder",
            multi_order(),
            Topic::MultiOrderCompleted,
        ),
    ] {
        for sandbox in [false, true] {
            let h = H::new().await;
            h.analyze(sandbox);
            let gate = Arc::new(PlaceGate::default());
            *h.mock.quote_gate.lock() = Some(gate.clone());
            let toggle = async {
                gate.entered.notified().await;
                h.analyze(!sandbox);
                gate.release.notify_one();
            };
            let ((s, v), ()) = tokio::join!(post(&h, path, body.clone()), toggle);
            assert_eq!(s, 200, "{path} {v}");
            all_on(&h, sandbox, &v, 3, done).await;
            h.shutdown().await;
        }
    }
}

/// A live split whose first chunk is in flight when analyzer mode is
/// turned on sends its other chunks live too, none to the sandbox.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_live_options_split_stays_live_when_analyzer_mode_turns_on_between_chunks() {
    let h = H::new().await;
    h.analyze(false);
    let gate = Arc::new(PlaceGate::default());
    *h.mock.place_gate.lock() = Some(gate.clone());
    let toggle = async {
        gate.entered.notified().await;
        h.analyze(true);
        *h.mock.place_gate.lock() = None;
        gate.release.notify_one();
    };
    let ((s, v), ()) = tokio::join!(post(&h, "/api/v1/optionsorder", options_split()), toggle);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["results"].as_array().map(Vec::len), Some(3), "{v}");
    assert!(
        v["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["status"] == "success" && r["orderid"].is_string()),
        "{v}"
    );
    all_on(&h, false, &v, 3, Topic::OptionsCompleted).await;
    h.shutdown().await;
}

/// The order mode turns to Semi-Auto while the first chunk is in flight:
/// the request was admitted as a whole, so its other chunks are sent too,
/// and none is queued in the Action Center on its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_live_options_split_is_not_queued_in_part_when_semi_auto_turns_on() {
    let h = H::new().await;
    h.analyze(false);
    let gate = Arc::new(PlaceGate::default());
    *h.mock.place_gate.lock() = Some(gate.clone());
    let toggle = async {
        gate.entered.notified().await;
        ApiKeyService::set_order_mode(&h.ctx, "semi_auto").unwrap();
        *h.mock.place_gate.lock() = None;
        gate.release.notify_one();
    };
    let ((s, v), ()) = tokio::join!(post(&h, "/api/v1/optionsorder", options_split()), toggle);
    assert_eq!(s, 200, "{v}");
    assert_eq!(placed(&h), 3, "{v}");
    let queued = action_center::pending_count(&h.ctx.sqlite.conn().unwrap(), "trader").unwrap();
    assert_eq!(queued, 0, "nothing queued in part: {v}");
    h.shutdown().await;
}
