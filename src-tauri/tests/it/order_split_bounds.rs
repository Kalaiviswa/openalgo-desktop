//! PROD-TR-03: a split is counted before anything is allocated. A split of
//! 2147483647 in chunks of 1 is schema-valid, and the desktop used to build
//! all its chunks (16 GiB) before checking the 100-order limit. Every path
//! here runs under a cap on the largest single allocation
//! (`alloc_support::capped`): a regression aborts the test instead of
//! swapping the machine.

use crate::alloc_support::{capped, capped_sync};
use crate::api_v1_support::H;
use openalgo_desktop_lib::brokers::mock::MockCall;
use openalgo_desktop_lib::services::batch_order_service::{self as batch, split_chunks};
use openalgo_desktop_lib::services::Route;
use serde_json::{json, Value};

/// No request here needs a single allocation anywhere near this.
const CAP: usize = 64 << 20;
/// What a refused split may allocate at most in one request.
const SMALL: usize = 4 << 20;
const TOO_MANY: &str = "Total number of orders would exceed maximum limit of 100";
const TOO_MANY_PER_LEG: &str = "Split orders would exceed maximum limit of 100 per leg";

fn placed(h: &H) -> Vec<i64> {
    h.mock
        .calls()
        .iter()
        .filter_map(|c| match c {
            MockCall::PlaceOrder(o) => Some(o.quantity),
            _ => None,
        })
        .collect()
}

async fn post(h: &H, path: &str, body: Value) -> (u16, Value) {
    let (s, v) = h.post(path, h.with_key(body)).await;
    (s.as_u16(), v)
}

async fn sandbox_orders(h: &H) -> usize {
    h.analyze(true);
    let (_, book) = post(h, "/api/v1/orderbook", json!({})).await;
    book["data"]["orders"].as_array().map_or(0, Vec::len)
}

fn split(quantity: i64, splitsize: i64) -> Value {
    json!({"strategy": "t", "exchange": "NSE", "symbol": "SBIN", "action": "BUY",
        "quantity": quantity, "splitsize": splitsize, "pricetype": "MARKET", "product": "MIS"})
}

fn options(quantity: i64, splitsize: i64) -> Value {
    json!({"strategy": "t", "underlying": "NIFTY", "exchange": "NSE_INDEX",
        "expiry_date": "06OCT26", "offset": "ATM", "option_type": "CE", "action": "BUY",
        "quantity": quantity, "splitsize": splitsize, "product": "NRML"})
}

fn multi(quantity: i64, splitsize: i64) -> Value {
    json!({"strategy": "t", "underlying": "NIFTY", "exchange": "NSE_INDEX",
        "expiry_date": "06OCT26", "legs": [{"offset": "ATM", "option_type": "CE",
        "action": "BUY", "quantity": quantity, "splitsize": splitsize, "product": "NRML"}]})
}

/// Through `/api/v1`, live and in analyzer mode: `splitorder`,
/// `optionsorder` and an `optionsmultiorder` leg of 2147483647 in chunks of
/// 1 (and the 101-chunk control) are refused with the web's messages,
/// allocating almost nothing, and nothing is sent or simulated.
#[tokio::test]
async fn a_split_beyond_the_limit_is_refused_before_its_chunks_are_built() {
    let h = H::new().await;
    // The key is checked once outside the cap.
    let _ = post(&h, "/api/v1/funds", json!({})).await;
    for analyze in [false, true] {
        h.analyze(analyze);
        for quantity in [i64::from(i32::MAX), 101] {
            let (answers, largest) = capped(CAP, async {
                (
                    post(&h, "/api/v1/splitorder", split(quantity, 1)).await,
                    post(&h, "/api/v1/optionsorder", options(quantity, 1)).await,
                    post(&h, "/api/v1/optionsmultiorder", multi(quantity, 1)).await,
                )
            })
            .await;
            let ((s, v), (os, ov), (ms, mv)) = answers;
            let at = format!("analyze {} quantity {}", analyze, quantity);
            assert_eq!(
                (s, v["message"].as_str()),
                (400, Some(TOO_MANY)),
                "{at} {v}"
            );
            assert_eq!(
                (os, ov["message"].as_str()),
                (400, Some(TOO_MANY)),
                "{at} {ov}"
            );
            assert_eq!(ms, 200, "{at} {mv}");
            assert_eq!(
                mv["results"][0]["message"].as_str(),
                Some(TOO_MANY_PER_LEG),
                "{at} {mv}"
            );
            assert!(largest < SMALL, "{at}: {largest} bytes in one allocation");
        }
    }
    assert!(placed(&h).is_empty(), "nothing reached the broker");
    assert_eq!(sandbox_orders(&h).await, 0, "nothing in the sandbox");
    h.shutdown().await;
}

/// The split helper counts before it allocates: what it cannot split into
/// at most 100 chunks (the largest, the widest and a negative total, which
/// used to wrap into a length of 2^64 - 1) is refused allocating nothing,
/// and the valid ends of the range, one chunk and the full 100 with and
/// without a remainder, keep their exact sizes.
#[test]
fn the_split_helper_counts_before_it_allocates() {
    let (refused, largest) = capped_sync(CAP, || {
        [
            (i64::from(i32::MAX), 1),
            (i64::MAX, 1),
            (101, 1),
            (-1, 1),
            (5, 0),
            (5, -1),
        ]
        .map(|(total, size)| split_chunks(total, size))
    });
    assert!(refused.iter().all(Option::is_none), "{refused:?}");
    assert!(largest < 1024, "{largest} bytes in one allocation");

    assert_eq!(split_chunks(7, 10), Some(vec![7]));
    assert_eq!(split_chunks(0, 10), Some(vec![]));
    for (total, size) in [(1000, 10), (995, 10), (i64::from(i32::MAX), 21_474_837)] {
        let chunks = split_chunks(total, size).unwrap();
        assert_eq!(chunks.len(), 100, "{total}/{size}");
        assert_eq!(chunks.iter().sum::<i64>(), total, "{total}/{size}");
        assert!(chunks[..99].iter().all(|c| *c == size), "{total}/{size}");
    }
}

/// The split's direct caller (the scalping exit's split, which has no
/// schema in front of it) refuses a split beyond the limit, or of a
/// negative total, before building its chunks, live and in the sandbox.
#[tokio::test]
async fn a_direct_split_beyond_the_limit_is_refused_before_its_chunks_are_built() {
    let h = H::new().await;
    for analyze in [false, true] {
        for quantity in [i64::from(i32::MAX), -5] {
            let req = h.with_key(split(quantity, 1));
            let (r, largest) = capped(
                CAP,
                batch::split_order_decided(&h.ctx, &req, Route::INTERNAL, analyze),
            )
            .await;
            let at = format!("analyze {} quantity {}", analyze, quantity);
            assert_eq!((r.status, r.message().as_str()), (400, TOO_MANY), "{at}");
            assert!(largest < SMALL, "{at}: {largest} bytes in one allocation");
        }
    }
    assert!(placed(&h).is_empty(), "nothing reached the broker");
    assert_eq!(sandbox_orders(&h).await, 0, "nothing in the sandbox");
    h.shutdown().await;
}

/// Through `/api/v1`, the valid ends of the range keep their exact sizes:
/// one chunk live, and the full 100 with a remainder chunk in the sandbox
/// (no pause between its orders).
#[tokio::test]
async fn a_split_of_one_or_a_hundred_chunks_keeps_its_exact_sizes() {
    let h = H::new().await;
    h.analyze(false);
    let (s, v) = post(&h, "/api/v1/splitorder", split(7, 10)).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(placed(&h), [7], "one chunk of the whole quantity");

    h.analyze(true);
    let (s, v) = post(&h, "/api/v1/splitorder", split(995, 10)).await;
    assert_eq!(s, 200, "{v}");
    let rows = v["results"].as_array().unwrap();
    assert_eq!(rows.len(), 100, "{v}");
    assert!(rows.iter().all(|r| r["status"] == "success"), "{v}");
    let sizes: Vec<i64> = rows
        .iter()
        .map(|r| r["quantity"].as_i64().unwrap())
        .collect();
    assert_eq!(sizes.iter().sum::<i64>(), 995);
    assert_eq!((sizes[0], sizes[98], sizes[99]), (10, 10, 5));
    assert_eq!(rows[99]["order_num"], 100);
    assert_eq!(sandbox_orders(&h).await, 100);
    assert_eq!(placed(&h), [7], "nothing more reached the broker");
    h.shutdown().await;
}
