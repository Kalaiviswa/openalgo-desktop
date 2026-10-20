//! Target: broker order-socket reconnect to book reconciled, under 5 s.
//!
//! Eight live scalping legs each have an exit order working at the broker.
//! For each sample the broker's order socket is dropped without a close
//! frame (a network drop) and, while it is down, one exit fills and its
//! position goes flat at the broker: the update frame that would have said
//! so is lost with the socket. Timed from the drop: the order socket back
//! (`connected`), the reconciler's gap for the reconnect closed after it
//! read the order book (`gap_closed`), and the leg cleared because the
//! book showed its exit complete and the position flat (`reconciled`, the
//! target). The broker feed streams 2,000 ticks a second over 1,000
//! symbols throughout.
//!
//! The manager resets its reconnect backoff only after a connection that
//! lived 30 s, so the samples are 31 s apart, as drops on a working line
//! are. Three drops 1 s after each reconnect follow, to show what a
//! flapping line costs (recorded, not held to the target).

use super::rig::{until_long, Rig, RigOptions, BASE_PRICE};
use super::{ms, Report, Stats};
use openalgo_desktop_lib::brokers::mock::MockCall;
use openalgo_desktop_lib::brokers::types::Order;
use openalgo_desktop_lib::clock::Clock;
use openalgo_desktop_lib::scalping::store::SlUpsert;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const LEGS: usize = 8;
const SAMPLES: usize = 6;
const STABLE: Duration = Duration::from_secs(31);
const FLAPS: usize = 3;
const TARGET: Duration = Duration::from_secs(5);
const GIVE_UP: Duration = Duration::from_secs(60);

fn symbol(i: usize) -> String {
    format!("SYM{:04}", i)
}

fn exit_row(order_id: &str, symbol: &str, filled: bool, at: &str) -> Order {
    Order {
        order_id: order_id.into(),
        exchange_order_id: None,
        symbol: symbol.into(),
        exchange: "NSE".into(),
        side: "SELL".into(),
        quantity: 1,
        filled_quantity: i32::from(filled),
        pending_quantity: i32::from(!filled),
        price: 0.0,
        trigger_price: 0.0,
        average_price: if filled { BASE_PRICE - 10.0 } else { 0.0 },
        order_type: "MARKET".into(),
        product: "MIS".into(),
        status: if filled { "complete" } else { "open" }.into(),
        validity: "DAY".into(),
        order_timestamp: at.into(),
        exchange_timestamp: None,
        rejection_reason: None,
        order_tag: None,
    }
}

/// One drop: (connected, gap closed, reconciled) after it.
struct Timeline {
    connected: Option<Duration>,
    gap_closed: Option<Duration>,
    reconciled: Option<Duration>,
}

/// Drop the order socket; while it is down, fill leg `filled` (if any) at
/// the broker without telling the app; follow what happens.
async fn drop_socket(rig: &Rig, book: &mut [Order], filled: Option<usize>) -> Timeline {
    let ctx = &rig.ctx;
    let connects = ctx.runtime.order_ws.stats().connects.load(Ordering::SeqCst);
    let gaps = ctx.reconciler.stats.gaps_closed.load(Ordering::SeqCst);
    let t0 = Instant::now();
    rig.orders.kick();
    if let Some(i) = filled {
        book[i].status = "complete".into();
        book[i].filled_quantity = 1;
        book[i].pending_quantity = 0;
        book[i].average_price = BASE_PRICE - 10.0;
        *rig.mock.order_book.lock() = Some(Ok(book.to_vec()));
        let held: Vec<(String, i32)> = (0..LEGS)
            .map(|j| (symbol(j), i32::from(book[j].status != "complete")))
            .collect();
        rig.set_positions(&held);
    }
    let mut t = Timeline {
        connected: None,
        gap_closed: None,
        reconciled: None,
    };
    let mut tick = tokio::time::interval(Duration::from_millis(1));
    while t0.elapsed() < GIVE_UP {
        tick.tick().await;
        let ws = &ctx.runtime.order_ws;
        if t.connected.is_none()
            && ws.stats().connects.load(Ordering::SeqCst) > connects
            && ws.is_connected()
        {
            t.connected = Some(t0.elapsed());
        }
        if t.gap_closed.is_none() && ctx.reconciler.stats.gaps_closed.load(Ordering::SeqCst) > gaps
        {
            t.gap_closed = Some(t0.elapsed());
        }
        if let Some(i) = filled {
            let gone = ctx
                .scalping
                .monitor
                .leg("live", &symbol(i), "NSE", "MIS")
                .is_none();
            if t.reconciled.is_none() && gone {
                t.reconciled = Some(t0.elapsed());
            }
        }
        let leg_done = filled.is_none() || t.reconciled.is_some();
        if t.connected.is_some() && t.gap_closed.is_some() && leg_done {
            break;
        }
    }
    t
}

fn ms_or_none(d: Option<Duration>) -> String {
    d.map(ms).unwrap_or_else(|| "none".into())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn order_socket_reconnect_to_book_reconciled_under_5s() {
    crate::isolated!(order_socket_reconnect_to_book_reconciled_under_5s);
    let rig = Rig::start(RigOptions {
        symbols: 1000,
        tick_rate: 2000,
        quiet: vec![],
        trend_per_s: 0.0,
    })
    .await;
    let connected_at = Instant::now();
    let ctx = rig.ctx.clone();

    // Eight live legs, each with its exit working at the broker.
    let held: Vec<(String, i32)> = (0..LEGS).map(|i| (symbol(i), 1)).collect();
    rig.set_positions(&held);
    for i in 0..LEGS {
        ctx.scalping
            .store
            .upsert_sl(&SlUpsert {
                symbol: symbol(i),
                exchange: "NSE".into(),
                product: "MIS".into(),
                mode: "live".into(),
                side: Some("BUY".into()),
                entry_price: Some(BASE_PRICE),
                quantity: Some(1),
                initial_sl: Some(BASE_PRICE - 10.0),
                is_active: Some(true),
                ..Default::default()
            })
            .unwrap();
    }
    ctx.scalping.monitor.request_sync();
    until_long("every leg managed", || {
        ctx.scalping.monitor.leg_count() == LEGS
    })
    .await;
    for i in 0..LEGS {
        ctx.scalping
            .monitor
            .process_tick(&symbol(i), "NSE", BASE_PRICE - 15.0)
            .await;
    }
    ctx.scalping.monitor.wait_idle().await;
    let placed = rig
        .mock
        .calls()
        .iter()
        .filter(|c| matches!(c, MockCall::PlaceOrder(_)))
        .count();
    assert_eq!(placed, LEGS, "every leg sent its exit");
    let at = rig
        .clock
        .now()
        .with_timezone(&chrono_tz::Asia::Kolkata)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let mut book: Vec<Order> = (0..LEGS)
        .map(|i| {
            let a = ctx
                .scalping
                .monitor
                .exit_attempt("live", &symbol(i), "NSE", "MIS")
                .expect("an exit attempt");
            let id = a.children[0].order_id.clone().expect("an order id");
            exit_row(&id, &symbol(i), false, &at)
        })
        .collect();
    *rig.mock.order_book.lock() = Some(Ok(book.clone()));

    let mut samples = Vec::new();
    for i in 0..SAMPLES {
        // The socket's current connection is this old.
        let since = samples.last().map(|(at, _)| *at).unwrap_or(connected_at);
        tokio::time::sleep(STABLE.saturating_sub(since.elapsed())).await;
        let t = drop_socket(&rig, &mut book, Some(i)).await;
        eprintln!(
            "reconnect {}: connected {} ms, gap closed {} ms, reconciled {} ms",
            i,
            ms_or_none(t.connected),
            ms_or_none(t.gap_closed),
            ms_or_none(t.reconciled)
        );
        samples.push((Instant::now(), t));
    }
    let mut flaps = Vec::new();
    for _ in 0..FLAPS {
        tokio::time::sleep(Duration::from_secs(1)).await;
        flaps.push(drop_socket(&rig, &mut book, None).await);
    }

    let take = |f: fn(&Timeline) -> Option<Duration>| -> Vec<Duration> {
        samples.iter().filter_map(|(_, t)| f(t)).collect()
    };
    let reconciled = take(|t| t.reconciled);
    let missing = SAMPLES - reconciled.len();
    let worst = Stats::of(&reconciled).max;
    let flap_connected: Vec<String> = flaps.iter().map(|t| ms_or_none(t.connected)).collect();
    let met = Report::new("reconnect")
        .field("samples", SAMPLES)
        .stats("connected", &Stats::of(&take(|t| t.connected)))
        .stats("gap_closed", &Stats::of(&take(|t| t.gap_closed)))
        .stats("reconciled", &Stats::of(&reconciled))
        .field("not_reconciled", missing)
        .field("flapping_connected_ms", flap_connected.join(","))
        .verdict("reconciled_max_ms<5000", missing == 0 && worst < TARGET);
    rig.stop().await;
    assert!(
        met,
        "order socket reconnect to book reconciled took up to {} ms ({} not reconciled)",
        ms(worst),
        missing
    );
}
