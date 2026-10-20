//! Target: critical event delivery (order, position and risk topics) has
//! zero drops under a 10x burst; market-data drops are counted and shown.
//!
//! "Normal" here is 100 order-path events a second (`order.placed`,
//! `order.update` twice, `position.closed`): a 25-order basket every
//! second with its fills, two and a half times the 10-a-second order
//! lane. The burst is ten times that, 1,000 a second for 10 s, between 5 s
//! of normal flow on each side, published from outside the bus as the
//! services do, to every subscriber the app registers (the strategy book
//! and order matching on the critical lane; the order log, Socket.IO, the
//! feed's order relay and the alert senders on the best-effort lane) plus
//! a probe on the critical lane that checks it received every event.
//!
//! Meanwhile the broker feed streams 2,000 ticks a second over 1,000
//! symbols to two SDK clients holding all of them: one reads every frame,
//! one reads a frame every 2 ms and falls behind, so its market data is
//! coalesced and dropped. The run reports what that client lost and
//! whether health shows it.

use super::rig::{until_long, Rig, RigOptions};
use super::{Report, Stats};
use crate::feed_support::Client;
use openalgo_desktop_lib::events::{Event, Lane, Mode, OrderUpdate, Subscriber, Topic};
use openalgo_desktop_lib::services::core;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const NORMAL_PER_S: u64 = 100;
const BURST_PER_S: u64 = 10 * NORMAL_PER_S;
const SCHEDULE: [(u64, Duration); 3] = [
    (NORMAL_PER_S, Duration::from_secs(5)),
    (BURST_PER_S, Duration::from_secs(10)),
    (NORMAL_PER_S, Duration::from_secs(5)),
];
const DRAIN_LIMIT: Duration = Duration::from_secs(60);

/// Each bus queue's deepest depth seen, its capacity and its lane.
type Deepest = HashMap<&'static str, (usize, usize, Lane)>;

/// A critical-lane subscriber that notes when each event arrived.
struct Probe {
    published: Arc<Mutex<HashMap<u64, Instant>>>,
    latencies: Mutex<Vec<Duration>>,
    received: AtomicU64,
}

#[async_trait::async_trait]
impl Subscriber for Probe {
    fn name(&self) -> &'static str {
        "perf-probe"
    }
    fn topics(&self) -> Vec<Topic> {
        vec![
            Topic::OrderPlaced,
            Topic::OrderUpdate,
            Topic::PositionClosed,
        ]
    }
    async fn handle(&self, event: Arc<Event>) {
        let now = Instant::now();
        let id = match event.as_ref() {
            Event::OrderPlaced { orderid, .. } => orderid.clone(),
            Event::OrderUpdate(u) => u.orderid.clone(),
            Event::PositionClosed { message, .. } => message.clone().unwrap_or_default(),
            _ => return,
        };
        let seq = id.trim_start_matches("perf-").parse::<u64>().ok();
        if let Some(at) = seq.and_then(|s| self.published.lock().get(&s).copied()) {
            self.latencies
                .lock()
                .push(now.saturating_duration_since(at));
        }
        self.received.fetch_add(1, Ordering::Relaxed);
    }
}

/// The `seq`th event: a placement, its open and complete updates, the
/// position closed.
fn event(seq: u64, generation: u64) -> Event {
    let id = format!("perf-{}", seq);
    let meta = |api: &str| core::meta(Mode::Live, api, json!({}), &json!({"status": "success"}));
    let update = |status: &str, filled: i64| {
        Event::OrderUpdate(OrderUpdate {
            mode: "live".into(),
            broker: "zerodha".into(),
            orderid: id.clone(),
            symbol: "SYM0001".into(),
            exchange: "NSE".into(),
            action: "BUY".into(),
            quantity: 1,
            price: 1000.0,
            pricetype: "MARKET".into(),
            product: "MIS".into(),
            order_status: status.into(),
            filled_quantity: filled,
            pending_quantity: 1 - filled,
            average_price: 1000.0,
            session_generation: generation,
            ..Default::default()
        })
    };
    match seq % 4 {
        0 => Event::OrderPlaced {
            meta: meta("placeorder"),
            strategy: "perf".into(),
            symbol: "SYM0001".into(),
            exchange: "NSE".into(),
            action: "BUY".into(),
            quantity: 1,
            pricetype: "MARKET".into(),
            product: "MIS".into(),
            orderid: id,
        },
        1 => update("open", 0),
        2 => update("complete", 1),
        _ => Event::PositionClosed {
            meta: meta("closeposition"),
            message: Some(id),
        },
    }
}

/// An SDK client holding every symbol that pauses `per_frame` after each
/// frame it reads; counts the market data it got.
async fn reader(rig: &Rig, per_frame: Duration) -> (Arc<AtomicU64>, tokio::task::JoinHandle<()>) {
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
            if !per_frame.is_zero() {
                tokio::time::sleep(per_frame).await;
            }
        }
    });
    (frames, task)
}

/// What health says the apps on the feed were not sent (folded into newer
/// frames, dropped, skipped), when it says it.
fn shown_in_health(health: &Value) -> Option<(u64, u64, u64)> {
    let c = &health["market_data"]["feed_clients"];
    Some((
        c["coalesced"].as_u64()?,
        c["dropped"].as_u64()?,
        c["skipped"].as_u64()?,
    ))
}

async fn health(rig: &Rig) -> Value {
    reqwest::Client::new()
        .get(format!("{}/health/api/current", rig.base))
        .header("cookie", &rig.cookie)
        .header("accept", "application/json")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap_or(Value::Null)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn critical_events_have_zero_drops_under_a_10x_burst() {
    crate::isolated!(critical_events_have_zero_drops_under_a_10x_burst);
    let skipped = super::lag_log::install();
    let rig = Rig::start(RigOptions {
        symbols: 1000,
        tick_rate: 2000,
        quiet: vec![],
        trend_per_s: 0.0,
    })
    .await;
    let ctx = rig.ctx.clone();
    let (fast, fast_task) = reader(&rig, Duration::ZERO).await;
    let (slow, slow_task) = reader(&rig, Duration::from_millis(2)).await;
    until_long("every symbol subscribed at the broker", || {
        ctx.websocket.instrument_count() >= 1000
    })
    .await;

    let published = Arc::new(Mutex::new(HashMap::new()));
    let probe = Arc::new(Probe {
        published: published.clone(),
        latencies: Mutex::new(Vec::new()),
        received: AtomicU64::new(0),
    });
    ctx.bus.subscribe(probe.clone(), Lane::Critical);

    let bus = ctx.bus.stats();
    let before = (
        bus.critical_dropped.load(Ordering::Relaxed),
        bus.critical_panics.load(Ordering::Relaxed),
        bus.closed_sends.load(Ordering::Relaxed),
        ctx.bus.gap_count(),
    );
    let shed_before: HashMap<&str, u64> = ctx
        .bus
        .queue_depths()
        .into_iter()
        .map(|q| (q.name, q.dropped))
        .collect();

    // The deepest each queue got, sampled every 20 ms.
    let deepest: Arc<Mutex<Deepest>> = Default::default();
    let sampler = {
        let (ctx, deepest) = (ctx.clone(), deepest.clone());
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(20));
            loop {
                tick.tick().await;
                for q in ctx.bus.queue_depths() {
                    let mut d = deepest.lock();
                    let e = d.entry(q.name).or_insert((0, q.capacity, q.lane));
                    e.0 = e.0.max(q.depth);
                }
            }
        })
    };

    let generation = ctx.bus.session_generation();
    let started = Instant::now();
    let mut seq = 0u64;
    for (rate, length) in SCHEDULE {
        let phase = Instant::now();
        let mut sent_in_phase = 0u64;
        let mut tick = tokio::time::interval(Duration::from_millis(1));
        while phase.elapsed() < length {
            tick.tick().await;
            let due = (phase.elapsed().as_secs_f64() * rate as f64) as u64;
            while sent_in_phase < due {
                published.lock().insert(seq, Instant::now());
                ctx.bus.publish(event(seq, generation));
                seq += 1;
                sent_in_phase += 1;
            }
        }
    }
    let flow = started.elapsed();
    let drained_at = Instant::now();
    until_long("every queue to drain", || {
        ctx.bus.queue_depths().iter().all(|q| q.depth == 0)
            && probe.received.load(Ordering::Relaxed) >= seq
            || drained_at.elapsed() > DRAIN_LIMIT
    })
    .await;
    let drain = drained_at.elapsed();
    sampler.abort();

    let critical_dropped = bus.critical_dropped.load(Ordering::Relaxed) - before.0;
    let critical_panics = bus.critical_panics.load(Ordering::Relaxed) - before.1;
    let closed_sends = bus.closed_sends.load(Ordering::Relaxed) - before.2;
    let fact_losses = ctx.bus.gap_count() - before.3;
    let received = probe.received.load(Ordering::Relaxed);
    let shed: Vec<String> = ctx
        .bus
        .queue_depths()
        .into_iter()
        .filter_map(|q| {
            let n = q.dropped - shed_before.get(q.name).copied().unwrap_or(0);
            (n > 0).then(|| format!("{}:{}", q.name, n))
        })
        .collect();
    let deepest_critical = deepest
        .lock()
        .iter()
        .filter(|(_, (_, _, lane))| *lane == Lane::Critical)
        .max_by_key(|(_, (d, _, _))| *d)
        .map(|(n, (d, c, _))| format!("{}:{}/{}", n, d, c))
        .unwrap_or_default();
    let latency = Stats::of(&probe.latencies.lock());

    let health = health(&rig).await;
    let (fast_n, slow_n) = (fast.load(Ordering::Relaxed), slow.load(Ordering::Relaxed));
    let reported = shown_in_health(&health);
    // The slow client's folded frames are what health must show.
    let shown = reported.is_some_and(|(folded, _, _)| folded > 0);
    let zero = critical_dropped == 0
        && critical_panics == 0
        && closed_sends == 0
        && fact_losses == 0
        && received == seq;
    let met = Report::new("event_burst")
        .field("normal_per_s", NORMAL_PER_S)
        .field("burst_per_s", BURST_PER_S)
        .field("published", seq)
        .field("flow_s", format!("{:.1}", flow.as_secs_f64()))
        .field("probe_received", received)
        .field("critical_dropped", critical_dropped)
        .field("critical_panics", critical_panics)
        .field("closed_sends", closed_sends)
        .field("fact_losses", fact_losses)
        .field("deepest_critical", deepest_critical)
        .field(
            "best_effort_shed",
            if shed.is_empty() {
                "none".into()
            } else {
                shed.join(",")
            },
        )
        .field("drain_s", format!("{:.2}", drain.as_secs_f64()))
        .stats("critical_delivery", &latency)
        .verdict("critical_zero_drops", zero);
    // Market data: what the slow client missed, against the one that read
    // everything, and whether health shows any of it.
    Report::new("market_data_drops")
        .field("fast_client_frames", fast_n)
        .field("slow_client_frames", slow_n)
        .field(
            "slow_client_missed_pct",
            format!(
                "{:.1}",
                100.0 * (1.0 - slow_n as f64 / fast_n.max(1) as f64)
            ),
        )
        .field("app_skipped", skipped.summary())
        .field(
            "health_feed_clients",
            reported
                .map(|(c, d, s)| format!("coalesced:{},dropped:{},skipped:{}", c, d, s))
                .unwrap_or_else(|| "absent".into()),
        )
        .verdict("counted_and_shown", shown);

    fast_task.abort();
    slow_task.abort();
    rig.stop().await;
    assert!(
        met,
        "critical delivery lost events: dropped {}, panics {}, closed {}, fact losses {}, probe {}/{}",
        critical_dropped, critical_panics, closed_sends, fact_losses, received, seq
    );
    assert!(
        shown,
        "health does not show what the slow app was not sent: {}",
        health["market_data"]
    );
}
