//! PROD-TR-02: an options order that may have been placed says so. A
//! placement whose answer was lost may be at the broker (LOG-08): an
//! options split holds back what follows it, and the reply of
//! `optionsorder` and `optionsmultiorder` carries that outcome to the
//! Action Center, which keeps the approved order unconfirmed, across a
//! restart, instead of recording it as refused or partly refused.

use crate::api_v1_support::H;
use openalgo_desktop_lib::brokers::mock::{AfterSend, MockCall, PlaceGate};
use openalgo_desktop_lib::db::sqlite::action_center;
use openalgo_desktop_lib::services::action_center_service as center;
use openalgo_desktop_lib::services::apikey_service::ApiKeyService;
use openalgo_desktop_lib::services::batch_order_service::SPLIT_HELD;
use openalgo_desktop_lib::services::{options_order_service as options, Route};
use serde_json::{json, Value};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

/// 195 NIFTY ATM calls in three chunks of 65.
fn options_split() -> Value {
    json!({"strategy": "t", "underlying": "NIFTY", "exchange": "NSE_INDEX",
        "expiry_date": "06OCT26", "offset": "ATM", "option_type": "CE", "action": "BUY",
        "quantity": 195, "splitsize": 65, "product": "NRML"})
}

fn multi_order(legs: Value) -> Value {
    json!({"strategy": "t", "underlying": "NIFTY", "exchange": "NSE_INDEX",
        "expiry_date": "06OCT26", "legs": legs})
}

fn leg(option_type: &str, action: &str, quantity: i64, splitsize: i64) -> Value {
    json!({"offset": "ATM", "option_type": option_type, "action": action,
        "quantity": quantity, "splitsize": splitsize, "product": "NRML"})
}

fn placed(h: &H) -> usize {
    h.mock
        .calls()
        .iter()
        .filter(|c| matches!(c, MockCall::PlaceOrder(_)))
        .count()
}

/// Each row's message, "sent" for a placed one.
fn messages(rows: &Value) -> Vec<&str> {
    rows.as_array()
        .map(|r| {
            r.iter()
                .map(|x| x["message"].as_str().unwrap_or("sent"))
                .collect()
        })
        .unwrap_or_default()
}

/// Run `request` while the broker takes `calls` placements one at a time
/// and loses its answer to placement `lost` (from 1): that order is at the
/// broker, its answer never arrives. Placements beyond `calls` are not
/// held, so a request that sends more is counted, not stuck.
async fn losing_answer<F: Future>(h: &H, lost: usize, calls: usize, request: F) -> F::Output {
    let gate = Arc::new(PlaceGate::default());
    *h.mock.place_gate.lock() = Some(gate.clone());
    let broker = async {
        for n in 1..=calls {
            gate.entered.notified().await;
            if n == lost {
                h.mock
                    .after_send
                    .lock()
                    .push_back((AfterSend::Dropped, "open"));
            }
            if n == calls {
                *h.mock.place_gate.lock() = None;
            }
            gate.release.notify_one();
        }
    };
    let both = async { tokio::join!(request, broker).0 };
    tokio::time::timeout(Duration::from_secs(60), both)
        .await
        .expect("the request did not make the placements expected")
}

/// A split whose first, middle or last chunk has no definite answer: the
/// chunks before it are placed, the rest held back unsent, and the reply
/// carries "may have been placed". Exactly one broker call per chunk up to
/// the lost one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_options_split_holds_after_a_lost_first_middle_or_last_chunk() {
    for lost in 1..=3 {
        let h = H::new().await;
        h.analyze(false);
        let req = h.with_key(options_split());
        let r = losing_answer(
            &h,
            lost,
            lost,
            options::options_order(&h.ctx, &req, Route::API),
        )
        .await;
        assert_eq!(placed(&h), lost, "chunk {lost}: {}", r.body);
        let rows = &r.body["results"];
        let mut want = vec!["sent"; lost - 1];
        want.push(rows[lost - 1]["message"].as_str().unwrap_or("sent"));
        want.extend(vec![SPLIT_HELD; 3 - lost]);
        assert_eq!(messages(rows), want, "chunk {lost}: {}", r.body);
        assert_eq!(
            rows[lost - 1]["status"],
            "error",
            "chunk {lost}: {}",
            r.body
        );
        assert!(
            r.is_uncertain(),
            "chunk {lost}: the reply says it may be placed"
        );
        h.shutdown().await;
    }
}

/// A multi-leg order whose second leg's answer is lost: the first leg is
/// placed, the second may be, and the reply says so (it used to read as a
/// plain partial failure). Two broker calls, no more.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_multi_leg_order_with_a_lost_leg_says_it_may_have_been_placed() {
    let h = H::new().await;
    h.analyze(false);
    let req = h.with_key(multi_order(json!([
        leg("CE", "BUY", 65, 0),
        leg("PE", "SELL", 65, 0)
    ])));
    let r = losing_answer(
        &h,
        2,
        2,
        options::options_multi_order(&h.ctx, &req, Route::API),
    )
    .await;
    assert_eq!(placed(&h), 2, "{}", r.body);
    assert_eq!(r.status, 200, "{}", r.body);
    let rows = &r.body["results"];
    assert_eq!(rows[0]["status"], "success", "{}", r.body);
    assert_eq!(rows[1]["status"], "error", "{}", r.body);
    assert!(r.is_uncertain(), "the reply says leg 2 may be placed");
    let why = format!("{:?}", r.placement);
    assert!(why.contains("leg 2"), "{why}");
    h.shutdown().await;
}

/// A multi-leg order whose split leg loses the answer to its middle chunk:
/// its last chunk is held back, the other leg is still placed, and the
/// reply says the split leg may have been placed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_multi_leg_order_with_a_lost_chunk_says_it_may_have_been_placed() {
    let h = H::new().await;
    h.analyze(false);
    let req = h.with_key(multi_order(json!([
        leg("CE", "BUY", 195, 65),
        leg("PE", "SELL", 65, 0)
    ])));
    let r = losing_answer(
        &h,
        2,
        3,
        options::options_multi_order(&h.ctx, &req, Route::API),
    )
    .await;
    assert_eq!(placed(&h), 3, "two chunks and the other leg: {}", r.body);
    let rows = &r.body["results"];
    let chunks = messages(&rows[0]["split_results"]);
    assert_eq!((chunks[0], chunks[2]), ("sent", SPLIT_HELD), "{}", r.body);
    assert_eq!(rows[1]["status"], "success", "{}", r.body);
    assert!(r.is_uncertain(), "the reply says leg 1 may be placed");
    let why = format!("{:?}", r.placement);
    assert!(why.contains("leg 1"), "{why}");
    h.shutdown().await;
}

/// An approved multi-leg order whose second leg's answer was lost stays
/// unconfirmed (it used to be recorded as partly placed), under the first
/// leg's order id; after a restart it is still unconfirmed and approving or
/// executing it again sends nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_approved_multi_leg_order_with_a_lost_leg_stays_unconfirmed_across_a_restart() {
    let h = H::new().await;
    h.analyze(false);
    ApiKeyService::set_order_mode(&h.ctx, "semi_auto").unwrap();
    let body = multi_order(json!([leg("CE", "BUY", 65, 0), leg("PE", "SELL", 65, 0)]));
    let (s, v) = h.post("/api/v1/optionsmultiorder", h.with_key(body)).await;
    assert_eq!((s.as_u16(), &v["mode"]), (200, &json!("semi_auto")), "{v}");
    let id = v["pending_order_id"].as_i64().unwrap();
    assert_eq!(placed(&h), 0, "queued, not sent");

    let answer = losing_answer(&h, 2, 2, center::approve(&h.ctx, id, "trader")).await;
    assert_eq!(placed(&h), 2, "{}", answer.body);
    let row = action_center::get(&h.ctx.sqlite.conn().unwrap(), id)
        .unwrap()
        .unwrap();
    assert!(center::is_unconfirmed(&row), "{row:?}");
    assert!(
        row.broker_order_id
            .as_deref()
            .is_some_and(|o| !o.is_empty()),
        "the first leg's order id is kept: {row:?}"
    );

    let h = h.restart().await;
    let row = action_center::get(&h.ctx.sqlite.conn().unwrap(), id)
        .unwrap()
        .unwrap();
    assert!(center::is_unconfirmed(&row), "after the restart: {row:?}");
    let again = center::execute_approved(&h.ctx, id).await;
    assert_eq!(
        again.status,
        center::ALREADY_SUBMITTING_STATUS,
        "{:?}",
        again
    );
    let again = center::approve(&h.ctx, id, "trader").await;
    assert_eq!(again.status, 409, "{}", again.body);
    assert_eq!(placed(&h), 0, "nothing sent again after the restart");
    h.shutdown().await;
}
