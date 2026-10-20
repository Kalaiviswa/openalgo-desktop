//! Target: feed tick to risk decision (stop, target, trailing), p99 under
//! 50 ms at 1,000 subscribed symbols and 2,000 ticks a second.
//!
//! The whole path runs: the synthetic broker socket sends a tick, the
//! `MockFeed` adapter parses it, the market-data manager broadcasts it, the
//! risk owner judges its legs on it through the risk core, and a breach
//! places the exit through the order service to `MockBroker`. A decision is
//! timed from the moment the deciding tick is handed to the app's socket
//! until the exit order reaches the broker (an upper bound on the decision
//! itself), or, for a trailing move, until the leg's stop shows the new
//! level. Both risk owners are measured, each in its own run:
//!
//! - the scalping risk monitor (live legs; a breach reads the broker's
//!   position book, then exits);
//! - the strategy module (one live run per leg; a breach writes the exit's
//!   order row, then exits, on the tick task).
//!
//! Load: 1,000 NSE symbols, all subscribed by an SDK client on the 8765
//! feed (which reads every frame), 2,000 ticks a second over them with a
//! slow uptrend so every high keeps moving, and 300 managed legs: 100
//! stops, 100 targets and 100 trailing stops. One decision is asked for
//! every 100 ms.

use super::lag_log::{self, Skipped};
use super::rig::{until_long, Rig, RigOptions, BASE_PRICE};
use super::{Report, Stats};
use crate::feed_support::Client;
use openalgo_desktop_lib::brokers::mock::MockCall;
use openalgo_desktop_lib::scalping::store::SlUpsert;
use openalgo_desktop_lib::strategy::engine::FillOpts;
use parking_lot::Mutex;
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const SYMBOLS: usize = 1000;
const TICKS_PER_SECOND: u64 = 2000;
/// Managed legs of each kind; `OPENALGO_PERF_LEGS_PER_KIND` overrides it
/// to find how many an owner can carry.
const LEGS_PER_KIND: usize = 100;
const TRIGGER_EVERY: Duration = Duration::from_millis(100);
const WARM_UP: Duration = Duration::from_secs(5);
const TARGET_P99: Duration = Duration::from_millis(50);
/// A decision not seen within this counts as lost (and fails the run).
const GIVE_UP: Duration = Duration::from_secs(10);
/// Prices rise this many points a second, so every leg's high keeps
/// moving (and the scalping monitor saves it, throttled) as in a trending
/// market; far from the targets over a run of a few minutes.
const TREND_PER_S: f64 = 0.02;

const STOP_PTS: f64 = 10.0;
const TARGET_PTS: f64 = 10.0;
const TRAIL_PTS: f64 = 10.0;
/// The trailing leg's high, and the tick under its trailed stop.
const HIGH: f64 = BASE_PRICE + 50.0;
const UNDER_TRAIL: f64 = HIGH - TRAIL_PTS - 5.0;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Kind {
    Stop,
    Target,
    TrailMove,
    TrailExit,
}

/// The risk owner under test, and how to read a leg's current stop.
#[derive(Clone)]
enum Owner {
    Scalping,
    /// Run id by symbol (leg 1 of each run).
    Strategy(Arc<HashMap<String, i64>>),
}

impl Owner {
    fn name(&self) -> &'static str {
        match self {
            Owner::Scalping => "tick_to_risk_scalping",
            Owner::Strategy(_) => "tick_to_risk_strategy",
        }
    }

    fn stop_of(&self, rig: &Rig, symbol: &str) -> Option<f64> {
        match self {
            Owner::Scalping => rig
                .ctx
                .scalping
                .monitor
                .leg("live", symbol, "NSE", "MIS")
                .and_then(|l| l.current_sl),
            Owner::Strategy(runs) => {
                let run = *runs.get(symbol)?;
                rig.ctx
                    .strategy
                    .state
                    .snapshot(run)
                    .and_then(|r| r.leg(1).and_then(|l| l.effective_sl))
            }
        }
    }
}

/// 300 legs on SYM0000..SYM0299, the kinds interleaved.
fn legs() -> Vec<(String, Kind)> {
    let per_kind = std::env::var("OPENALGO_PERF_LEGS_PER_KIND")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| (1..=300).contains(n))
        .unwrap_or(LEGS_PER_KIND);
    (0..3 * per_kind)
        .map(|i| {
            let kind = [Kind::Stop, Kind::Target, Kind::TrailMove][i % 3];
            (format!("SYM{:04}", i), kind)
        })
        .collect()
}

fn scalping_leg(symbol: &str, kind: Kind) -> SlUpsert {
    SlUpsert {
        symbol: symbol.into(),
        exchange: "NSE".into(),
        product: "MIS".into(),
        mode: "live".into(),
        side: Some("BUY".into()),
        entry_price: Some(BASE_PRICE),
        quantity: Some(1),
        initial_sl: (kind != Kind::Target).then_some(BASE_PRICE - STOP_PTS),
        target: (kind == Kind::Target).then_some(BASE_PRICE + TARGET_PTS),
        trailing_enabled: Some(kind == Kind::TrailMove),
        trailing_step: (kind == Kind::TrailMove).then_some(TRAIL_PTS),
        is_active: Some(true),
        ..Default::default()
    }
}

/// A one-leg cash strategy, long one share: a stop, a target, or a
/// fixed-distance trail (`trail.x` alone).
fn strategy_config(symbol: &str, kind: Kind) -> serde_json::Value {
    let mut leg = json!({"id": 1, "segment": "cash", "position": "B", "lots": 1});
    match kind {
        Kind::Stop => leg["sl_pts"] = json!(STOP_PTS),
        Kind::Target => leg["target_pts"] = json!(TARGET_PTS),
        _ => {
            leg["sl_pts"] = json!(STOP_PTS);
            leg["trail"] = json!({"x": TRAIL_PTS, "y": 0});
        }
    }
    json!({
        "name": format!("Perf {}", symbol),
        "underlying": symbol,
        "underlying_exchange": "NSE",
        "universe_tab": "stocks_fno",
        "product": "CNC",
        "strategy_type": "positional",
        "legs": [leg],
    })
}

/// Start one live run per leg and fill its entry at the base price.
async fn start_runs(rig: &Rig, legs: &[(String, Kind)]) -> HashMap<String, i64> {
    let s = &rig.ctx.strategy;
    let mut runs = HashMap::new();
    for (symbol, kind) in legs {
        let (row, _) = s
            .store
            .create_strategy("alice", &strategy_config(symbol, *kind))
            .unwrap();
        s.store.set_live_enabled(row.id, "alice", true).unwrap();
        let r = s.start_run(row.id, "alice", "live", "manual", None).await;
        assert!(r.ok, "{} did not start: {:?}", symbol, r);
        let run = r.run_id.unwrap();
        s.apply_fill(run, 1, Some(BASE_PRICE), true, FillOpts::default())
            .await;
        runs.insert(symbol.clone(), run);
    }
    runs
}

/// When the first exit (a SELL) for each symbol reached the broker, found
/// by scanning the broker's call log every millisecond.
fn watch_exits(
    rig: &Rig,
) -> (
    Arc<Mutex<HashMap<String, Instant>>>,
    tokio::task::JoinHandle<()>,
) {
    let placed = Arc::new(Mutex::new(HashMap::new()));
    let (mock, seen) = (rig.mock.clone(), placed.clone());
    let task = tokio::spawn(async move {
        let mut next = 0usize;
        let mut tick = tokio::time::interval(Duration::from_millis(1));
        loop {
            tick.tick().await;
            let now = Instant::now();
            let calls = mock.calls.lock();
            for c in calls.iter().skip(next) {
                if let MockCall::PlaceOrder(o) = c {
                    if o.action.as_str() == "SELL" {
                        seen.lock().entry(o.symbol.clone()).or_insert(now);
                    }
                }
            }
            next = calls.len();
        }
    });
    (placed, task)
}

/// Send the deciding tick and wait for its decision; the latency from the
/// tick leaving the broker socket.
async fn decide(
    rig: &Rig,
    owner: &Owner,
    placed: &Mutex<HashMap<String, Instant>>,
    (symbol, price, kind): (&str, f64, Kind),
) -> Option<Duration> {
    let stamp = rig.market.send_stamped(symbol, price);
    let deadline = Instant::now() + GIVE_UP;
    let mut tick = tokio::time::interval(Duration::from_millis(1));
    loop {
        tick.tick().await;
        let decided = match kind {
            Kind::TrailMove => {
                (owner.stop_of(rig, symbol) == Some(price - TRAIL_PTS)).then(Instant::now)
            }
            _ => placed.lock().get(symbol).copied(),
        };
        if let (Some(at), Some(sent)) = (decided, rig.market.sent_at(stamp)) {
            return Some(at.saturating_duration_since(sent));
        }
        if Instant::now() > deadline {
            return None;
        }
    }
}

/// An SDK client on the 8765 feed holding every symbol; it reads every
/// frame and counts the market data it got.
async fn sdk_client(rig: &Rig) -> (Arc<AtomicU64>, tokio::task::JoinHandle<()>) {
    let mut c = Client::connect(&rig.ws_url).await;
    let v = c
        .request(json!({"action": "authenticate", "api_key": rig.key}))
        .await;
    assert_eq!(v["status"], "success", "{}", v);
    let symbols: Vec<_> = rig
        .symbols
        .iter()
        .map(|s| json!({"symbol": s, "exchange": "NSE"}))
        .collect();
    c.send(&json!({"action": "subscribe", "symbols": symbols, "mode": 1}))
        .await;
    let frames = Arc::new(AtomicU64::new(0));
    let n = frames.clone();
    let task = tokio::spawn(async move {
        while let Some(v) = c.recv_within(Duration::from_secs(30)).await {
            if v["type"] == "market_data" {
                n.fetch_add(1, Ordering::Relaxed);
            }
        }
    });
    (frames, task)
}

/// The rig with every symbol subscribed and the trailing legs left out of
/// the noise (a noise tick after a trail moved would take the exit first).
async fn loaded_rig(legs: &[(String, Kind)]) -> (Rig, Arc<AtomicU64>, tokio::task::JoinHandle<()>) {
    let trailing: Vec<String> = legs
        .iter()
        .filter(|(_, k)| *k == Kind::TrailMove)
        .map(|(s, _)| s.clone())
        .collect();
    let rig = Rig::start(RigOptions {
        symbols: SYMBOLS,
        tick_rate: TICKS_PER_SECOND,
        quiet: trailing,
        trend_per_s: TREND_PER_S,
    })
    .await;
    let (frames, reader) = sdk_client(&rig).await;
    let ctx = rig.ctx.clone();
    until_long("every symbol subscribed at the broker", || {
        ctx.websocket.instrument_count() >= SYMBOLS
    })
    .await;
    (rig, frames, reader)
}

/// What one tick costs the strategy module for a run that holds the
/// symbol (two run reads, the evaluation, the room push), timed over 200
/// direct calls at a price that decides nothing, beside the feed's load.
async fn evaluation_cost(rig: &Rig, symbol: &str) -> Duration {
    let t = Instant::now();
    for _ in 0..200 {
        rig.ctx
            .strategy
            .process_tick(symbol, "NSE", BASE_PRICE)
            .await;
    }
    t.elapsed() / 200
}

/// One block of `/health/api/current` as compact JSON, or `absent`.
async fn health_block(rig: &Rig, key: &str) -> String {
    let v: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/health/api/current", rig.base))
        .header("cookie", &rig.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap_or_default();
    match &v[key] {
        serde_json::Value::Null => "absent".into(),
        b => b.to_string(),
    }
}

/// Ask for every leg's decision, one every `TRIGGER_EVERY`, and report.
async fn measure(
    skipped: &Skipped,
    rig: Rig,
    owner: Owner,
    legs: Vec<(String, Kind)>,
    frames: Arc<AtomicU64>,
) -> bool {
    tokio::time::sleep(WARM_UP).await;
    let ctx = rig.ctx.clone();
    let (placed, watcher) = watch_exits(&rig);
    let events_before = ctx.websocket.stats().events.load(Ordering::Relaxed);
    let generated_before = rig.market.generated.load(Ordering::Relaxed);
    let started = Instant::now();
    let rig = Arc::new(rig);
    let mut waiters = tokio::task::JoinSet::new();
    let mut pace = tokio::time::interval(TRIGGER_EVERY);
    for (symbol, kind) in legs.clone() {
        pace.tick().await;
        let (r, p, o) = (rig.clone(), placed.clone(), owner.clone());
        let at = started.elapsed();
        waiters.spawn(async move {
            let s = symbol.as_str();
            let decided = match kind {
                Kind::Stop => {
                    let d = decide(&r, &o, &p, (s, BASE_PRICE - STOP_PTS - 5.0, kind)).await;
                    vec![(kind, d)]
                }
                Kind::Target => {
                    let d = decide(&r, &o, &p, (s, BASE_PRICE + TARGET_PTS + 5.0, kind)).await;
                    vec![(kind, d)]
                }
                _ => {
                    let moved = decide(&r, &o, &p, (s, HIGH, Kind::TrailMove)).await;
                    let exit = decide(&r, &o, &p, (s, UNDER_TRAIL, Kind::TrailExit)).await;
                    vec![(Kind::TrailMove, moved), (Kind::TrailExit, exit)]
                }
            };
            (at, symbol, decided)
        });
    }
    let mut by_kind: HashMap<Kind, Vec<Duration>> = HashMap::new();
    let mut lost = 0usize;
    let mut slow = Vec::new();
    while let Some(r) = waiters.join_next().await {
        let (at, symbol, decided) = r.unwrap();
        for (kind, d) in decided {
            if d.is_none_or(|d| d > TARGET_P99) {
                slow.push((at, symbol.clone(), kind, d));
            }
            match d {
                Some(d) => by_kind.entry(kind).or_default().push(d),
                None => lost += 1,
            }
        }
    }
    // When the slow ones were asked for: a cluster every few seconds points
    // at a periodic task, a spread at a busy tick path.
    slow.sort_by_key(|(at, ..)| *at);
    for (at, symbol, kind, d) in &slow {
        eprintln!(
            "slow decision: +{:.1} s {:?} {} {}",
            at.as_secs_f64(),
            kind,
            symbol,
            d.map(super::ms)
                .map(|m| format!("{} ms", m))
                .unwrap_or_else(|| "never".into())
        );
    }
    let ran = started.elapsed().as_secs_f64();
    watcher.abort();
    let events = ctx.websocket.stats().events.load(Ordering::Relaxed) - events_before;
    let generated = rig.market.generated.load(Ordering::Relaxed) - generated_before;
    let all: Vec<Duration> = by_kind.values().flatten().copied().collect();
    let overall = Stats::of(&all);
    let mut report = Report::new(owner.name())
        .field("symbols", SYMBOLS)
        .field("ticks_per_s_target", TICKS_PER_SECOND)
        .field("ticks_per_s_sent", format!("{:.0}", generated as f64 / ran))
        .field(
            "ticks_per_s_received",
            format!("{:.0}", events as f64 / ran),
        )
        .field("legs", legs.len())
        .field("lost_decisions", lost)
        .stats("all", &overall);
    for (name, kind) in [
        ("stop", Kind::Stop),
        ("target", Kind::Target),
        ("trail_move", Kind::TrailMove),
        ("trail_exit", Kind::TrailExit),
    ] {
        if let Some(v) = by_kind.get(&kind) {
            report = report.stats(name, &Stats::of(v));
        }
    }
    let bus = ctx.bus.stats();
    let met = report
        .field("sdk_frames", frames.load(Ordering::Relaxed))
        .field(
            "socket_frames_skipped",
            rig.market.skipped.load(Ordering::Relaxed),
        )
        .field("relay_lagged", bus.relay_lagged.load(Ordering::Relaxed))
        .field("fact_losses", ctx.bus.gap_count())
        .field("ticks_skipped", skipped.summary())
        .field(
            "health_strategy_ticks",
            health_block(&rig, "strategy_ticks").await,
        )
        .verdict("all_p99_ms<50", overall.p99 < TARGET_P99 && lost == 0);
    if let Ok(r) = Arc::try_unwrap(rig) {
        r.stop().await;
    }
    assert_eq!(lost, 0, "{} decisions were never made", lost);
    met
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn scalping_monitor_tick_to_decision_p99_under_50ms() {
    crate::isolated!(scalping_monitor_tick_to_decision_p99_under_50ms);
    let skipped = lag_log::install();
    let legs = legs();
    let (rig, frames, reader) = loaded_rig(&legs).await;
    let ctx = rig.ctx.clone();
    let held: Vec<(String, i32)> = legs.iter().map(|(s, _)| (s.clone(), 1)).collect();
    rig.set_positions(&held);
    for (s, k) in &legs {
        ctx.scalping.store.upsert_sl(&scalping_leg(s, *k)).unwrap();
    }
    ctx.scalping.monitor.request_sync();
    until_long("every leg managed", || {
        ctx.scalping.monitor.leg_count() == legs.len()
            && ctx.scalping.monitor.subscribed().len() == legs.len()
    })
    .await;
    let met = measure(&skipped, rig, Owner::Scalping, legs, frames).await;
    reader.abort();
    assert!(met, "scalping tick to decision p99 over 50 ms");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn strategy_module_tick_to_decision_p99_under_50ms() {
    crate::isolated!(strategy_module_tick_to_decision_p99_under_50ms);
    let skipped = lag_log::install();
    let legs = legs();
    let (rig, frames, reader) = loaded_rig(&legs).await;
    let ctx = rig.ctx.clone();
    // The strategy module's tick consumer, as the app starts it.
    ctx.strategy.start().await;
    let runs = start_runs(&rig, &legs).await;
    until_long("every run's leg open", || {
        legs.iter().all(|(s, _)| {
            ctx.strategy
                .state
                .snapshot(runs[s])
                .and_then(|r| r.leg(1).map(|l| l.status == "open"))
                .unwrap_or(false)
        })
    })
    .await;
    // The entries were BUYs; only exits (SELLs) are watched from here.
    let cost = evaluation_cost(&rig, &legs[0].0).await;
    eprintln!(
        "strategy: one tick's evaluation of a held leg takes {} ms on average",
        super::ms(cost)
    );
    let met = measure(&skipped, rig, Owner::Strategy(Arc::new(runs)), legs, frames).await;
    reader.abort();
    assert!(met, "strategy tick to decision p99 over 50 ms");
}
