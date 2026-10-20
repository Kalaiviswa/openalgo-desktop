//! Tick-to-decision latency while report pages read a large log: the
//! Web UI's report routes must not hold the async runtime that carries the
//! feed and the risk owners ("Async-worker DB reads", roadmap).
//!
//! The rig of `tick_to_risk` (1,000 symbols, 2,000 ticks a second) with 90
//! scalping stops, over a log grown large: 100,000 requests in the traffic
//! log (its cap) and 200,000 order log entries. Four browsers keep
//! reloading the report pages (`PAGES`) meanwhile, each as soon as its last
//! answer came. The decision latency is the target's (p99 under 50 ms);
//! the pages' own times are reported beside it.

use super::browsers::{self, Page};
use super::lag_log;
use super::rig::Rig;
use super::tick_to_risk::{legs_per_kind, loaded_rig, manage_scalping_legs, measure, Owner};
use super::{Report, Stats};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const BROWSERS: usize = 4;
const TRAFFIC_ROWS: i64 = 100_000;
const ORDER_LOG_ROWS: i64 = 200_000;
/// The report pages a trader keeps open, as their pages call them.
const PAGES: &[&str] = &[
    "/traffic/api/logs?limit=1000",
    "/traffic/api/stats",
    "/logs?page=1",
    "/logs?page=1&search=SYM0001",
];

/// The large log, written in two statements.
fn seed(rig: &Rig) {
    let c = rig.ctx.logs.conn().unwrap();
    c.execute(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?1)
         INSERT INTO traffic_logs (timestamp, client_ip, method, path, status_code,
             duration_ms, host, error)
         SELECT datetime('2026-10-05 03:00:00', '-' || i || ' seconds'), '127.0.0.1', 'POST',
             CASE i % 4 WHEN 0 THEN '/api/v1/placeorder' WHEN 1 THEN '/api/v1/quotes'
                        WHEN 2 THEN '/api/v1/funds' ELSE '/dashboard' END,
             CASE WHEN i % 25 = 0 THEN 500 ELSE 200 END, (i % 97) * 0.5, '127.0.0.1:5000', NULL
         FROM n",
        [TRAFFIC_ROWS],
    )
    .unwrap();
    c.execute(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?1)
         INSERT INTO order_logs (api_type, request_data, response_data, created_at)
         SELECT 'placeorder',
             '{\"strategy\":\"perf\",\"symbol\":\"SYM' || printf('%04d', i % 1000) ||
                 '\",\"exchange\":\"NSE\",\"action\":\"BUY\",\"quantity\":\"1\"}',
             '{\"status\":\"success\",\"orderid\":\"' || i || '\"}',
             strftime('%Y-%m-%dT%H:%M:%SZ', '2026-10-05 03:00:00', '-' || i || ' seconds')
         FROM n",
        [ORDER_LOG_ROWS],
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn tick_to_decision_while_report_pages_read_a_large_log() {
    crate::isolated!(tick_to_decision_while_report_pages_read_a_large_log);
    let skipped = lag_log::install();
    let legs = legs_per_kind(30);
    let (rig, frames, reader) = loaded_rig(&legs).await;
    seed(&rig);
    manage_scalping_legs(&rig, &legs).await;
    let stop = Arc::new(AtomicBool::new(false));
    let pages = PAGES.iter().map(|p| Page::get(*p)).collect();
    let browsers = browsers::start(&rig, pages, BROWSERS, stop.clone());
    let met = measure(
        "tick_to_risk_under_report_load",
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
    let all_ok = seen.statuses.keys().all(|s| *s == 200);
    Report::new("report_pages_under_load")
        .field("browsers", BROWSERS)
        .field("traffic_rows", TRAFFIC_ROWS)
        .field("order_log_rows", ORDER_LOG_ROWS)
        .field("statuses", seen.status_summary())
        .stats("page", &Stats::of(&seen.took))
        .verdict("every_page_200", all_ok);
    assert!(all_ok, "a report page failed: {}", seen.status_summary());
    assert!(
        met,
        "tick to decision p99 over 50 ms, or a decision lost, while the report pages load"
    );
}
