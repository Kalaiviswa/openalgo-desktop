//! The Latency, Traffic, Logs and Analyzer reports over a large log
//! (DB-03, DB-01, OBS-05): computed in SQL off the async workers, the
//! exports read and formatted off them (the latency export streamed), and
//! the analyzer's day statistics over every request of the day.

use crate::webui_support;

use axum::extract::ConnectInfo;
use axum::http::StatusCode;
use http_body_util::BodyExt;
use openalgo_desktop_lib::db::sqlite::monitor::{self as monitor_store, TrafficRow};
use openalgo_desktop_lib::services::monitor::Record;
use openalgo_desktop_lib::state::AppState;
use serde_json::json;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tower::ServiceExt;
use webui_support::*;

const ROWS: i64 = 200_000;

/// `ROWS` latency rows in one statement: three brokers, every tenth one
/// failed, all inside the last 30 days of the harness clock.
fn seed_latency(h: &H) {
    h.ctx
        .logs
        .conn()
        .unwrap()
        .execute(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?1)
             INSERT INTO order_latency (timestamp, order_id, broker, symbol, order_type, rtt_ms,
                 validation_latency_ms, response_latency_ms, overhead_ms, total_latency_ms, status)
             SELECT datetime('2026-10-05 04:00:00', '-' || (i % 2000000) || ' seconds'), i,
                 CASE i % 3 WHEN 0 THEN 'zerodha' WHEN 1 THEN 'dhan' ELSE 'fyers' END, 'SBIN',
                 'PLACE', (i % 397) * 1.0, 0, 0, i % 13, (i % 397) + (i % 13),
                 CASE WHEN i % 10 = 0 THEN 'FAILED' ELSE 'SUCCESS' END
             FROM n",
            [ROWS],
        )
        .unwrap();
}

type App = tower_http::normalize_path::NormalizePath<axum::Router>;
type Reply = (StatusCode, Vec<u8>);

/// The whole router, built once before anything is measured: the harness
/// builds the route table per request, which is the test's own work on the
/// runtime thread, not the server's.
fn app(h: &H) -> App {
    openalgo_desktop_lib::server::app(h.ctx.clone())
}

/// GET `path` through `app` as the signed-in browser `c`, body collected.
async fn fetch(h: &H, app: &App, c: &str, path: &str) -> Reply {
    let mut r = with(get(path), c, None);
    r.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40000))));
    crate::with_host(&mut r, &h.ctx);
    let resp = app.clone().oneshot(r).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, body.to_vec())
}

/// Four Latency page refreshes and an export, all at once.
async fn refreshes_and_export(h: &H, app: &App, c: &str) -> ([Reply; 4], Reply) {
    let stats = || fetch(h, app, c, "/latency/api/stats");
    let export = fetch(h, app, c, "/latency/export");
    let (a, b, d, e, x) = tokio::join!(stats(), stats(), stats(), stats(), export);
    ([a, b, d, e], x)
}

/// DB-03, DB-01: four Latency page refreshes and an export over 200,000
/// rows never hold the runtime. Before DB-03, each report loaded every row
/// and sorted it on the async worker (`runtime_stall` 7.5 s). Until the
/// export formatted its pages on the blocking pool, it spent 17 ms of the
/// async worker per page of 1,000 rows, 3.5 s in all (`runtime_busy`),
/// and a loaded CI runner stretched one page past 200 ms.
#[tokio::test]
async fn latency_reports_over_a_large_log_do_not_hold_the_runtime() {
    let (h, c, _) = signed_in();
    seed_latency(&h);
    let app = app(&h);
    let (((stats, (s, csv)), busy), worst) =
        crate::runtime_stall(crate::runtime_busy(refreshes_and_export(&h, &app, &c))).await;
    for (s, body) in stats {
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(s, StatusCode::OK, "{}", v);
        assert_eq!(v["total_orders"], ROWS);
        assert_eq!(v["failed_orders"], ROWS / 10);
        assert!(v["avg_overhead"].as_f64().unwrap() > 0.0);
        assert_eq!(v["broker_stats"]["dhan"]["total_orders"], 66_667);
    }
    assert_eq!(s, StatusCode::OK);
    let csv = String::from_utf8(csv).unwrap();
    assert_eq!(csv.lines().count() as i64, ROWS + 1, "header and every row");
    assert!(csv.starts_with("Date & Time (IST),Broker,Order ID"));
    assert!(
        worst < Duration::from_millis(200),
        "the runtime was held for {:?} at once",
        worst
    );
    assert!(
        busy < Duration::from_secs(1),
        "the reports kept the runtime busy for {:?} in all",
        busy
    );
}

/// Queue a traffic record and save the queue every 20 ms until `stop`, on
/// a thread of its own as the monitor's writer does; returns the slowest
/// save and how many records were sent. The harness runs no writer task,
/// so `drain_now` saves with the writer's own `save_batch`.
fn traffic_probe(ctx: Arc<AppState>, stop: Arc<AtomicBool>) -> JoinHandle<(Duration, i64)> {
    std::thread::spawn(move || {
        let (mut slowest, mut sent) = (Duration::ZERO, 0);
        while !stop.load(Ordering::Relaxed) {
            ctx.monitor.send(Record::Traffic(TrafficRow {
                timestamp: monitor_store::ts(ctx.now()),
                client_ip: "127.0.0.1".into(),
                method: "GET".into(),
                path: "/probe".into(),
                status_code: 200,
                duration_ms: 1.0,
                host: None,
                error: None,
            }));
            sent += 1;
            let t = Instant::now();
            ctx.monitor.drain_now(&ctx);
            slowest = slowest.max(t.elapsed());
            std::thread::sleep(Duration::from_millis(20));
        }
        (slowest, sent)
    })
}

/// Four Latency page refreshes and an export leave the logs database's
/// writers a connection: every traffic record saved meanwhile is written,
/// none waiting long. When each refresh computed its own report, the four
/// held all four of the logs pool's connections for the whole report
/// (seconds in a debug build): a save waited for one to finish, and failed
/// once that outlasted the pool's 10 s wait.
#[tokio::test]
async fn traffic_is_logged_while_latency_reports_run() {
    let (h, c, _) = signed_in();
    seed_latency(&h);
    let app = app(&h);
    let stop = Arc::new(AtomicBool::new(false));
    let probe = traffic_probe(h.ctx.clone(), stop.clone());
    let (stats, (s, _)) = refreshes_and_export(&h, &app, &c).await;
    stop.store(true, Ordering::Relaxed);
    let (slowest, sent) = probe.join().unwrap();
    assert!(stats.iter().all(|r| r.0 == StatusCode::OK));
    assert_eq!(s, StatusCode::OK);
    let saved: i64 = h
        .ctx
        .logs
        .conn()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM traffic_logs WHERE path = '/probe'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(saved, sent, "every traffic record saved");
    assert_eq!(h.ctx.monitor.lost(), 0);
    assert!(
        slowest < Duration::from_secs(1),
        "a traffic save waited {:?} for the logs database",
        slowest
    );
}

/// The requests the traffic log keeps at most (`monitor::RETENTION`).
const TRAFFIC_ROWS: i64 = 100_000;

/// `TRAFFIC_ROWS` requests in one statement, a second apart.
fn seed_traffic(h: &H) {
    h.ctx
        .logs
        .conn()
        .unwrap()
        .execute(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?1)
             INSERT INTO traffic_logs (timestamp, client_ip, method, path, status_code,
                 duration_ms, host, error)
             SELECT datetime('2026-10-05 04:00:00', '-' || i || ' seconds'), '127.0.0.1', 'POST',
                 '/api/v1/placeorder', CASE WHEN i % 10 = 0 THEN 400 ELSE 200 END,
                 (i % 97) * 1.5, '127.0.0.1:5500', NULL
             FROM n",
            [TRAFFIC_ROWS],
        )
        .unwrap();
}

/// DB-01: the Traffic page's export of a full log is read and formatted
/// off the async workers. Both ran on the async worker before, keeping
/// the runtime busy for seconds in a debug build.
#[tokio::test]
async fn traffic_export_of_a_full_log_does_not_hold_the_runtime() {
    let (h, c, _) = signed_in();
    seed_traffic(&h);
    let app = app(&h);
    let ((s, csv), busy) = crate::runtime_busy(fetch(&h, &app, &c, "/traffic/export")).await;
    assert_eq!(s, StatusCode::OK);
    let csv = String::from_utf8(csv).unwrap();
    assert!(csv.starts_with("Timestamp,Client IP,Method,Path"));
    assert!(
        csv.lines().count() as i64 > TRAFFIC_ROWS,
        "header and every request"
    );
    assert!(
        busy < Duration::from_millis(500),
        "the export kept the runtime busy for {:?}",
        busy
    );
}

/// Order log entries for the export test, all on the harness's day.
const ORDER_LOG_ROWS: i64 = 50_000;

/// `ORDER_LOG_ROWS` placed orders in one statement, within the last 8 hours.
fn seed_order_logs(h: &H) {
    h.ctx
        .logs
        .conn()
        .unwrap()
        .execute(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?1)
             INSERT INTO order_logs (api_type, request_data, response_data, created_at)
             SELECT 'placeorder',
                 json_object('strategy', 's1', 'exchange', 'NSE', 'symbol', 'SBIN',
                     'action', 'BUY', 'product', 'MIS', 'pricetype', 'MARKET', 'quantity', '1'),
                 json_object('status', 'success', 'orderid', 'ord' || i),
                 strftime('%Y-%m-%dT%H:%M:%SZ', '2026-10-05 04:00:00',
                     '-' || (i % 30000) || ' seconds')
             FROM n",
            [ORDER_LOG_ROWS],
        )
        .unwrap();
}

/// DB-01: the Logs page's export of a large day is read and formatted off
/// the async workers. Both ran on the async worker before (each entry's
/// request and response parsed there), keeping the runtime busy for
/// seconds in a debug build.
#[tokio::test]
async fn order_log_export_of_a_large_day_does_not_hold_the_runtime() {
    let (h, c, _) = signed_in();
    seed_order_logs(&h);
    let app = app(&h);
    let path = "/logs/export?start_date=2026-10-05&end_date=2026-10-05";
    let ((s, csv), busy) = crate::runtime_busy(fetch(&h, &app, &c, path)).await;
    assert_eq!(s, StatusCode::OK);
    let csv = String::from_utf8(csv).unwrap();
    assert!(csv.starts_with("ID,Timestamp,API Type,Strategy"));
    assert_eq!(
        csv.lines().count() as i64,
        ORDER_LOG_ROWS + 1,
        "header and every entry"
    );
    assert!(
        busy < Duration::from_millis(500),
        "the export kept the runtime busy for {:?}",
        busy
    );
}

/// OBS-05: the Analyzer page's day statistics count every request of the
/// last 24 hours, not only the newest 10,000 the page lists.
#[tokio::test]
async fn analyzer_day_statistics_count_every_request() {
    let (h, c, _) = signed_in();
    let newest = h.ctx.now() - chrono::Duration::hours(1);
    let at = |secs: i64| {
        (newest - chrono::Duration::seconds(secs))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string()
    };
    {
        let mut conn = h.ctx.logs.conn().unwrap();
        let tx = conn.transaction().unwrap();
        // The oldest request in the window carries the only rate-limit error.
        tx.execute(
            "INSERT INTO analyzer_logs (api_type, request_data, response_data, created_at)
             VALUES ('placeorder', ?1, ?2, ?3)",
            rusqlite::params![
                json!({"symbol": "OLDEST", "strategy": "early"}).to_string(),
                json!({"status": "error", "message": "Rate limit exceeded"}).to_string(),
                at(20_000)
            ],
        )
        .unwrap();
        let mut st = tx
            .prepare(
                "INSERT INTO analyzer_logs (api_type, request_data, response_data, created_at)
                 VALUES ('placeorder', ?1, ?2, ?3)",
            )
            .unwrap();
        let req = json!({"symbol": "SBIN", "strategy": "s1"}).to_string();
        let ok = json!({"status": "success"}).to_string();
        for i in 0..10_000 {
            st.execute(rusqlite::params![req, ok, at(i)]).unwrap();
        }
        drop(st);
        tx.commit().unwrap();
    }
    let (s, v) = h.json(with(get("/analyzer/api/data"), &c, None)).await;
    assert_eq!(s, StatusCode::OK, "{}", v);
    let stats = &v["data"]["stats"];
    assert_eq!(stats["total_requests"], 10_001, "{}", stats);
    assert_eq!(stats["issues"]["total"], 1);
    assert_eq!(stats["issues"]["by_type"]["rate_limit"], 1);
    assert_eq!(stats["symbols"], json!(["SBIN", "OLDEST"]));
    assert_eq!(stats["sources"], json!(["s1", "early"]));
}
