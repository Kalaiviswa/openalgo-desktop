//! `/api/v1` order latency (web `latency_db.OrderLatency`).

use super::cap_text;
use crate::error::Result;
use rusqlite::{params, Connection};

#[derive(Debug, Clone, Default)]
pub struct LatencyRow {
    pub id: i64,
    pub timestamp: String,
    pub order_id: String,
    pub broker: Option<String>,
    pub symbol: Option<String>,
    pub order_type: String,
    pub rtt_ms: f64,
    pub validation_latency_ms: f64,
    pub response_latency_ms: f64,
    pub overhead_ms: f64,
    pub total_latency_ms: f64,
    pub status: String,
    pub error: Option<String>,
}

pub fn insert_latency(conn: &Connection, l: &LatencyRow) -> Result<()> {
    conn.execute(
        "INSERT INTO order_latency (timestamp, order_id, broker, symbol, order_type, rtt_ms,
            validation_latency_ms, response_latency_ms, overhead_ms, total_latency_ms, status, error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            l.timestamp,
            cap_text(&l.order_id, 100),
            l.broker.as_deref().map(|s| cap_text(s, 50)),
            l.symbol.as_deref().map(|s| cap_text(s, 50)),
            cap_text(&l.order_type, 20),
            l.rtt_ms,
            l.validation_latency_ms,
            l.response_latency_ms,
            l.overhead_ms,
            l.total_latency_ms,
            l.status,
            l.error.as_deref().map(|s| cap_text(s, 500)),
        ],
    )?;
    Ok(())
}

pub fn recent_latency(conn: &Connection, limit: Option<i64>) -> Result<Vec<LatencyRow>> {
    let mut st = conn.prepare(
        "SELECT id, timestamp, order_id, broker, symbol, order_type, rtt_ms, validation_latency_ms,
                response_latency_ms, overhead_ms, total_latency_ms, status, error
         FROM order_latency ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = st
        .query_map([limit.unwrap_or(-1)], |r| {
            Ok(LatencyRow {
                id: r.get(0)?,
                timestamp: r.get(1)?,
                order_id: r.get(2)?,
                broker: r.get(3)?,
                symbol: r.get(4)?,
                order_type: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                rtt_ms: r.get::<_, Option<f64>>(6)?.unwrap_or(0.0),
                validation_latency_ms: r.get::<_, Option<f64>>(7)?.unwrap_or(0.0),
                response_latency_ms: r.get::<_, Option<f64>>(8)?.unwrap_or(0.0),
                overhead_ms: r.get::<_, Option<f64>>(9)?.unwrap_or(0.0),
                total_latency_ms: r.get(10)?,
                status: r.get::<_, Option<String>>(11)?.unwrap_or_default(),
                error: r.get(12)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// `(broker, rtt_ms, total_ms, failed, timestamp)` for statistics.
/// `(broker, rtt_ms, total_ms, failed, timestamp)`.
pub type LatencyPoint = (Option<String>, f64, f64, bool, String);

pub fn latency_points(conn: &Connection) -> Result<Vec<LatencyPoint>> {
    let mut st = conn.prepare(
        "SELECT broker, COALESCE(rtt_ms, 0), total_latency_ms, status = 'FAILED', timestamp
         FROM order_latency ORDER BY id",
    )?;
    let rows = st
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get::<_, Option<bool>>(3)?.unwrap_or(false),
                r.get(4)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}
