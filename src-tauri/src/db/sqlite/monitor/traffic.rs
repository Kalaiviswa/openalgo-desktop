//! The request log (web `traffic_db`): one row per request, path without its query.

use super::{cap_text, strip_query};
use crate::error::Result;
use rusqlite::{params, Connection};

#[derive(Debug, Clone)]
pub struct TrafficRow {
    pub timestamp: String,
    pub client_ip: String,
    pub method: String,
    pub path: String,
    pub status_code: i64,
    pub duration_ms: f64,
    pub host: Option<String>,
    pub error: Option<String>,
}

pub fn insert_traffic(conn: &Connection, r: &TrafficRow) -> Result<()> {
    conn.execute(
        "INSERT INTO traffic_logs (timestamp, client_ip, method, path, status_code, duration_ms, host, error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            r.timestamp,
            cap_text(&r.client_ip, 50),
            cap_text(&r.method, 10),
            cap_text(&strip_query(&r.path), 500),
            r.status_code,
            r.duration_ms,
            r.host.as_deref().map(|h| cap_text(h, 500)),
            r.error.as_deref().map(|e| cap_text(e, 500)),
        ],
    )?;
    Ok(())
}

fn traffic_from_row(r: &rusqlite::Row) -> rusqlite::Result<TrafficRow> {
    Ok(TrafficRow {
        timestamp: r.get(0)?,
        client_ip: r.get(1)?,
        method: r.get(2)?,
        path: r.get(3)?,
        status_code: r.get(4)?,
        duration_ms: r.get(5)?,
        host: r.get(6)?,
        error: r.get(7)?,
    })
}

pub fn recent_traffic(conn: &Connection, limit: Option<i64>) -> Result<Vec<TrafficRow>> {
    let mut st = conn.prepare(
        "SELECT timestamp, client_ip, method, path, status_code, duration_ms, host, error
         FROM traffic_logs ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = st
        .query_map([limit.unwrap_or(-1)], traffic_from_row)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// `(total, errors, avg_duration)` for paths matching `like` (all when None).
pub fn traffic_summary(conn: &Connection, like: Option<&str>) -> Result<(i64, i64, f64)> {
    let pat = like.unwrap_or("%");
    Ok(conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(CASE WHEN status_code >= 400 THEN 1 ELSE 0 END), 0),
                COALESCE(AVG(duration_ms), 0)
         FROM traffic_logs WHERE path LIKE ?1",
        [pat],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?)
}

/// Distinct client addresses seen with a Host header containing `host`.
pub fn traffic_ips_for_host(conn: &Connection, host: &str) -> Result<Vec<String>> {
    let mut st = conn.prepare(
        "SELECT DISTINCT client_ip FROM traffic_logs WHERE host LIKE ?1 ESCAPE '\\' LIMIT 1000",
    )?;
    let esc = host
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let rows = st
        .query_map([format!("%{}%", esc)], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}
