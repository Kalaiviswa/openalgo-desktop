//! Tick-to-decision latency while the trading pages poll over a long
//! history: the reads outside the report pages must not hold the async
//! runtime that carries the feed and the risk owners either (follow-up of
//! "Async-worker DB reads").
//!
//! The rig of `tick_to_risk` (1,000 symbols, 2,000 ticks a second) with 90
//! scalping stops, over a year of history: a strategy with 25,000 orders
//! over 250 runs, an OpenScript deployment with 25,000 orders, 10,000
//! Action Center orders, 500,000 analyzer log entries and 100,000 Telegram
//! and WhatsApp commands. Four browsers keep reloading what the pages poll
//! (`pages`): the strategy's three books (every 5 s per open Detail page),
//! the deployment's positions and orders (every 15 s per running
//! deployment), the Action Center (on every order event), the analyzer
//! status an SDK or MCP client asks for, and the bots' statistics.

use super::browsers::{self, Page};
use super::lag_log;
use super::rig::Rig;
use super::tick_to_risk::{
    legs_per_kind, loaded_rig, manage_scalping_legs, measure, strategy_config, Kind, Owner,
};
use super::{Report, Stats};
use openalgo_desktop_lib::trading::runner::store::deployment_id;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const BROWSERS: usize = 4;
const STRATEGY_RUNS: i64 = 250;
const STRATEGY_ORDERS: i64 = 25_000;
const RUNNER_ORDERS: i64 = 25_000;
const PENDING_ORDERS: i64 = 10_000;
const ANALYZER_LOGS: i64 = 500_000;
const BOT_COMMANDS: i64 = 100_000;

/// A strategy of alice's with `STRATEGY_ORDERS` orders over
/// `STRATEGY_RUNS` finished live runs; its id.
fn seed_strategy(rig: &Rig) -> i64 {
    let (row, _) = rig
        .ctx
        .strategy
        .store
        .create_strategy("alice", &strategy_config("SBIN", Kind::Stop))
        .unwrap();
    let c = rig.ctx.sqlite.conn().unwrap();
    c.execute(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?2)
         INSERT INTO sm_strategy_run (strategy_id, mode, broker, started_at, stopped_at,
             stop_reason, pnl_realized)
         SELECT ?1, 'live', 'zerodha', datetime('now', '-' || i || ' days'),
             datetime('now', '-' || i || ' days', '+6 hours'), 'manual', (i % 7) * 10.0
         FROM n",
        rusqlite::params![row.id, STRATEGY_RUNS],
    )
    .unwrap();
    let first: i64 = c
        .query_row(
            "SELECT MIN(id) FROM sm_strategy_run WHERE strategy_id = ?1",
            [row.id],
            |r| r.get(0),
        )
        .unwrap();
    c.execute(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?1)
         INSERT INTO sm_strategy_order (run_id, leg_id, kind, broker_order_id, symbol,
             exchange, action, qty, product, pricetype, status, placed_at, filled_at,
             avg_fill_price, filled_qty)
         SELECT ?2 + i % ?3, 1, CASE i % 2 WHEN 0 THEN 'entry' ELSE 'exit' END, 'SM' || i,
             'SBIN', 'NSE', CASE i % 2 WHEN 0 THEN 'BUY' ELSE 'SELL' END, 1, 'CNC',
             'MARKET', 'complete', datetime('now', '-' || i || ' minutes'),
             datetime('now', '-' || i || ' minutes'), 1000.0, 1
         FROM n",
        rusqlite::params![STRATEGY_ORDERS, first, STRATEGY_RUNS],
    )
    .unwrap();
    row.id
}

/// A deployment (sandbox) with `RUNNER_ORDERS` orders; its id.
fn seed_deployment(rig: &Rig) -> String {
    let id = deployment_id("perf.py", "SBIN", "NSE", "5m", "a1b2c3");
    let c = rig.ctx.sqlite.conn().unwrap();
    c.execute(
        "INSERT INTO openscript_deployments (deployment, script, symbol, exchange, interval,
             product, user_id, mode, updated_at)
         VALUES (?1, 'perf.py', 'SBIN', 'NSE', '5m', 'MIS', 'alice', 'sandbox',
             datetime('now'))",
        [&id],
    )
    .unwrap();
    c.execute(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?2)
         INSERT INTO openscript_orders (deployment, mode, tag, symbol, exchange, action,
             quantity, pricetype, product, orderid, status, filled_quantity, average_price,
             is_exit, placed_at, updated_at, broker, account)
         SELECT ?1, 'sandbox', ?1, 'SBIN', 'NSE', CASE i % 2 WHEN 0 THEN 'BUY' ELSE 'SELL' END,
             1, 'MARKET', 'MIS', 'OS' || i, 'complete', 1, 1000.0, i % 2,
             datetime('now', '-' || i || ' minutes'), datetime('now', '-' || i || ' minutes'),
             'sandbox', ''
         FROM n",
        rusqlite::params![id, RUNNER_ORDERS],
    )
    .unwrap();
    id
}

/// Alice's Action Center orders, every tenth still pending.
fn seed_pending(rig: &Rig) {
    rig.ctx
        .sqlite
        .conn()
        .unwrap()
        .execute(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?1)
             INSERT INTO pending_orders (user_id, api_type, order_data, created_at,
                 created_at_ist, status)
             SELECT 'alice', 'placeorder',
                 json_object('strategy', 'perf', 'symbol', 'SBIN', 'exchange', 'NSE',
                     'action', 'BUY', 'quantity', '1', 'pricetype', 'MARKET', 'product', 'MIS'),
                 strftime('%Y-%m-%dT%H:%M:%SZ', 'now', '-' || i || ' minutes'),
                 strftime('%d-%m-%Y %I:%M:%S %p', 'now', '-' || i || ' minutes'),
                 CASE WHEN i % 10 = 0 THEN 'pending' ELSE 'approved' END
             FROM n",
            [PENDING_ORDERS],
        )
        .unwrap();
}

/// The analyzer log (never purged, as on the web) and both bots' command
/// logs, spread over the last week.
fn seed_logs(rig: &Rig) {
    rig.ctx
        .logs
        .conn()
        .unwrap()
        .execute(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?1)
             INSERT INTO analyzer_logs (api_type, request_data, response_data, created_at)
             SELECT 'placeorder', '{\"symbol\":\"SBIN\",\"strategy\":\"perf\"}',
                 '{\"status\":\"success\"}',
                 strftime('%Y-%m-%dT%H:%M:%SZ', 'now', '-' || i || ' seconds')
             FROM n",
            [ANALYZER_LOGS],
        )
        .unwrap();
    let c = rig.ctx.sqlite.conn().unwrap();
    c.execute(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?1)
         INSERT INTO command_logs (telegram_id, command, chat_id, parameters, executed_at)
         SELECT 1000 + i % 50, CASE i % 3 WHEN 0 THEN 'orderbook' WHEN 1 THEN 'positions'
                 ELSE 'funds' END, 1, NULL,
             datetime('now', '-' || (i * 6) || ' seconds')
         FROM n",
        [BOT_COMMANDS],
    )
    .unwrap();
    c.execute(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?1)
         INSERT INTO whatsapp_command_logs (whatsapp_jid, command, parameters, executed_at)
         SELECT (9100000000 + i % 50) || '@s.whatsapp.net', CASE i % 3 WHEN 0 THEN 'orderbook'
                 WHEN 1 THEN 'positions' ELSE 'funds' END, NULL,
             datetime('now', '-' || (i * 6) || ' seconds')
         FROM n",
        [BOT_COMMANDS],
    )
    .unwrap();
}

/// What the pages poll, as they ask for it.
fn pages(strategy: i64, deployment: &str) -> Vec<Page> {
    let s = format!("/strategy/api/strategies/{}", strategy);
    vec![
        Page::get(format!("{}/orderbook", s)),
        Page::get(format!("{}/tradebook", s)),
        Page::get(format!("{}/positions", s)),
        Page::get(format!("/openscript/runner/positions/{}", deployment)),
        Page::get(format!("/openscript/runner/orderbook/{}", deployment)),
        Page::get("/action-center/api/data"),
        Page::ApiPost("/api/v1/analyzer".into()),
        Page::get("/telegram/api/index"),
        Page::get("/whatsapp/stats?days=30"),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn tick_to_decision_while_trading_pages_poll_a_long_history() {
    crate::isolated!(tick_to_decision_while_trading_pages_poll_a_long_history);
    let skipped = lag_log::install();
    let legs = legs_per_kind(30);
    let (rig, frames, reader) = loaded_rig(&legs).await;
    let strategy = seed_strategy(&rig);
    let deployment = seed_deployment(&rig);
    seed_pending(&rig);
    seed_logs(&rig);
    manage_scalping_legs(&rig, &legs).await;
    let pages = pages(strategy, &deployment);
    // Each page answers over the seeded history (its status is reported:
    // a book the synthetic broker or the stopped sandbox cannot give is
    // still read locally first).
    for p in &pages {
        let t = std::time::Instant::now();
        let (status, _) = browsers::fetch_json(&rig, p).await;
        eprintln!(
            "trading page {:?}: {} in {} ms alone",
            p,
            status,
            super::ms(t.elapsed())
        );
    }
    let stop = Arc::new(AtomicBool::new(false));
    let browsers = browsers::start(&rig, pages, BROWSERS, stop.clone());
    let met = measure(
        "tick_to_risk_under_trading_pages",
        &skipped,
        rig,
        Owner::Scalping,
        legs,
        frames,
    )
    .await;
    stop.store(true, Ordering::Relaxed);
    let seen = browsers::collect(browsers).await;
    reader.abort();
    Report::new("trading_pages_under_load")
        .field("browsers", BROWSERS)
        .field("strategy_orders", STRATEGY_ORDERS)
        .field("runner_orders", RUNNER_ORDERS)
        .field("pending_orders", PENDING_ORDERS)
        .field("analyzer_logs", ANALYZER_LOGS)
        .field("bot_commands", BOT_COMMANDS)
        .field("statuses", seen.status_summary())
        .stats("page", &Stats::of(&seen.took))
        .verdict("recorded", true);
    assert!(
        met,
        "tick to decision p99 over 50 ms, or a decision lost, while the trading pages poll"
    );
}
