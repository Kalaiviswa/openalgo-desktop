//! How large each database in the data folder is, and how fast it grows
//! (NEW-REL-05). Log retention matches the web: the order and analyzer logs
//! are kept forever, so the trader sees the folder's growth in health
//! rather than finding it when the disk fills.
//!
//! Every health sample stores the sizes (`health_metrics.db_sizes`), so the
//! growth over the monitoring window (the samples' retention, 7 days) is
//! the newest sample's sizes against the oldest one kept, and the growth
//! over the last day the newest against the oldest of the last 24 hours.

use crate::db::sqlite::monitor::{self as store, HealthRow};
use crate::services::health_service::iso_ist;
use crate::state::AppState;
use serde_json::{json, Map, Value};
use std::path::Path;

/// What a store keeps on disk beside its main file.
#[derive(Clone, Copy)]
enum Kind {
    /// The file, its write-ahead log and its shared-memory index.
    Sqlite,
    /// The file and its write-ahead log (`.wal`, not `-wal`).
    DuckDb,
}

/// Every database in the data folder: the name health and the system
/// report show, and its file. The sandbox's is as large as the trader's
/// analyzer history and grows with it; order latency is kept in `logs.db`
/// with the order and analyzer logs.
pub const DATABASES: &[(&str, &str)] = &[
    ("openalgo", "openalgo.db"),
    ("logs", "logs.db"),
    ("sandbox", "sandbox.db"),
    ("historify", "historify.duckdb"),
];

fn kind_of(file: &str) -> Kind {
    if file.ends_with(".duckdb") {
        Kind::DuckDb
    } else {
        Kind::Sqlite
    }
}

/// One store's bytes on disk, its side files included.
fn store_bytes(dir: &Path, file: &str) -> u64 {
    let suffixes: &[&str] = match kind_of(file) {
        Kind::Sqlite => &["", "-wal", "-shm"],
        Kind::DuckDb => &["", ".wal"],
    };
    suffixes
        .iter()
        .filter_map(|s| std::fs::metadata(dir.join(format!("{}{}", file, s))).ok())
        .map(|m| m.len())
        .sum()
}

fn mb(bytes: u64) -> f64 {
    round2(bytes as f64 / (1024.0 * 1024.0))
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// File sizes of each database, MB (side files included).
pub fn sizes(ctx: &AppState) -> Map<String, Value> {
    DATABASES
        .iter()
        .map(|&(name, file)| {
            (
                name.to_string(),
                json!(mb(store_bytes(&ctx.data_dir, file))),
            )
        })
        .collect()
}

/// A daily rate needs a day of samples: over a few hours it would stretch
/// one trading session's writes over the whole day.
const RATE_AFTER_HOURS: f64 = 24.0;

/// Growth of each store and of all of them over two windows ending at
/// `latest` (PROD-REL-05): every sample kept (the monitoring window, up to
/// 7 days) at the top level, and the last day under `last_day`. Each is
/// `{window_hours, since, stores: {name: {size_mb, growth_mb, per_day_mb}},
/// total: {...}}`; growth is null without an earlier sample that recorded
/// sizes, and the daily rate is null over less than a day.
pub fn growth_json(ctx: &AppState, latest: &HealthRow) -> Value {
    let day_start = store::parse_ts(&latest.timestamp).map(|t| t - chrono::Duration::hours(24));
    let (kept, last_day) = ctx
        .logs
        .conn()
        .and_then(|c| {
            Ok((
                store::oldest_db_sizes(&c, None)?,
                store::oldest_db_sizes(&c, day_start)?,
            ))
        })
        .unwrap_or_else(|e| {
            tracing::debug!("Database growth not read: {}", e);
            (None, None)
        });
    let mut v = window_json(latest, kept);
    v["last_day"] = window_json(latest, last_day);
    v
}

/// Each store's growth from `oldest` (a sample's timestamp and sizes) to
/// `latest`.
fn window_json(latest: &HealthRow, oldest: Option<(String, Value)>) -> Value {
    let now = latest.db_sizes.as_ref().and_then(Value::as_object);
    let window_hours = oldest
        .as_ref()
        .and_then(|(ts, _)| {
            let (then, latest) = (store::parse_ts(ts)?, store::parse_ts(&latest.timestamp)?);
            Some((latest - then).num_seconds().max(0) as f64 / 3600.0)
        })
        .unwrap_or(0.0);
    let then = oldest.as_ref().and_then(|(_, v)| v.as_object());
    let rate = |growth: f64| {
        (window_hours >= RATE_AFTER_HOURS).then(|| round2(growth * 24.0 / window_hours))
    };
    let mut stores = Map::new();
    let (mut total_now, mut total_then) = (0.0, 0.0);
    for &(name, _) in DATABASES {
        let size = now.and_then(|m| m.get(name)).and_then(Value::as_f64);
        let before = then.and_then(|m| m.get(name)).and_then(Value::as_f64);
        let growth = size.zip(before).map(|(s, b)| round2(s - b));
        total_now += size.unwrap_or(0.0);
        total_then += before.unwrap_or(size.unwrap_or(0.0));
        stores.insert(
            name.into(),
            json!({"size_mb": size, "growth_mb": growth, "per_day_mb": growth.and_then(rate)}),
        );
    }
    let total_growth = then.map(|_| round2(total_now - total_then));
    json!({
        "window_hours": round2(window_hours),
        "since": oldest.as_ref().map(|(ts, _)| iso_ist(ts)),
        "stores": stores,
        "total": {
            "size_mb": round2(total_now),
            "growth_mb": total_growth,
            "per_day_mb": total_growth.and_then(rate),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DuckDB's write-ahead log is `<file>.wal`; it was missed when every
    /// store was measured with SQLite's `-wal` and `-shm`.
    #[test]
    fn each_store_counts_its_own_side_files() {
        let dir = tempfile::tempdir().unwrap();
        let write = |f: &str, n: usize| std::fs::write(dir.path().join(f), vec![0u8; n]).unwrap();
        write("logs.db", 1000);
        write("logs.db-wal", 200);
        write("logs.db-shm", 30);
        write("historify.duckdb", 4000);
        write("historify.duckdb.wal", 500);
        write("historify.duckdb-wal", 7);
        assert_eq!(store_bytes(dir.path(), "logs.db"), 1230);
        assert_eq!(store_bytes(dir.path(), "historify.duckdb"), 4500);
        assert_eq!(store_bytes(dir.path(), "sandbox.db"), 0);
    }
}
