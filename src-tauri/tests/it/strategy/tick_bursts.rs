//! The strategy tick path under a burst (benchmarks P1): a deciding price
//! is never lost while the evaluator is busy, and a tick the reader does
//! miss is counted for health.

use super::*;
use openalgo_desktop_lib::brokers::common::streaming::{FeedEvent, MarketEvent, NormalizedTick};
use openalgo_desktop_lib::strategy::tick_feed::{Key, PriceSource};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

/// A market-data channel as small as a burst needs to overrun it.
struct Burst {
    tx: broadcast::Sender<MarketEvent>,
    subscribed: Mutex<Vec<Key>>,
}

impl Burst {
    fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            tx: broadcast::channel(capacity).0,
            subscribed: Mutex::new(Vec::new()),
        })
    }

    fn send(&self, symbol: &str, ltp: f64) {
        let tick = NormalizedTick {
            symbol: symbol.into(),
            exchange: "NFO".into(),
            ltp,
            ..Default::default()
        };
        let _ = self.tx.send(Arc::new(FeedEvent::Tick(tick)));
    }
}

#[async_trait::async_trait]
impl PriceSource for Burst {
    async fn subscribe(&self, keys: &[Key]) {
        self.subscribed.lock().extend_from_slice(keys);
    }
    async fn unsubscribe(&self, _keys: &[Key]) {}
    fn ticks(&self) -> broadcast::Receiver<MarketEvent> {
        self.tx.subscribe()
    }
    async fn poll(&self, _keys: &[Key]) -> Vec<(Key, f64)> {
        vec![]
    }
}

/// A long weekly NIFTY call at `offset` from the money, with `risk`.
fn long_call(offset: &str, risk: Value) -> Value {
    let mut leg = json!({
        "id": 1, "segment": "options", "expiry": "weekly", "lots": 1,
        "position": "B", "option_type": "CE", "strike_mode": "atm",
        "atm_offset": offset, "trail": {"x": 0, "y": 0},
    });
    for (k, v) in risk.as_object().unwrap() {
        leg[k] = v.clone();
    }
    leg
}

/// A run of one long call bought at 100; answers its run and contract.
async fn held(t: &T, name: &str, offset: &str, risk: Value) -> (i64, String) {
    let sid = t.make(config(name, json!([long_call(offset, risk)]), json!({})));
    let run = t.start_filled(sid, 100.0).await;
    (run, t.leg(run, 1).symbol)
}

fn sold(t: &T, symbol: &str) -> bool {
    t.gw.placed()
        .iter()
        .any(|o| o.symbol == symbol && o.action == "SELL")
}

/// While the evaluator is held placing one run's exit, the feed floods
/// three other held contracts, each with one spike in it: through a stop
/// (to 85 under 90), through a target (to 115 over 110), and up through a
/// trailing stop's trigger (to 150, trailing it to 140) with the noise
/// after it under the trail. Before, one consumer read and evaluated: held
/// in the placement it stopped reading, the channel overran and the spikes
/// were skipped (`Lagged`, logged at debug), so three positions got no
/// exit. Now the reader keeps reading while decisions wait, each contract
/// keeps its path (folded to its extremes in order when long), and every
/// spike is decided once the evaluator is free.
#[tokio::test]
async fn a_spike_is_decided_even_while_the_evaluator_is_held() {
    let burst = Burst::new(16);
    let t = t_with(
        ist(2026, 10, 7, 10, 0),
        Some(burst.clone() as Arc<dyn PriceSource>),
    );
    t.m.start().await;
    let (_, blocker) = held(&t, "Blocker", "OTM3", json!({"sl_pts": 10})).await;
    let (stop_run, stop) = held(&t, "Stop", "ATM", json!({"sl_pts": 10})).await;
    let (target_run, target) = held(&t, "Target", "OTM1", json!({"target_pts": 10})).await;
    let (trail_run, trail) = held(
        &t,
        "Trail",
        "OTM2",
        json!({"sl_pts": 10, "trail": {"x": 10, "y": 0}}),
    )
    .await;

    let gate = Arc::new(Gate::default());
    *t.gw.place_gate.lock() = Some(gate.clone());
    burst.send(&blocker, 85.0);
    gate.entered().await;
    for i in 0..200 {
        let noise = 100.0 + f64::from(i % 5) * 0.1;
        burst.send(&stop, if i == 50 { 85.0 } else { noise });
        burst.send(&target, if i == 50 { 115.0 } else { noise });
        burst.send(&trail, if i == 50 { 150.0 } else { noise });
        tokio::task::yield_now().await;
    }
    *t.gw.place_gate.lock() = None;
    gate.release();

    for _ in 0..500 {
        if [&blocker, &stop, &target, &trail]
            .iter()
            .all(|s| sold(&t, s))
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    for (name, symbol) in [("stop", &stop), ("target", &target), ("trail", &trail)] {
        assert!(sold(&t, symbol), "the {} spike was never decided", name);
    }
    let kind = |run: i64| {
        t.orders(run)
            .into_iter()
            .find(|o| o.kind != "entry")
            .map(|o| o.kind)
    };
    assert_eq!(kind(stop_run).as_deref(), Some("exit_sl"));
    assert_eq!(kind(target_run).as_deref(), Some("exit_target"));
    assert_eq!(kind(trail_run).as_deref(), Some("exit_sl"));
    let h = t.m.feed.health();
    assert_eq!(h["ticks_missed"], 0, "{}", h);
    assert!(h["ticks_folded"].as_u64().unwrap() > 0, "{}", h);
    t.m.shutdown().await;
}

/// A tick the reader itself misses (the channel overran it) is counted and
/// turns the strategy tick health to warn; the web logs it at debug only.
#[tokio::test]
async fn a_missed_tick_is_counted_for_health() {
    let burst = Burst::new(4);
    let t = t_with(
        ist(2026, 10, 7, 10, 0),
        Some(burst.clone() as Arc<dyn PriceSource>),
    );
    t.m.start().await;
    let (_, symbol) = held(&t, "Held", "ATM", json!({"sl_pts": 10})).await;
    // Sent without yielding: the reader cannot run in between.
    for _ in 0..20 {
        burst.send(&symbol, 100.5);
    }
    for _ in 0..500 {
        if t.m.feed.health()["ticks_missed"].as_u64().unwrap_or(0) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let h = t.m.feed.health();
    assert_eq!(h["ticks_missed"], 16, "{}", h);
    assert_eq!(h["status"], "warn");
    assert_eq!(t.m.feed.ticks_missed_unreported(), Some(16));
    assert_eq!(t.m.feed.ticks_missed_unreported(), None, "alerted once");
    t.m.shutdown().await;
}
