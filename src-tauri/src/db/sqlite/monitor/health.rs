//! Health samples and alerts (web `health_db`).

use super::ts;
use crate::error::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub struct HealthRow {
    pub id: i64,
    pub timestamp: String,
    pub fd_count: Option<i64>,
    pub fd_limit: Option<i64>,
    pub fd_usage_percent: Option<f64>,
    pub fd_status: Option<String>,
    pub memory_rss_mb: Option<f64>,
    pub memory_vms_mb: Option<f64>,
    pub memory_percent: Option<f64>,
    pub memory_available_mb: Option<f64>,
    pub memory_swap_mb: Option<f64>,
    pub memory_status: Option<String>,
    pub db_connections_total: Option<i64>,
    pub db_connections: Option<Value>,
    pub db_status: Option<String>,
    pub ws_connections_total: Option<i64>,
    pub ws_connections: Option<Value>,
    pub ws_total_symbols: Option<i64>,
    pub ws_status: Option<String>,
    pub thread_count: Option<i64>,
    pub stuck_threads: Option<i64>,
    pub thread_details: Option<Value>,
    pub thread_status: Option<String>,
    pub process_details: Option<Value>,
    pub db_sizes: Option<Value>,
    pub overall_status: Option<String>,
}

pub fn insert_health(conn: &Connection, h: &HealthRow) -> Result<i64> {
    let j = |v: &Option<Value>| v.as_ref().map(|v| v.to_string());
    conn.execute(
        "INSERT INTO health_metrics (timestamp, fd_count, fd_limit, fd_usage_percent, fd_available, fd_status,
            memory_rss_mb, memory_vms_mb, memory_percent, memory_available_mb, memory_swap_mb, memory_status,
            db_connections_total, db_connections, db_status, ws_connections_total, ws_connections,
            ws_total_symbols, ws_status, thread_count, stuck_threads, thread_details, thread_status,
            process_details, db_sizes, overall_status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19,
                 ?20, ?21, ?22, ?23, ?24, ?25, ?26)",
        params![
            h.timestamp,
            h.fd_count,
            h.fd_limit,
            h.fd_usage_percent,
            match (h.fd_limit, h.fd_count) {
                (Some(l), Some(c)) => Some(l - c),
                _ => None,
            },
            h.fd_status,
            h.memory_rss_mb,
            h.memory_vms_mb,
            h.memory_percent,
            h.memory_available_mb,
            h.memory_swap_mb,
            h.memory_status,
            h.db_connections_total,
            j(&h.db_connections),
            h.db_status,
            h.ws_connections_total,
            j(&h.ws_connections),
            h.ws_total_symbols,
            h.ws_status,
            h.thread_count,
            h.stuck_threads,
            j(&h.thread_details),
            h.thread_status,
            j(&h.process_details),
            j(&h.db_sizes),
            h.overall_status,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

const HEALTH_COLS: &str =
    "id, timestamp, fd_count, fd_limit, fd_usage_percent, fd_status, memory_rss_mb,
    memory_vms_mb, memory_percent, memory_available_mb, memory_swap_mb, memory_status,
    db_connections_total, db_connections, db_status, ws_connections_total, ws_connections,
    ws_total_symbols, ws_status, thread_count, stuck_threads, thread_details, thread_status,
    process_details, db_sizes, overall_status";

fn health_from_row(r: &rusqlite::Row) -> rusqlite::Result<HealthRow> {
    let j = |s: Option<String>| s.and_then(|s| serde_json::from_str::<Value>(&s).ok());
    Ok(HealthRow {
        id: r.get(0)?,
        timestamp: r.get(1)?,
        fd_count: r.get(2)?,
        fd_limit: r.get(3)?,
        fd_usage_percent: r.get(4)?,
        fd_status: r.get(5)?,
        memory_rss_mb: r.get(6)?,
        memory_vms_mb: r.get(7)?,
        memory_percent: r.get(8)?,
        memory_available_mb: r.get(9)?,
        memory_swap_mb: r.get(10)?,
        memory_status: r.get(11)?,
        db_connections_total: r.get(12)?,
        db_connections: j(r.get(13)?),
        db_status: r.get(14)?,
        ws_connections_total: r.get(15)?,
        ws_connections: j(r.get(16)?),
        ws_total_symbols: r.get(17)?,
        ws_status: r.get(18)?,
        thread_count: r.get(19)?,
        stuck_threads: r.get(20)?,
        thread_details: j(r.get(21)?),
        thread_status: r.get(22)?,
        process_details: j(r.get(23)?),
        db_sizes: j(r.get(24)?),
        overall_status: r.get(25)?,
    })
}

pub fn latest_health(conn: &Connection) -> Result<Option<HealthRow>> {
    Ok(conn
        .query_row(
            &format!("SELECT {HEALTH_COLS} FROM health_metrics ORDER BY id DESC LIMIT 1"),
            [],
            health_from_row,
        )
        .optional()?)
}

pub fn health_since(conn: &Connection, since: DateTime<Utc>) -> Result<Vec<HealthRow>> {
    let mut st = conn.prepare(&format!(
        "SELECT {HEALTH_COLS} FROM health_metrics WHERE timestamp >= ?1 ORDER BY id"
    ))?;
    let rows = st
        .query_map([ts(since)], health_from_row)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[derive(Debug, Clone, Default)]
pub struct AlertRow {
    pub id: i64,
    pub timestamp: String,
    pub alert_type: String,
    pub severity: String,
    pub metric_name: String,
    pub metric_value: f64,
    pub threshold_value: f64,
    pub message: String,
    pub acknowledged: bool,
    pub resolved: bool,
}

/// Raise an alert, or refresh the open one of the same type.
pub fn raise_alert(conn: &Connection, a: &AlertRow, now: DateTime<Utc>) -> Result<()> {
    let open: Option<i64> = conn
        .query_row(
            "SELECT id FROM health_alerts WHERE alert_type = ?1 AND resolved = 0 ORDER BY id DESC LIMIT 1",
            [&a.alert_type],
            |r| r.get(0),
        )
        .optional()?;
    match open {
        Some(id) => {
            conn.execute(
                "UPDATE health_alerts SET timestamp = ?1, metric_value = ?2 WHERE id = ?3",
                params![ts(now), a.metric_value, id],
            )?;
        }
        None => {
            conn.execute(
                "INSERT INTO health_alerts (timestamp, alert_type, severity, metric_name, metric_value,
                    threshold_value, message) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    ts(now),
                    a.alert_type,
                    a.severity,
                    a.metric_name,
                    a.metric_value,
                    a.threshold_value,
                    a.message
                ],
            )?;
        }
    }
    Ok(())
}

/// Resolve open alerts for a metric that is back under its warning level.
pub fn auto_resolve(conn: &Connection, metric: &str, now: DateTime<Utc>) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE health_alerts SET resolved = 1, resolved_at = ?1 WHERE metric_name = ?2 AND resolved = 0",
        params![ts(now), metric],
    )?)
}

pub fn active_alerts(conn: &Connection) -> Result<Vec<AlertRow>> {
    let mut st = conn.prepare(
        "SELECT id, timestamp, alert_type, severity, metric_name, metric_value, threshold_value, message,
                acknowledged, resolved
         FROM health_alerts WHERE resolved = 0 ORDER BY timestamp DESC, id DESC",
    )?;
    let rows = st
        .query_map([], |r| {
            Ok(AlertRow {
                id: r.get(0)?,
                timestamp: r.get(1)?,
                alert_type: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                severity: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                metric_name: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                metric_value: r.get::<_, Option<f64>>(5)?.unwrap_or(0.0),
                threshold_value: r.get::<_, Option<f64>>(6)?.unwrap_or(0.0),
                message: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
                acknowledged: r.get(8)?,
                resolved: r.get(9)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn acknowledge_alert(conn: &Connection, id: i64, now: DateTime<Utc>) -> Result<bool> {
    Ok(conn.execute(
        "UPDATE health_alerts SET acknowledged = 1, acknowledged_at = ?1 WHERE id = ?2",
        params![ts(now), id],
    )? > 0)
}

pub fn resolve_alert(conn: &Connection, id: i64, now: DateTime<Utc>) -> Result<bool> {
    Ok(conn.execute(
        "UPDATE health_alerts SET resolved = 1, resolved_at = ?1 WHERE id = ?2",
        params![ts(now), id],
    )? > 0)
}
