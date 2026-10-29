//! Web test/test_sector_rotation.py: the calculation against a plain loop
//! written here independently, values worked by hand, weekly folding, and
//! the service over a fake loader.

use super::calc::*;
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

/// Weekdays from `start` (pandas `bdate_range`).
fn bdays(start: &str, n: usize) -> Vec<NaiveDate> {
    let mut out = Vec::new();
    let mut day = d(start);
    while out.len() < n {
        if !matches!(
            chrono::Datelike::weekday(&day),
            chrono::Weekday::Sat | chrono::Weekday::Sun
        ) {
            out.push(day);
        }
        day = day.succ_opt().unwrap();
    }
    out
}

/// A seeded random walk on business days: `start * exp(cumsum(N(0, 0.01)))`.
fn walk(seed: u64, n: usize) -> Series {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut uniform = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        ((state >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    };
    let mut level = 0.0;
    bdays("2024-01-01", n)
        .into_iter()
        .map(|day| {
            let (u1, u2) = (uniform(), uniform());
            level += 0.01 * (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos();
            (day, 100.0 * f64::exp(level))
        })
        .collect()
}

/// The same positions, one bar at a time, with no shared machinery.
fn plain_rotation(sector: &[f64], bench: &[f64], p: Params) -> Vec<(f64, f64)> {
    let rs: Vec<f64> = sector
        .iter()
        .zip(bench)
        .map(|(s, b)| 100.0 * s / b)
        .collect();
    let weighted = |len: usize, end: usize| {
        let window = &rs[end + 1 - len..=end];
        let num: f64 = window
            .iter()
            .enumerate()
            .map(|(i, v)| (i + 1) as f64 * v)
            .sum();
        num / (1..=len).sum::<usize>() as f64
    };
    let mut ratio = vec![None; rs.len()];
    for (i, r) in ratio.iter_mut().enumerate().skip(p.slow - 1) {
        *r = Some(100.0 * weighted(p.fast, i) / weighted(p.slow, i));
    }
    (0..rs.len())
        .filter_map(|i| {
            let r = ratio[i]?;
            let before = ratio[i.checked_sub(p.momentum)?]?;
            Some((py_r(r), py_r(100.0 * r / before)))
        })
        .collect()
}

fn py_r(x: f64) -> f64 {
    crate::brokers::common::mpp::py_round(x, 2)
}

#[test]
fn wma_weights_the_newest_bar_most() {
    let out = wma(&[1.0, 2.0, 3.0, 9.0], 3);
    assert_eq!(out[..2], [None, None]);
    assert!((out[2].unwrap() - 14.0 / 6.0).abs() < 1e-12);
    assert!((out[3].unwrap() - 35.0 / 6.0).abs() < 1e-12);
}

#[test]
fn matches_a_plain_loop() {
    for tf in [Timeframe::Daily, Timeframe::Weekly] {
        let (sector, bench) = (walk(1, 400), walk(2, 400));
        let values = |s: &Series| s.iter().map(|(_, c)| *c).collect::<Vec<_>>();
        let expected = plain_rotation(&values(&sector), &values(&bench), tf.params());
        let got: Vec<(f64, f64)> = rotation_series(&sector, &bench, tf)
            .iter()
            .map(|p| (py_r(p.rs_ratio), py_r(p.rs_momentum)))
            .collect();
        assert!(got.len() > 300);
        assert_eq!(got, expected);
    }
}

#[test]
fn lengths_per_timeframe() {
    let (daily, weekly) = (Timeframe::Daily.params(), Timeframe::Weekly.params());
    assert_eq!((daily.fast, daily.slow, daily.momentum), (14, 36, 8));
    assert_eq!((weekly.fast, weekly.slow, weekly.momentum), (5, 15, 5));
}

#[test]
fn first_point_needs_exactly_min_bars() {
    for tf in [Timeframe::Daily, Timeframe::Weekly] {
        let need = tf.params().min_bars();
        let (sector, bench) = (walk(3, need), walk(4, need));
        let short = rotation_series(
            &sector[..need - 1].to_vec(),
            &bench[..need - 1].to_vec(),
            tf,
        );
        assert!(short.is_empty());
        assert_eq!(rotation_series(&sector, &bench, tf).len(), 1);
    }
}

#[test]
fn a_symbol_moving_with_the_benchmark_sits_at_the_centre() {
    let bench = walk(5, 400);
    let tripled: Series = bench.iter().map(|&(d, c)| (d, c * 3.0)).collect();
    for p in rotation_series(&tripled, &bench, Timeframe::Daily) {
        assert!((p.rs_ratio - 100.0).abs() < 1e-9 && (p.rs_momentum - 100.0).abs() < 1e-9);
    }
}

#[test]
fn steady_outperformance_reads_strong() {
    let bench = walk(6, 400);
    let n = bench.len() as f64;
    let drifted: Series = bench
        .iter()
        .enumerate()
        .map(|(i, &(d, c))| (d, c * f64::exp(0.3 * i as f64 / (n - 1.0))))
        .collect();
    let last = *rotation_series(&drifted, &bench, Timeframe::Daily)
        .last()
        .unwrap();
    assert!(last.rs_ratio > 100.0);
}

#[test]
fn quadrants() {
    assert_eq!(quadrant(101.0, 101.0), "leading");
    assert_eq!(quadrant(101.0, 99.0), "weakening");
    assert_eq!(quadrant(99.0, 99.0), "lagging");
    assert_eq!(quadrant(99.0, 101.0), "improving");
    assert_eq!(quadrant(100.0, 100.0), "leading");
}

#[test]
fn bars_in_quadrant_counts_the_current_run_only() {
    let pts: Vec<Point> = [
        (101., 101.),
        (101., 101.),
        (99., 101.),
        (99., 99.),
        (99., 98.),
        (98., 97.),
    ]
    .iter()
    .map(|&(r, m)| Point {
        date: d("2026-10-01"),
        rs_ratio: r,
        rs_momentum: m,
    })
    .collect();
    assert_eq!(bars_in_quadrant(&pts), 3);
}

#[test]
fn weekly_bar_is_dated_by_its_last_session() {
    // Mon 2026-10-26 to Wed 2026-10-28: a week still in progress.
    let daily: Series = ["2026-10-19", "2026-10-23", "2026-10-26", "2026-10-28"]
        .iter()
        .zip([1.0, 2.0, 3.0, 4.0])
        .map(|(s, c)| (d(s), c))
        .collect();
    assert_eq!(
        to_weekly(&daily),
        vec![(d("2026-10-23"), 2.0), (d("2026-10-28"), 4.0)]
    );
}

#[test]
fn uses_only_sessions_both_traded() {
    let sector = walk(5, 400);
    let mut bench = walk(6, 400);
    bench.drain(100..110);
    let dates: std::collections::HashSet<_> = bench.iter().map(|(d, _)| *d).collect();
    assert!(rotation_series(&sector, &bench, Timeframe::Daily)
        .iter()
        .all(|p| dates.contains(&p.date)));
}

#[test]
fn weekly_keeps_a_week_one_side_ended_early() {
    // The benchmark trades every weekday; the symbol misses every fourth
    // Friday (a holiday on its exchange only). Each such week still pairs.
    let bench = walk(7, 600);
    let fridays: Vec<NaiveDate> = bench
        .iter()
        .map(|(d, _)| *d)
        .filter(|d| chrono::Datelike::weekday(d) == chrono::Weekday::Fri)
        .step_by(4)
        .collect();
    let symbol: Series = walk(8, 600)
        .into_iter()
        .filter(|(d, _)| !fridays.contains(d))
        .collect();
    let (ws, wb) = paired_bars(&symbol, &bench, Timeframe::Weekly);
    assert_eq!(
        ws.iter().map(|p| p.0).collect::<Vec<_>>(),
        wb.iter().map(|p| p.0).collect::<Vec<_>>()
    );
    assert_eq!(wb.len(), to_weekly(&bench).len());
}

/// A loader with a walk per symbol (seeded by the symbol's bytes), some
/// symbols missing and some short, counting its reads.
struct Fake {
    missing: &'static [&'static str],
    short: &'static [&'static str],
    reads: AtomicUsize,
}

fn fake(missing: &'static [&'static str], short: &'static [&'static str]) -> Fake {
    Fake {
        missing,
        short,
        reads: AtomicUsize::new(0),
    }
}

#[async_trait]
impl CloseLoader for Fake {
    async fn daily_closes(
        &self,
        ins: &Instrument,
        _: NaiveDate,
        _: NaiveDate,
    ) -> Result<Series, String> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if self.missing.contains(&ins.symbol.as_str()) {
            return Err(format!("{}: no history", ins.symbol));
        }
        let seed = ins.symbol.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
        });
        let n = if self.short.contains(&ins.symbol.as_str()) {
            10
        } else {
            300
        };
        Ok(walk(seed, n))
    }

    fn scope(&self) -> String {
        "fake".into()
    }
}

fn query(benchmark: &str, timeframe: &str, source: &str, instruments: Vec<Value>) -> Query {
    Query {
        benchmark: benchmark.into(),
        timeframe: timeframe.into(),
        source: source.into(),
        instruments,
        end: d("2026-10-27"),
    }
}

async fn run(q: Query, f: &Fake) -> Reply {
    compute(&q, f, &ClosesCache::default()).await
}

fn symbols(r: &Reply, key: &str) -> Vec<String> {
    r.body["data"][key]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["symbol"].as_str().unwrap().to_string())
        .collect()
}

fn reason(r: &Reply, symbol: &str) -> String {
    r.body["data"]["unavailable"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["symbol"] == symbol)
        .map(|u| u["reason"].as_str().unwrap().to_string())
        .unwrap()
}

#[tokio::test]
async fn one_missing_index_does_not_cost_the_map() {
    let r = run(
        query("NIFTY", "daily", "db", vec![]),
        &fake(&["NIFTYMEDIA"], &[]),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(symbols(&r, "unavailable"), ["NIFTYMEDIA"]);
    assert_eq!(symbols(&r, "instruments").len(), SECTORS.len() - 1);
}

#[tokio::test]
async fn short_history_is_reported_not_plotted() {
    let r = run(
        query("NIFTY", "daily", "db", vec![]),
        &fake(&[], &["NIFTYMEDIA"]),
    )
    .await;
    assert!(reason(&r, "NIFTYMEDIA").contains("Not enough daily history"));
}

#[tokio::test]
async fn sector_equal_to_benchmark_is_left_off() {
    let r = run(query("FINNIFTY", "weekly", "db", vec![]), &fake(&[], &[])).await;
    assert!(!symbols(&r, "instruments").contains(&"FINNIFTY".to_string()));
    assert_eq!(r.body["data"]["timeframe"], "weekly");
    assert!(reason(&r, "FINNIFTY").contains("benchmark itself"));
}

#[tokio::test]
async fn missing_benchmark_names_the_fix() {
    let r = run(
        query("NIFTY", "daily", "db", vec![]),
        &fake(&["NIFTY"], &[]),
    )
    .await;
    assert_eq!(r.status, 422);
    assert!(r.message().contains("Historify"));
}

#[tokio::test]
async fn rejects_unknown_inputs() {
    let f = fake(&[], &[]);
    assert_eq!(
        run(query("DOW", "daily", "db", vec![]), &f).await.status,
        400
    );
    assert_eq!(
        run(query("NIFTY", "monthly", "db", vec![]), &f)
            .await
            .status,
        400
    );
    assert_eq!(
        run(query("NIFTY", "daily", "csv", vec![]), &f).await.status,
        400
    );
}

#[tokio::test]
async fn custom_list_places_stocks_and_indices() {
    let r = run(
        query(
            "NIFTY",
            "daily",
            "db",
            vec![
                json!({"symbol": "reliance", "exchange": "nse"}),
                json!({"symbol": "NIFTYIT", "exchange": "NSE_INDEX"}),
                json!({"symbol": "RELIANCE", "exchange": "NSE"}),
            ],
        ),
        &fake(&[], &[]),
    )
    .await;
    let placed: Vec<(String, String)> = r.body["data"]["instruments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["symbol"].as_str().unwrap().into(),
                i["label"].as_str().unwrap().into(),
            )
        })
        .collect();
    // Duplicates dropped; a known sector keeps its name, a stock its symbol.
    assert_eq!(
        placed,
        [
            ("RELIANCE".into(), "RELIANCE".into()),
            ("NIFTYIT".into(), "IT".into())
        ]
    );
}

#[tokio::test]
async fn custom_list_refuses_derivatives_and_oversize() {
    let f = fake(&[], &[]);
    let r = run(
        query(
            "NIFTY",
            "daily",
            "db",
            vec![json!({"symbol": "NIFTY30OCT26FUT", "exchange": "NFO"})],
        ),
        &f,
    )
    .await;
    assert_eq!(r.status, 400);
    assert!(r.message().contains("NFO"));
    let too_many = (0..=MAX_INSTRUMENTS)
        .map(|i| json!({"symbol": format!("S{}", i), "exchange": "NSE"}))
        .collect();
    assert_eq!(
        run(query("NIFTY", "daily", "db", too_many), &f)
            .await
            .status,
        400
    );
}

#[tokio::test]
async fn closes_are_reused_across_maps() {
    let (f, cache) = (fake(&[], &[]), ClosesCache::default());
    compute(&query("NIFTY", "daily", "api", vec![]), &f, &cache).await;
    let first = f.reads.load(Ordering::SeqCst);
    let one = vec![json!({"symbol": "NIFTYIT", "exchange": "NSE_INDEX"})];
    compute(&query("NIFTY", "daily", "api", one), &f, &cache).await;
    assert_eq!(f.reads.load(Ordering::SeqCst), first);
}

#[tokio::test]
async fn trail_is_capped_at_max_tail() {
    let r = run(query("NIFTY", "daily", "db", vec![]), &fake(&[], &[])).await;
    for s in r.body["data"]["instruments"].as_array().unwrap() {
        let tail = s["tail"].as_array().unwrap();
        assert_eq!(tail.len(), MAX_TAIL);
        assert_eq!(tail[MAX_TAIL - 1]["rs_ratio"], s["rs_ratio"]);
    }
}

#[test]
fn default_map_leaves_off_indices_brokers_rarely_publish() {
    let optional: Vec<&str> = OPTIONAL_SECTORS.iter().map(|s| s.0).collect();
    assert_eq!(
        optional,
        [
            "NIFTYCONSRDURBL",
            "NIFTYHEALTHCARE",
            "NIFTYINDDEFENCE",
            "NIFTYOILANDGAS"
        ]
    );
    assert!(SECTORS.iter().all(|s| !optional.contains(&s.0)));
}

#[tokio::test]
async fn an_optional_sector_added_by_hand_keeps_its_name() {
    let one = vec![json!({"symbol": "NIFTYHEALTHCARE", "exchange": "NSE_INDEX"})];
    let r = run(query("NIFTY", "daily", "db", one), &fake(&[], &[])).await;
    assert_eq!(r.body["data"]["instruments"][0]["label"], "Healthcare");
}

/// History candles become one positive close per IST session; a re-read
/// session keeps its last close; fewer than two sessions is no history.
#[test]
fn history_candles_become_daily_closes() {
    // IST midnight of 27 and 28 Oct 2026, the 28th twice.
    let (d27, d28) = (1_793_039_400i64, 1_793_125_800i64);
    let data = json!([
        {"timestamp": d27, "close": 10.0},
        {"timestamp": d28, "close": 11.0},
        {"timestamp": d28, "close": 12.0},
    ]);
    assert_eq!(
        daily_series(&data).unwrap(),
        vec![(d("2026-10-27"), 10.0), (d("2026-10-28"), 12.0)]
    );
    assert!(daily_series(&json!([{"timestamp": d27, "close": 10.0}])).is_err());
    assert!(daily_series(
        &json!([{"timestamp": d27, "close": 0.0}, {"timestamp": d28, "close": 1.0}])
    )
    .is_err());
}
