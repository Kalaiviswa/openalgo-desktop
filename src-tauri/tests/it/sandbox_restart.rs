//! What the sandbox fixes leave behind when the app restarts: each test runs
//! on a `sandbox.db` file, closes the sandbox (engine and event bus) and
//! opens a new one on the same file, as the app does across a restart.

use crate::sandbox_support;

use openalgo_desktop_lib::sandbox::{BroadcastTicks, SandboxOptions, SymbolKey, Tick};
use rust_decimal::Decimal;
use sandbox_support::*;
use std::sync::Arc;
use std::time::Duration;

fn opts() -> SandboxOptions {
    SandboxOptions::default()
}

async fn open_orders(env: &Env) -> Vec<(String, String, i64)> {
    env.sb
        .orderbook()
        .await
        .unwrap()
        .data
        .orders
        .iter()
        .filter(|o| o.order_status == "open" || o.order_status == "trigger pending")
        .map(|o| (o.orderid.clone(), o.action.clone(), o.quantity))
        .collect()
}

// SB-02: a fill that could not record its trade stays rejected, with its
// margin returned, after a restart; once trades can be written again the
// next order fills with exactly one trade.
#[tokio::test]
async fn a_rolled_back_fill_stays_rejected_across_a_restart() {
    let env = Env::on_disk("2026-10-05 10:00:00", opts());
    env.ltp("SBIN", "NSE", "100");
    env.sb
        .db()
        .with_conn(|c| {
            c.execute_batch(
                "CREATE TRIGGER refuse_trades BEFORE INSERT ON sandbox_trades
                 BEGIN SELECT RAISE(ABORT, 'trade refused'); END;",
            )
        })
        .unwrap();
    let id = place(&env, req("SBIN", "NSE", "BUY", 10, "MARKET", "MIS")).await;
    assert_eq!(env.status(&id).await, "rejected");

    let env = env.restart(opts()).await;
    assert_eq!(env.status(&id).await, "rejected");
    assert_eq!(env.qty("SBIN", "NSE", "MIS").await, 0);
    assert_eq!(env.used().await, Decimal::ZERO);
    assert_eq!(env.available().await, d("10000000"));
    assert!(env.sb.tradebook().await.unwrap().data.is_empty());
    env.assert_margin_consistent().await;

    env.sb
        .db()
        .with_conn(|c| c.execute_batch("DROP TRIGGER refuse_trades;"))
        .unwrap();
    let ok = place(&env, req("SBIN", "NSE", "BUY", 10, "MARKET", "MIS")).await;
    assert_eq!(env.status(&ok).await, "complete");
    let trades = env.sb.tradebook().await.unwrap().data;
    assert_eq!(trades.len(), 1);
    assert_eq!(trades[0].orderid, ok);
    env.assert_margin_consistent().await;
    env.shutdown().await;
}

// SB-05: a close waiting for a price is in sandbox.db. After a restart,
// close-all and the square-off sweep still find it and place no second exit;
// it fills when prices return and the position ends flat with no margin.
#[tokio::test]
async fn a_pending_close_survives_a_restart_and_is_not_placed_twice() {
    let env = Env::on_disk("2026-10-05 10:00:00", opts());
    env.ltp("SBIN", "NSE", "100");
    place(&env, req("SBIN", "NSE", "BUY", 10, "MARKET", "MIS")).await;
    env.quotes.remove("SBIN", "NSE");
    env.sb.close_all_positions().await.unwrap();
    let before = open_orders(&env).await;
    assert_eq!(before.len(), 1);

    let env = env.restart(opts()).await;
    let v = serde_json::to_value(env.sb.close_all_positions().await.unwrap()).unwrap();
    assert_eq!(v["closed_positions"], 0, "{v}");
    env.set_time("2026-10-05 15:15:05");
    let sweep = env.sb.square_off_now().await.unwrap();
    assert_eq!((sweep.closed_positions, sweep.pending_closures), (0, 1));
    assert_eq!(open_orders(&env).await, before, "the same single close");

    env.ltp("SBIN", "NSE", "101");
    env.sb.poll_once().await.unwrap();
    assert_eq!(env.status(&before[0].0).await, "complete");
    assert_eq!(env.qty("SBIN", "NSE", "MIS").await, 0);
    assert_eq!(env.used().await, Decimal::ZERO);
    env.assert_margin_consistent().await;
    env.shutdown().await;
}

// SB-05: margin blocked when an exit reopened a position is held by that
// position in sandbox.db, and the books still agree after a restart.
#[tokio::test]
async fn margin_blocked_at_fill_is_kept_across_a_restart() {
    let env = Env::on_disk("2026-10-05 10:00:00", opts());
    env.ltp("SBIN", "NSE", "100");
    place(&env, req("SBIN", "NSE", "BUY", 10, "MARKET", "MIS")).await;
    place(
        &env,
        trigger(req("SBIN", "NSE", "SELL", 10, "SL-M", "MIS"), "95"),
    )
    .await;
    place(
        &env,
        limit(req("SBIN", "NSE", "SELL", 10, "LIMIT", "MIS"), "105"),
    )
    .await;
    env.sb
        .on_tick(Tick::new("SBIN", "NSE", d("105")))
        .await
        .unwrap();
    env.sb
        .on_tick(Tick::new("SBIN", "NSE", d("95")))
        .await
        .unwrap();
    assert_eq!(env.qty("SBIN", "NSE", "MIS").await, -10);

    let env = env.restart(opts()).await;
    let p = env
        .sb
        .position_row("SBIN", "NSE", "MIS")
        .await
        .unwrap()
        .unwrap();
    assert_eq!((p.quantity, p.margin_blocked), (-10, d("190")));
    assert_eq!(env.used().await, d("190"));
    env.assert_margin_consistent().await;
    env.shutdown().await;
}

// SB-03: a crypto position's margin (contract value included) is what the
// funds hold after a restart, and the books still agree.
#[tokio::test]
async fn crypto_margin_and_trade_value_hold_across_a_restart() {
    let env = Env::on_disk("2026-10-05 10:00:00", opts());
    env.ltp("BTCUSD.P", "CRYPTO", "60000");
    place(&env, req("BTCUSD.P", "CRYPTO", "BUY", 10, "MARKET", "NRML")).await;
    assert_eq!(env.used().await, d("60"));

    let env = env.restart(opts()).await;
    assert_eq!(env.used().await, d("60"));
    env.assert_margin_consistent().await;
    assert_eq!(env.sb.tradebook().await.unwrap().data[0].trade_value, 600.0);
    env.ltp("BTCUSD.P", "CRYPTO", "60100");
    assert_eq!(
        env.sb.positionbook().await.unwrap().data[0].unrealized_pnl,
        1.0
    );
    env.shutdown().await;
}

// SB-01 and SB-06: freshness and receipt times live in memory. After a
// restart the engine starts each symbol's quiet period afresh, ignores a
// price received long before, and polls a symbol whose feed never ticks.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restarted_engine_polls_a_quiet_symbol_and_ignores_old_prices() {
    let quick = || SandboxOptions {
        stale_feed_after: Duration::from_millis(300),
        ..SandboxOptions::default()
    };
    let env = Env::on_disk("2026-10-05 10:00:00", quick());
    env.sb
        .update_config("order_check_interval", "1")
        .await
        .unwrap();
    env.ltp("SBIN", "NSE", "100");
    let id = place(
        &env,
        limit(req("SBIN", "NSE", "BUY", 10, "LIMIT", "MIS"), "95"),
    )
    .await;

    let env = env.restart(quick()).await;
    let old = Tick {
        received_at: std::time::Instant::now()
            .checked_sub(Duration::from_secs(60))
            .expect("uptime"),
        ..Tick::new("SBIN", "NSE", d("94"))
    };
    assert_eq!(env.sb.on_tick(old).await.unwrap(), (0, 0));
    assert_eq!(env.status(&id).await, "open");

    let ticks = Arc::new(BroadcastTicks::new(64));
    env.sb.start_engine(ticks.clone()).await.unwrap();
    env.ltp("SBIN", "NSE", "94");
    let started = std::time::Instant::now();
    while env.status(&id).await != "complete" {
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "the restarted engine did not poll SBIN"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(ticks.resubscribed(&SymbolKey::new("SBIN", "NSE")) >= 1);
    env.assert_margin_consistent().await;
    env.shutdown().await;
}

// Crypto weekend accounting: snapshots written before a restart stay, and
// the catch-up that runs when analyzer mode starts after the restart fills
// in the Sunday the app missed.
#[tokio::test]
async fn crypto_weekend_snapshots_and_backfill_across_a_restart() {
    let env = Env::on_disk("2026-10-09 10:00:00", opts());
    env.ltp("BTCUSD.P", "CRYPTO", "60000");
    place(&env, req("BTCUSD.P", "CRYPTO", "BUY", 10, "MARKET", "NRML")).await;
    env.ltp("BTCUSD.P", "CRYPTO", "60100");
    place(&env, req("BTCUSD.P", "CRYPTO", "SELL", 5, "MARKET", "NRML")).await;
    env.sb.run_due_jobs().await.unwrap();
    for at in ["2026-10-09 23:59:10", "2026-10-10 23:59:10"] {
        env.set_time(at);
        env.sb.run_due_jobs().await.unwrap();
    }

    env.set_time("2026-10-12 09:00:00");
    let env = env.restart(opts()).await;
    let ticks: Arc<dyn openalgo_desktop_lib::sandbox::TickSource> =
        Arc::new(BroadcastTicks::new(16));
    env.sb.set_analyzer_mode(true, ticks).await.unwrap();
    let mut dates: Vec<String> = env
        .sb
        .mypnl()
        .await
        .unwrap()
        .data
        .daily_pnl
        .iter()
        .map(|r| r.date.clone())
        .collect();
    dates.sort();
    assert_eq!(dates, vec!["2026-10-09", "2026-10-10", "2026-10-11"]);
    assert_eq!(env.qty("BTCUSD.P", "CRYPTO", "NRML").await, 5);
    env.shutdown().await;
}
