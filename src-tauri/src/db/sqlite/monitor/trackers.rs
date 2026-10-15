//! The 404 and invalid API key trackers (web `traffic_db.Error404Tracker`,
//! `InvalidAPIKeyTracker`).

use super::{cap_text, parse_ts, strip_query, ts};
use crate::error::Result;
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct TrackerRow {
    pub ip_address: String,
    pub count: i64,
    pub first_at: String,
    pub last_at: String,
    pub detail: Option<String>,
}

/// Count a 404 for `ip`, remembering up to 50 distinct paths. A tracker
/// older than a day starts over (web `_track_404_locked`). Returns the count.
pub fn track_404(conn: &Connection, ip: &str, path: &str, now: DateTime<Utc>) -> Result<i64> {
    let path = cap_text(&strip_query(path), 200);
    let row: Option<(i64, String, Option<String>)> = conn
        .query_row(
            "SELECT error_count, first_error_at, paths_attempted FROM error_404_tracker WHERE ip_address = ?1",
            [ip],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let stale = |first: &str| {
        parse_ts(first)
            .map(|f| now - f >= Duration::days(1))
            .unwrap_or(true)
    };
    match row {
        Some((n, first, paths)) if !stale(&first) => {
            let mut v: Vec<String> = paths
                .and_then(|p| serde_json::from_str(&p).ok())
                .unwrap_or_default();
            if !v.contains(&path) {
                v.push(path);
                if v.len() > 50 {
                    v.drain(..v.len() - 50);
                }
            }
            conn.execute(
                "UPDATE error_404_tracker SET error_count = ?1, last_error_at = ?2, paths_attempted = ?3
                 WHERE ip_address = ?4",
                params![n + 1, ts(now), Value::from(v).to_string(), ip],
            )?;
            Ok(n + 1)
        }
        _ => {
            conn.execute(
                "INSERT INTO error_404_tracker (ip_address, error_count, first_error_at, last_error_at, paths_attempted)
                 VALUES (?1, 1, ?2, ?2, ?3)
                 ON CONFLICT(ip_address) DO UPDATE SET error_count = 1, first_error_at = excluded.first_error_at,
                     last_error_at = excluded.last_error_at, paths_attempted = excluded.paths_attempted",
                params![ip, ts(now), json!([path]).to_string()],
            )?;
            Ok(1)
        }
    }
}

/// Count an invalid API key attempt for `ip`. The key itself is never
/// stored (the web keeps hashes; the desktop keeps nothing).
pub fn track_invalid_api_key(conn: &Connection, ip: &str, now: DateTime<Utc>) -> Result<i64> {
    let row: Option<(i64, String)> = conn
        .query_row(
            "SELECT attempt_count, first_attempt_at FROM invalid_api_key_tracker WHERE ip_address = ?1",
            [ip],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let fresh = row
        .as_ref()
        .and_then(|(_, f)| parse_ts(f))
        .map(|f| now - f < Duration::days(1))
        .unwrap_or(false);
    match row {
        Some((n, _)) if fresh => {
            conn.execute(
                "UPDATE invalid_api_key_tracker SET attempt_count = ?1, last_attempt_at = ?2 WHERE ip_address = ?3",
                params![n + 1, ts(now), ip],
            )?;
            Ok(n + 1)
        }
        _ => {
            conn.execute(
                "INSERT INTO invalid_api_key_tracker (ip_address, attempt_count, first_attempt_at, last_attempt_at, api_keys_tried)
                 VALUES (?1, 1, ?2, ?2, '[]')
                 ON CONFLICT(ip_address) DO UPDATE SET attempt_count = 1,
                     first_attempt_at = excluded.first_attempt_at, last_attempt_at = excluded.last_attempt_at",
                params![ip, ts(now)],
            )?;
            Ok(1)
        }
    }
}

fn trackers(
    conn: &Connection,
    sql: &str,
    min: i64,
    now: DateTime<Utc>,
    first_col: &str,
    table: &str,
) -> Result<Vec<TrackerRow>> {
    conn.execute(
        &format!("DELETE FROM {table} WHERE {first_col} < ?1"),
        [ts(now - Duration::days(1))],
    )?;
    let mut st = conn.prepare(sql)?;
    let rows = st
        .query_map([min], |r| {
            Ok(TrackerRow {
                ip_address: r.get(0)?,
                count: r.get(1)?,
                first_at: r.get(2)?,
                last_at: r.get(3)?,
                detail: r.get(4)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn suspicious_404(conn: &Connection, min: i64, now: DateTime<Utc>) -> Result<Vec<TrackerRow>> {
    trackers(
        conn,
        "SELECT ip_address, error_count, first_error_at, last_error_at, paths_attempted
         FROM error_404_tracker WHERE error_count >= ?1 ORDER BY error_count DESC LIMIT 1000",
        min,
        now,
        "first_error_at",
        "error_404_tracker",
    )
}

pub fn suspicious_api(conn: &Connection, min: i64, now: DateTime<Utc>) -> Result<Vec<TrackerRow>> {
    trackers(
        conn,
        "SELECT ip_address, attempt_count, first_attempt_at, last_attempt_at, api_keys_tried
         FROM invalid_api_key_tracker WHERE attempt_count >= ?1 ORDER BY attempt_count DESC LIMIT 1000",
        min,
        now,
        "first_attempt_at",
        "invalid_api_key_tracker",
    )
}

pub fn clear_404(conn: &Connection, ip: &str) -> Result<bool> {
    Ok(conn.execute("DELETE FROM error_404_tracker WHERE ip_address = ?1", [ip])? > 0)
}
