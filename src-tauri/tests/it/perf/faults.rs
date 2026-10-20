//! The faults the endurance run injects into the running app, each with
//! its expected outcome written down before the run and checked after the
//! fault (`docs/audit/fault-drill.md` has the same list for a real
//! install, where the maintainer injects them by hand, plus disk full,
//! which cannot be injected portably in-process).
//!
//! | Fault | Injected as | Expected outcome |
//! | --- | --- | --- |
//! | Broker feed disconnect | the broker market socket drops every connection without a close frame | the feed reconnects and resubscribes within 10 s, ticks reach the SDK client again, the reconciler's gap for the reconnect closes |
//! | Order-socket drop | the broker order socket drops without a close frame | the order socket reconnects within 10 s and the reconciler reads the order book (gap closed) within 10 s |
//! | Sleep and wake | the wall clock jumps 2 minutes ahead of the monotonic clock | within 5 s the wake is noticed: both broker sockets reconnect and the reconciler is asked for a pass; the API keeps answering |
//! | Clock change | the wall clock is set back 2 minutes | nothing reconnects and nothing is redone (a clock set back is not a wake); the API keeps answering |
//! | Locked database | another program holds `openalgo.db`'s write lock for 3 s | a live order placed meanwhile is accepted at once; a stop saved meanwhile waits and is saved when the lock goes, within the 5 s busy budget |

use super::rig::Rig;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    FeedDrop,
    OrderSocketDrop,
    SleepWake,
    ClockBack,
    LockedDatabase,
}

pub const ROTATION: [Fault; 5] = [
    Fault::FeedDrop,
    Fault::OrderSocketDrop,
    Fault::SleepWake,
    Fault::ClockBack,
    Fault::LockedDatabase,
];

impl Fault {
    pub fn name(&self) -> &'static str {
        match self {
            Fault::FeedDrop => "feed_drop",
            Fault::OrderSocketDrop => "order_socket_drop",
            Fault::SleepWake => "sleep_wake",
            Fault::ClockBack => "clock_back",
            Fault::LockedDatabase => "locked_database",
        }
    }
}

/// What happened after one fault.
#[derive(Debug)]
pub struct Outcome {
    pub fault: Fault,
    pub met: bool,
    pub detail: String,
}

const RECOVER: Duration = Duration::from_secs(10);
const LOCK_HELD: Duration = Duration::from_secs(3);

/// Poll `f` every 10 ms until it holds or `limit` after `t0` passes; how
/// long after `t0` (the fault) it held.
async fn within(t0: Instant, limit: Duration, mut f: impl FnMut() -> bool) -> Option<Duration> {
    loop {
        if f() {
            return Some(t0.elapsed());
        }
        if t0.elapsed() > limit {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn secs(d: Option<Duration>) -> String {
    d.map(|d| format!("{:.2}s", d.as_secs_f64()))
        .unwrap_or_else(|| "never".into())
}

pub async fn inject(rig: &Rig, fault: Fault, sdk_frames: &std::sync::atomic::AtomicU64) -> Outcome {
    let ctx = &rig.ctx;
    let feed_connects = || ctx.websocket.stats().connects.load(Ordering::SeqCst);
    let order_connects = || ctx.runtime.order_ws.stats().connects.load(Ordering::SeqCst);
    let gaps_closed = || ctx.reconciler.stats.gaps_closed.load(Ordering::SeqCst);
    let (f0, o0, g0) = (feed_connects(), order_connects(), gaps_closed());
    let t0 = Instant::now();
    let (met, detail) = match fault {
        Fault::FeedDrop => {
            rig.market.kick();
            let back = within(t0, RECOVER, || {
                feed_connects() > f0 && ctx.websocket.is_connected()
            })
            .await;
            let frames = sdk_frames.load(Ordering::Relaxed);
            let ticks = within(t0, RECOVER, || {
                sdk_frames.load(Ordering::Relaxed) > frames + 100
            })
            .await;
            let gap = within(t0, RECOVER, || gaps_closed() > g0).await;
            (
                back.is_some() && ticks.is_some() && gap.is_some(),
                format!(
                    "reconnected {} ticks {} gap_closed {}",
                    secs(back),
                    secs(ticks),
                    secs(gap)
                ),
            )
        }
        Fault::OrderSocketDrop => {
            rig.orders.kick();
            let back = within(t0, RECOVER, || {
                order_connects() > o0 && ctx.runtime.order_ws.is_connected()
            })
            .await;
            let gap = within(t0, RECOVER, || gaps_closed() > g0).await;
            (
                back.is_some() && gap.is_some(),
                format!("reconnected {} gap_closed {}", secs(back), secs(gap)),
            )
        }
        Fault::SleepWake => {
            let asked = ctx.reconciler.stats.requested.load(Ordering::SeqCst);
            rig.clock.jump(chrono::Duration::minutes(2));
            let noticed = within(t0, Duration::from_secs(5), || {
                feed_connects() > f0
                    && order_connects() > o0
                    && ctx.reconciler.stats.requested.load(Ordering::SeqCst) > asked
            })
            .await;
            let api = api_answers(rig).await;
            (
                noticed.is_some() && api,
                format!("woke_and_reconnected {} api {}", secs(noticed), api),
            )
        }
        Fault::ClockBack => {
            rig.clock.jump(chrono::Duration::minutes(-2));
            tokio::time::sleep(Duration::from_secs(5)).await;
            let quiet = feed_connects() == f0 && order_connects() == o0;
            let api = api_answers(rig).await;
            (quiet && api, format!("no_reconnect {} api {}", quiet, api))
        }
        Fault::LockedDatabase => locked_database(rig).await,
    };
    Outcome { fault, met, detail }
}

/// `/api/v1/funds` answers 200 with success.
async fn api_answers(rig: &Rig) -> bool {
    let r = reqwest::Client::new()
        .post(format!("{}/api/v1/funds", rig.base))
        .json(&json!({"apikey": rig.key}))
        .timeout(Duration::from_secs(10))
        .send()
        .await;
    match r {
        Ok(resp) => {
            let ok = resp.status().is_success();
            let v: Value = resp.json().await.unwrap_or(Value::Null);
            ok && v["status"] == "success"
        }
        Err(_) => false,
    }
}

/// Another program takes `openalgo.db`'s write lock for `LOCK_HELD`; a live
/// order and a stop save happen meanwhile.
async fn locked_database(rig: &Rig) -> (bool, String) {
    let path = rig.dir.path().join("openalgo.db");
    let (locked_tx, locked_rx) = tokio::sync::oneshot::channel();
    let holder = tokio::task::spawn_blocking(move || {
        let c = rusqlite::Connection::open(path).unwrap();
        c.execute_batch("BEGIN EXCLUSIVE").unwrap();
        let _ = locked_tx.send(());
        std::thread::sleep(LOCK_HELD);
        c.execute_batch("COMMIT").unwrap();
    });
    locked_rx.await.unwrap();
    let t0 = Instant::now();
    let store = rig.ctx.scalping.store.clone();
    let save = tokio::task::spawn_blocking(move || {
        store.upsert_sl(&openalgo_desktop_lib::scalping::store::SlUpsert {
            symbol: "SYM0999".into(),
            exchange: "NSE".into(),
            product: "MIS".into(),
            mode: "live".into(),
            side: Some("BUY".into()),
            entry_price: Some(1000.0),
            quantity: Some(1),
            initial_sl: Some(990.0),
            is_active: Some(false),
            ..Default::default()
        })
    });
    let order_t0 = Instant::now();
    let order = place_live(rig).await;
    let order_took = order_t0.elapsed();
    let saved = save.await.unwrap();
    let save_took = t0.elapsed();
    holder.await.unwrap();
    let met = order
        && order_took < Duration::from_secs(1)
        && saved.is_ok()
        && save_took < Duration::from_secs(5);
    (
        met,
        format!(
            "order_accepted {} in {:.2}s, stop_saved {} after {:.2}s",
            order,
            order_took.as_secs_f64(),
            saved.is_ok(),
            save_took.as_secs_f64()
        ),
    )
}

/// One live MARKET order over `/api/v1/placeorder`; accepted?
pub async fn place_live(rig: &Rig) -> bool {
    let body = json!({
        "apikey": rig.key, "strategy": "endurance", "symbol": "SYM0001", "action": "BUY",
        "exchange": "NSE", "pricetype": "MARKET", "product": "MIS", "quantity": "1"
    });
    let r = reqwest::Client::new()
        .post(format!("{}/api/v1/placeorder", rig.base))
        .json(&body)
        .timeout(Duration::from_secs(10))
        .send()
        .await;
    match r {
        Ok(resp) => {
            let v: Value = resp.json().await.unwrap_or(Value::Null);
            v["status"] == "success"
        }
        Err(_) => false,
    }
}
