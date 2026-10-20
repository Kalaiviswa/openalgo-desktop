//! Target: disk growth. Log retention matches the web (the order and
//! analyzer logs are kept forever, maintainer decision 2026-10-15), so the
//! bound is the monitoring tables: each stays at its retention cap however
//! much is written, and `logs.db` stops growing once its free pages are
//! reused. The data folder's growth must be reported in health.
//!
//! Each cycle writes more than every capped table holds (request log,
//! server errors, health samples, order latency), runs the monitor's
//! housekeeping as the app does every minute, then measures the rows and
//! the file. A day passes between cycles.

use super::{folder_bytes, Report};
use crate::webui_support::{get, signed_in, with};
use openalgo_desktop_lib::db::sqlite::monitor::{
    self as store, ErrorEntry, HealthRow, LatencyRow, TrafficRow, LATENCY_ROW_CAP, RETENTION,
};
use openalgo_desktop_lib::services::monitor;
use std::path::Path;

const CYCLES: usize = 6;
/// Rows written per cycle: 20% over each cap.
const OVER: f64 = 1.2;
/// The file may be this much larger at the end than after the first full
/// cycle that reached the high-water mark (cycle 2).
const PLATEAU: f64 = 1.05;

fn cap(table: &str) -> i64 {
    RETENTION
        .iter()
        .find(|(t, ..)| *t == table)
        .map(|r| r.3)
        .unwrap_or_else(|| panic!("{} has no retention", table))
}

/// `logs.db` with its WAL and shared memory, in bytes.
fn logs_bytes(dir: &Path) -> u64 {
    ["logs.db", "logs.db-wal", "logs.db-shm"]
        .iter()
        .filter_map(|f| std::fs::metadata(dir.join(f)).ok())
        .map(|m| m.len())
        .sum()
}

fn write_cycle(conn: &mut rusqlite::Connection, stamp: &str, cycle: usize) {
    let n = |table: &str| (cap(table) as f64 * OVER) as usize;
    let tx = conn.transaction().unwrap();
    for i in 0..n("traffic_logs") {
        store::insert_traffic(
            &tx,
            &TrafficRow {
                timestamp: stamp.into(),
                client_ip: "127.0.0.1".into(),
                method: "POST".into(),
                path: "/api/v1/quotes".into(),
                status_code: 200,
                duration_ms: 1.5,
                host: Some("127.0.0.1:5000".into()),
                error: (i % 50 == 0).then(|| "timeout".into()),
            },
        )
        .unwrap();
    }
    for i in 0..n("error_logs") {
        store::insert_error(
            &tx,
            &ErrorEntry {
                ts: stamp.into(),
                level: "ERROR".into(),
                logger: Some("openalgo".into()),
                module: Some("perf".into()),
                file: None,
                message: format!("cycle {} error {}", cycle, i),
                exception: None,
                request: None,
            },
        )
        .unwrap();
    }
    for _ in 0..n("health_metrics") {
        store::insert_health(
            &tx,
            &HealthRow {
                timestamp: stamp.into(),
                fd_count: Some(40),
                memory_rss_mb: Some(180.0),
                overall_status: Some("pass".into()),
                ..Default::default()
            },
        )
        .unwrap();
    }
    for i in 0..(LATENCY_ROW_CAP as f64 * OVER) as usize {
        store::insert_latency(
            &tx,
            &LatencyRow {
                timestamp: stamp.into(),
                order_id: format!("{}-{}", cycle, i),
                broker: Some("zerodha".into()),
                symbol: Some("SBIN".into()),
                order_type: "PLACE".into(),
                rtt_ms: 40.0,
                total_latency_ms: 45.0,
                status: "SUCCESS".into(),
                ..Default::default()
            },
        )
        .unwrap();
    }
    tx.commit().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn monitoring_tables_stay_bounded_and_growth_is_reported() {
    crate::isolated!(monitoring_tables_stay_bounded_and_growth_is_reported);
    let (h, cookie, _) = signed_in();
    let dir = h.ctx.data_dir.clone();
    let capped = [
        "traffic_logs",
        "error_logs",
        "health_metrics",
        "order_latency",
    ];

    let mut sizes = Vec::new();
    let mut over_cap = Vec::new();
    let started = std::time::Instant::now();
    for cycle in 0..CYCLES {
        let stamp = store::ts(h.ctx.now());
        {
            let mut conn = h.ctx.logs.conn().unwrap();
            write_cycle(&mut conn, &stamp, cycle);
        }
        monitor::housekeep(&h.ctx);
        let conn = h.ctx.logs.conn().unwrap();
        for table in capped {
            let rows = store::count(&conn, table).unwrap();
            let limit = if table == "order_latency" {
                LATENCY_ROW_CAP
            } else {
                cap(table)
            };
            if rows > limit {
                over_cap.push(format!("{} {} > {} (cycle {})", table, rows, limit, cycle));
            }
        }
        drop(conn);
        sizes.push(logs_bytes(&dir));
        eprintln!(
            "disk: cycle {} logs.db {} KiB, data folder {} KiB",
            cycle,
            sizes[cycle] / 1024,
            folder_bytes(&dir) / 1024
        );
        h.clock.advance(chrono::Duration::days(1));
    }

    // Health: every store's size, and the folder's growth, in a sample
    // taken now (the rows written above carry no sizes).
    openalgo_desktop_lib::services::health_service::sample_once(&h.ctx).unwrap();
    let (s, v) = h
        .json(with(get("/health/api/current"), &cookie, None))
        .await;
    assert_eq!(s, 200, "{}", v);
    let reported = v["database"]["sizes_mb"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let stores = ["openalgo", "logs", "sandbox", "historify"];
    let missing: Vec<&str> = stores
        .iter()
        .filter(|s| !reported.contains_key(**s))
        .copied()
        .collect();
    let growth_shown = v["database"]["growth"]["stores"]
        .as_object()
        .is_some_and(|m| stores.iter().all(|s| m.contains_key(*s)));

    let plateau_base = sizes[2];
    let end = sizes[CYCLES - 1];
    let bounded = over_cap.is_empty() && (end as f64) <= plateau_base as f64 * PLATEAU;
    let met = Report::new("disk_growth")
        .field("cycles", CYCLES)
        .field("seconds", started.elapsed().as_secs())
        .field("logs_db_kib_cycle1", sizes[0] / 1024)
        .field("logs_db_kib_cycle3", plateau_base / 1024)
        .field("logs_db_kib_end", end / 1024)
        .field("tables_over_cap", over_cap.len())
        .verdict("monitoring_tables_capped_and_logs_db_flat", bounded);
    // Reported in health: every store's size, and its growth over the
    // monitoring window (NEW-REL-05).
    let shown = Report::new("disk_growth_reported")
        .field(
            "health_sizes_missing",
            if missing.is_empty() {
                "none".into()
            } else {
                missing.join(",")
            },
        )
        .field("health_growth_shown", growth_shown)
        .verdict(
            "every_store_and_growth_in_health",
            missing.is_empty() && growth_shown,
        );
    assert!(over_cap.is_empty(), "tables over their cap: {:?}", over_cap);
    assert!(met, "logs.db kept growing: {:?}", sizes);
    assert!(
        shown,
        "health does not report every store's size and growth (missing {:?}): {}",
        missing, v["database"]
    );
}
