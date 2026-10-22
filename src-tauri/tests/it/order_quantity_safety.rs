//! A size is never rounded on its way to the broker or the sandbox. A
//! smart order's fractional `position_size` and a GTT's fractional quantity
//! used to be truncated (a target of 0.5 became 0 and closed the position;
//! a 0.5 GTT became a GTT for 0); they are refused wherever they cannot be
//! carried exactly. Through `/api/v1` with the mock broker, live and in
//! analyzer mode. The live CRYPTO side, where a fraction is carried, is in
//! `broker_deltaexchange`.

use crate::api_v1_support::H;
use openalgo_desktop_lib::brokers::mock::MockCall;
use openalgo_desktop_lib::brokers::types::Position;
use serde_json::{json, Value};

const FRACTIONAL_TARGET: &str =
    "Fractional position size (0.5) is not allowed for non-crypto exchanges.";
const TOO_LARGE: &str = "larger than any exchange accepts";
const FRACTIONAL_REFUSED: &str =
    "Fractional quantities are not supported in OpenAlgo Desktop yet. Use a whole-number quantity.";

fn count(h: &H, wanted: impl Fn(&MockCall) -> bool) -> usize {
    h.mock.calls().iter().filter(|c| wanted(c)).count()
}

/// The orders the broker received, as (side, quantity).
fn placed(h: &H) -> Vec<(String, i64)> {
    h.mock
        .calls()
        .iter()
        .filter_map(|c| match c {
            MockCall::PlaceOrder(o) => Some((o.action.as_str().to_string(), o.quantity)),
            _ => None,
        })
        .collect()
}

async fn post(h: &H, path: &str, body: Value) -> (u16, Value) {
    let (s, v) = h.post(path, h.with_key(body)).await;
    (s.as_u16(), v)
}

// -- smart orders ------------------------------------------------------------

fn smart(position_size: Value) -> Value {
    json!({"strategy": "t", "exchange": "NSE", "symbol": "SBIN", "action": "BUY",
        "quantity": 1, "position_size": position_size, "pricetype": "MARKET", "product": "MIS"})
}

fn sbin_long(qty: i32) -> Position {
    Position {
        symbol: "SBIN".into(),
        exchange: "NSE".into(),
        product: "MIS".into(),
        quantity: qty,
        overnight_quantity: 0,
        average_price: 800.0,
        ltp: 801.0,
        pnl: 0.0,
        realized_pnl: 0.0,
        unrealized_pnl: 0.0,
        buy_quantity: qty,
        buy_value: 0.0,
        sell_quantity: 0,
        sell_value: 0.0,
    }
}

/// The SBIN MIS position, as `/api/v1/openposition` reports it.
async fn sbin_held(h: &H) -> Value {
    let ask = json!({"strategy": "t", "symbol": "SBIN", "exchange": "NSE", "product": "MIS"});
    post(h, "/api/v1/openposition", ask).await.1["quantity"].clone()
}

/// Live, holding 6: a target of 0.5 is refused with the web's validation
/// shape before the position is read; it used to be read as 0 and sold all
/// 6. A target no exchange takes is refused too (it used to buy
/// 2,147,483,642). A whole target still trades the difference.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fractional_target_position_is_refused_live() {
    let h = H::new().await;
    h.analyze(false);
    *h.mock.positions.lock() = Some(Ok(vec![sbin_long(6)]));
    for (target, why) in [
        (json!(0.5), FRACTIONAL_TARGET),
        (json!(2_147_483_648u64), TOO_LARGE),
    ] {
        let (s, v) = post(&h, "/api/v1/placesmartorder", smart(target.clone())).await;
        assert_eq!(s, 400, "target {} -> {}", target, v);
        assert!(v.to_string().contains(why), "target {} -> {}", target, v);
    }
    assert!(placed(&h).is_empty(), "no order reached the broker");
    assert_eq!(
        count(&h, |c| matches!(
            c,
            MockCall::OpenPosition(..) | MockCall::Positions
        )),
        0,
        "refused before the position was read"
    );

    let (s, v) = post(&h, "/api/v1/placesmartorder", smart(json!(5))).await;
    assert_eq!(s, 200, "{}", v);
    assert_eq!(placed(&h), [("SELL".to_string(), 1)]);
    h.shutdown().await;
}

/// In the sandbox, holding 6: a target of 0.5 is refused and the position
/// stays 6; it used to be read as 0 and closed the position.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fractional_target_position_is_refused_in_the_sandbox() {
    let h = H::new().await;
    h.analyze(true);
    let mut open = smart(json!(6));
    if let Some(m) = open.as_object_mut() {
        m.remove("position_size");
        m.insert("quantity".into(), json!(6));
    }
    let (s, v) = post(&h, "/api/v1/placeorder", open).await;
    assert_eq!(s, 200, "{}", v);
    assert_eq!(sbin_held(&h).await, json!(6));

    let (s, v) = post(&h, "/api/v1/placesmartorder", smart(json!(0.5))).await;
    assert_eq!(s, 400, "{}", v);
    assert!(v.to_string().contains(FRACTIONAL_TARGET), "{}", v);
    assert_eq!(sbin_held(&h).await, json!(6), "the position is untouched");

    let (s, v) = post(&h, "/api/v1/placesmartorder", smart(json!(5))).await;
    assert_eq!(s, 200, "{}", v);
    assert_eq!(sbin_held(&h).await, json!(5));
    assert!(placed(&h).is_empty(), "nothing reached the broker");
    h.shutdown().await;
}

// -- GTT ---------------------------------------------------------------------

fn gtt(quantity: Value) -> Value {
    json!({"strategy": "t", "trigger_type": "SINGLE", "exchange": "CRYPTO", "symbol": "BTCUSDFUT",
        "action": "BUY", "product": "NRML", "quantity": quantity, "pricetype": "LIMIT",
        "price": 50000.0, "triggerprice_sl": 50000.0})
}

fn gtt_calls(h: &H) -> usize {
    count(h, |c| {
        matches!(c, MockCall::PlaceGtt | MockCall::ModifyGtt(_))
    })
}

/// Live: a fractional GTT quantity is refused, placed or modified, and
/// nothing reaches the broker; it used to arrive as a GTT for 0.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fractional_gtt_never_reaches_the_broker() {
    let h = H::new().await;
    h.analyze(false);
    let (s, v) = post(&h, "/api/v1/placegttorder", gtt(json!(0.5))).await;
    assert_eq!(
        (s, v["message"].as_str()),
        (400, Some(FRACTIONAL_REFUSED)),
        "{}",
        v
    );
    let mut modify = gtt(json!(0.5));
    modify["trigger_id"] = json!("GTT-1");
    let (s, v) = post(&h, "/api/v1/modifygttorder", modify).await;
    assert_eq!(
        (s, v["message"].as_str()),
        (400, Some(FRACTIONAL_REFUSED)),
        "{}",
        v
    );
    assert_eq!(gtt_calls(&h), 0, "no GTT reached the broker");

    let (s, v) = post(&h, "/api/v1/placegttorder", gtt(json!(2))).await;
    assert_eq!(s, 200, "{}", v);
    assert_eq!(gtt_calls(&h), 1);
    h.shutdown().await;
}

// -- options splits ----------------------------------------------------------

/// 195 NIFTY ATM calls in three chunks of 65.
fn options_split() -> Value {
    json!({"strategy": "t", "underlying": "NIFTY", "exchange": "NSE_INDEX",
        "expiry_date": "06OCT26", "offset": "ATM", "option_type": "CE", "action": "BUY",
        "quantity": 195, "splitsize": 65, "product": "NRML"})
}

fn messages(rows: &Value) -> Vec<&str> {
    rows.as_array()
        .map(|r| {
            r.iter()
                .map(|x| x["message"].as_str().unwrap_or("sent"))
                .collect()
        })
        .unwrap_or_default()
}

/// LOG-08 P1-5 for options: after a chunk whose answer was lost (it may be
/// at the broker) the rest are held back and answered as `splitorder`
/// answers them; they used to be sent. The reply carries the unknown
/// outcome. A multi-order leg in chunks holds back the same way.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_options_split_holds_back_after_a_chunk_with_no_definite_answer() {
    use openalgo_desktop_lib::brokers::mock::AfterSend;
    use openalgo_desktop_lib::services::batch_order_service::SPLIT_HELD;
    use openalgo_desktop_lib::services::{options_order_service as options, Route};
    let h = H::new().await;
    h.analyze(false);
    h.mock
        .after_send
        .lock()
        .push_back((AfterSend::Dropped, "open"));
    let r = options::options_order(&h.ctx, &h.with_key(options_split()), Route::API).await;
    assert_eq!(
        placed(&h).len(),
        1,
        "nothing after the lost chunk: {}",
        r.body
    );
    let rows = &r.body["results"];
    assert_eq!(rows[0]["status"], "error", "{}", r.body);
    assert_eq!(messages(rows)[1..], [SPLIT_HELD, SPLIT_HELD], "{}", r.body);
    assert!(r.is_uncertain(), "the reply carries the unknown outcome");

    h.mock
        .after_send
        .lock()
        .push_back((AfterSend::Dropped, "open"));
    let multi = json!({"strategy": "t", "underlying": "NIFTY", "exchange": "NSE_INDEX",
        "expiry_date": "06OCT26", "legs": [{"offset": "ATM", "option_type": "CE",
        "action": "BUY", "quantity": 195, "splitsize": 65, "product": "NRML"}]});
    let r = options::options_multi_order(&h.ctx, &h.with_key(multi), Route::API).await;
    assert_eq!(placed(&h).len(), 2, "one more chunk only: {}", r.body);
    let rows = &r.body["results"][0]["split_results"];
    assert_eq!(messages(rows)[1..], [SPLIT_HELD, SPLIT_HELD], "{}", r.body);
    h.shutdown().await;
}

/// In analyzer mode every chunk goes to the sandbox, whose answer is always
/// definite, and none to the broker.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_options_split_in_the_sandbox_places_every_chunk() {
    let h = H::new().await;
    h.analyze(true);
    let (s, v) = post(&h, "/api/v1/optionsorder", options_split()).await;
    assert_eq!((s, &v["mode"]), (200, &json!("analyze")), "{}", v);
    assert_eq!(messages(&v["results"]), ["sent", "sent", "sent"], "{}", v);
    let (_, book) = post(&h, "/api/v1/orderbook", json!({})).await;
    assert_eq!(book["data"]["orders"].as_array().map(Vec::len), Some(3));
    assert!(placed(&h).is_empty(), "nothing reached the broker");
    h.shutdown().await;
}
