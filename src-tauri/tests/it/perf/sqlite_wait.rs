//! Target: order-path SQLite wait while Historify imports, p99 under 20 ms.
//!
//! Historify keeps its bars in DuckDB and never opens SQLite, so an import
//! competes with the order path for the CPU, the disk (its writes against
//! `openalgo.db`'s `synchronous = FULL` commits) and the blocking pool, not
//! for SQLite's lock. Two phases, the same load in each: a baseline, then
//! the same with 200,000-row CSV imports running back to back through
//! `Historify::upload` (what the Upload page calls).
//!
//! Measured in both: an order-path write to `openalgo.db` (saving a
//! scalping stop, as an entry from the terminal does), timed from asking
//! for it until it is committed, every 20 ms; and `/api/v1/placeorder`
//! end to end over HTTP to the mock broker (its reads: the API key, the
//! order mode, the analyzer toggle; its order log goes to `logs.db`), eight
//! a second, under the 10-a-second order limit. The broker feed streams
//! 500 ticks a second over 200 symbols meanwhile.

use super::rig::{Rig, RigOptions};
use super::{ms, Report, Stats};
use openalgo_desktop_lib::historify::import::FileKind;
use openalgo_desktop_lib::scalping::store::SlUpsert;
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const PHASE: Duration = Duration::from_secs(30);
const WRITE_EVERY: Duration = Duration::from_millis(20);
const ORDER_EVERY: Duration = Duration::from_millis(125);
const IMPORT_ROWS: usize = 200_000;
const TARGET_P99: Duration = Duration::from_millis(20);

/// One-minute bars, 375 a day from 09:15, `rows` of them.
fn write_csv(path: &Path, rows: usize) {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    writeln!(f, "date,time,open,high,low,close,volume,oi").unwrap();
    let day0 = chrono::NaiveDate::from_ymd_opt(2023, 1, 2).unwrap();
    for i in 0..rows {
        let day = day0 + chrono::Duration::days((i / 375) as i64);
        let minute = 9 * 60 + 15 + (i % 375);
        let p = 100.0 + (i % 997) as f64 * 0.05;
        writeln!(
            f,
            "{},{:02}:{:02}:00,{:.2},{:.2},{:.2},{:.2},{},0",
            day.format("%Y-%m-%d"),
            minute / 60,
            minute % 60,
            p,
            p + 0.5,
            p - 0.5,
            p + 0.1,
            1000 + i % 500
        )
        .unwrap();
    }
    f.flush().unwrap();
}

/// What one phase measured.
struct Phase {
    writes: Vec<Duration>,
    /// Writes refused (the database stayed busy, or never waited).
    write_errors: Vec<String>,
    orders: Vec<Duration>,
    imports: u64,
    imported_rows: u64,
}

fn stop_row(i: u64) -> SlUpsert {
    SlUpsert {
        symbol: format!("SYM{:04}", i % 200),
        exchange: "NSE".into(),
        product: "MIS".into(),
        mode: "live".into(),
        side: Some("BUY".into()),
        entry_price: Some(1000.0),
        quantity: Some(1),
        initial_sl: Some(990.0 - (i % 10) as f64),
        // Saved, not watched: the monitor is not asked to pick it up.
        is_active: Some(false),
        ..Default::default()
    }
}

/// Back-to-back imports until `stop`, each under a new symbol.
fn import_loop(
    rig: &Rig,
    csv: PathBuf,
    stop: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<(u64, u64)> {
    let historify = rig.ctx.historify.clone();
    tokio::spawn(async move {
        let (mut n, mut rows) = (0u64, 0u64);
        while !stop.load(Ordering::Relaxed) {
            let reply = historify
                .upload(&csv, FileKind::Csv, &format!("IMP{:03}", n), "NSE", "1m")
                .await;
            let v = reply.body;
            assert_eq!(v["status"], "success", "import failed: {}", v);
            rows += v["records"].as_u64().unwrap_or(0);
            n += 1;
        }
        (n, rows)
    })
}

async fn phase(rig: &Rig, csv: Option<PathBuf>) -> Phase {
    let stop = Arc::new(AtomicBool::new(false));
    let imports = csv.map(|c| import_loop(rig, c, stop.clone()));
    let store = rig.ctx.scalping.store.clone();
    let writer = {
        let stop = stop.clone();
        tokio::spawn(async move {
            let (mut out, mut errors) = (Vec::new(), Vec::new());
            let mut pace = tokio::time::interval(WRITE_EVERY);
            pace.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let i = AtomicU64::new(0);
            while !stop.load(Ordering::Relaxed) {
                pace.tick().await;
                let (s, row) = (store.clone(), stop_row(i.fetch_add(1, Ordering::Relaxed)));
                let t = Instant::now();
                match tokio::task::spawn_blocking(move || s.upsert_sl(&row))
                    .await
                    .unwrap()
                {
                    Ok(_) => out.push(t.elapsed()),
                    Err(e) => errors.push(format!("{} after {:?}", e, t.elapsed())),
                }
            }
            (out, errors)
        })
    };
    let orders = {
        let (stop, base, key) = (stop.clone(), rig.base.clone(), rig.key.clone());
        tokio::spawn(async move {
            let http = reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap();
            let mut out = Vec::new();
            let mut pace = tokio::time::interval(ORDER_EVERY);
            pace.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            while !stop.load(Ordering::Relaxed) {
                pace.tick().await;
                let body = json!({
                    "apikey": key, "strategy": "perf", "symbol": "SYM0001", "action": "BUY",
                    "exchange": "NSE", "pricetype": "MARKET", "product": "MIS", "quantity": "1"
                });
                let t = Instant::now();
                let resp = http
                    .post(format!("{}/api/v1/placeorder", base))
                    .json(&body)
                    .send()
                    .await
                    .unwrap();
                let status = resp.status().as_u16();
                let v: Value = resp.json().await.unwrap_or(Value::Null);
                out.push(t.elapsed());
                assert_eq!(
                    (status, v["status"].as_str()),
                    (200, Some("success")),
                    "{}",
                    v
                );
            }
            out
        })
    };
    tokio::time::sleep(PHASE).await;
    stop.store(true, Ordering::Relaxed);
    let (writes, write_errors) = writer.await.unwrap();
    let orders = orders.await.unwrap();
    let (imports, imported_rows) = match imports {
        Some(t) => t.await.unwrap(),
        None => (0, 0),
    };
    Phase {
        writes,
        write_errors,
        orders,
        imports,
        imported_rows,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn order_path_sqlite_wait_p99_under_20ms_while_historify_imports() {
    crate::isolated!(order_path_sqlite_wait_p99_under_20ms_while_historify_imports);
    let rig = Rig::start(RigOptions {
        symbols: 200,
        tick_rate: 500,
        quiet: vec![],
        trend_per_s: 0.0,
    })
    .await;
    let csv = rig.dir.path().join("perf_import.csv");
    write_csv(&csv, IMPORT_ROWS);

    let base = phase(&rig, None).await;
    let load = phase(&rig, Some(csv)).await;
    assert!(load.imports > 0, "no import finished during the phase");

    let (bw, lw) = (Stats::of(&base.writes), Stats::of(&load.writes));
    let (bo, lo) = (Stats::of(&base.orders), Stats::of(&load.orders));
    let met = Report::new("sqlite_wait")
        .stats("write_baseline", &bw)
        .stats("write_importing", &lw)
        .stats("placeorder_baseline", &bo)
        .stats("placeorder_importing", &lo)
        .field("write_errors_baseline", base.write_errors.len())
        .field("write_errors_importing", load.write_errors.len())
        .field("imports", load.imports)
        .field("imported_rows", load.imported_rows)
        .field(
            "import_rows_per_s",
            format!("{:.0}", load.imported_rows as f64 / PHASE.as_secs_f64()),
        )
        .verdict(
            "write_importing_p99_ms<20",
            lw.p99 < TARGET_P99 && base.write_errors.is_empty() && load.write_errors.is_empty(),
        );
    rig.stop().await;
    let errors: Vec<&String> = base.write_errors.iter().chain(&load.write_errors).collect();
    assert!(errors.is_empty(), "order-path writes refused: {:?}", errors);
    assert!(
        met,
        "order-path write p99 {} ms while importing (target under {} ms)",
        ms(lw.p99),
        ms(TARGET_P99)
    );
}
