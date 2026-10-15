//! Historify table operations (web `database/historify_db.py`). Every
//! function takes a borrowed connection; callers decide read versus
//! serialized mutation through [`crate::db::duckdb::HistorifyDb`]. Job rows
//! are `jobs`, schedule rows `schedules`.

use super::interval::{self, Kind};
use super::time::{http_date, ist_date};
use crate::brokers::types::Candle;
use crate::db::duckdb::migrations::next_id;
use crate::{clock::IST_OFFSET_SECS, error::Result};
use chrono::NaiveDateTime;
use duckdb::types::Value as Dv;
use duckdb::{params, params_from_iter, Connection};
use serde_json::{json, Map, Value};

mod jobs;
mod schedules;

pub use jobs::*;
pub use schedules::*;

/// One candle as stored and served.
pub type Bar = Candle;

/// Column kinds for [`rows_json`].
#[derive(Clone, Copy)]
pub enum Ty {
    Str,
    Int,
    Float,
    Bool,
    /// A timestamp selected as text; served as Flask serializes a raw
    /// datetime (RFC 1123, "GMT").
    Http,
}

/// Python `isoformat()` of a stored naive timestamp (seconds precision).
pub fn iso(col: &str) -> String {
    format!("strftime({}, '%Y-%m-%dT%H:%M:%S')", col)
}

pub fn ts_param(t: NaiveDateTime) -> String {
    t.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Run `sql` and shape each row as a JSON object with `cols`.
pub fn rows_json(
    c: &Connection,
    sql: &str,
    params: Vec<Dv>,
    cols: &[(&str, Ty)],
) -> Result<Vec<Value>> {
    let mut st = c.prepare(sql)?;
    let mut rows = st.query(params_from_iter(params))?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let mut m = Map::new();
        for (i, (name, ty)) in cols.iter().enumerate() {
            let v = match ty {
                Ty::Str => r.get::<_, Option<String>>(i)?.map(Value::from),
                Ty::Int => r.get::<_, Option<i64>>(i)?.map(Value::from),
                Ty::Float => r
                    .get::<_, Option<f64>>(i)?
                    .filter(|f| f.is_finite())
                    .map(Value::from),
                Ty::Bool => r.get::<_, Option<bool>>(i)?.map(Value::from),
                Ty::Http => r
                    .get::<_, Option<String>>(i)?
                    .and_then(|s| http_date(&s))
                    .map(Value::from),
            };
            m.insert(name.to_string(), v.unwrap_or(Value::Null));
        }
        out.push(Value::Object(m));
    }
    Ok(out)
}

fn text(s: impl Into<String>) -> Dv {
    Dv::Text(s.into())
}

fn opt_text(s: Option<&str>) -> Dv {
    s.map(text).unwrap_or(Dv::Null)
}

// ------------------------------------------------------------- watchlist

pub fn watchlist(c: &Connection) -> Result<Vec<Value>> {
    rows_json(
        c,
        &format!(
            "SELECT id, symbol, exchange, display_name, {} FROM watchlist ORDER BY added_at DESC",
            iso("added_at")
        ),
        vec![],
        &[
            ("id", Ty::Int),
            ("symbol", Ty::Str),
            ("exchange", Ty::Str),
            ("display_name", Ty::Str),
            ("added_at", Ty::Http),
        ],
    )
}

pub fn watchlist_symbols(c: &Connection) -> Result<Vec<(String, String)>> {
    let mut st = c.prepare("SELECT symbol, exchange FROM watchlist ORDER BY added_at DESC")?;
    let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

fn watch_set(c: &Connection) -> Result<std::collections::HashSet<(String, String)>> {
    Ok(watchlist_symbols(c)?.into_iter().collect())
}

/// Web `add_to_watchlist`: (already present?, message).
pub fn watchlist_add(
    c: &Connection,
    symbol: &str,
    exchange: &str,
    display_name: Option<&str>,
    now: NaiveDateTime,
) -> Result<String> {
    let (s, e) = (symbol.to_uppercase(), exchange.to_uppercase());
    if watch_set(c)?.contains(&(s.clone(), e.clone())) {
        return Ok(format!("{} already in watchlist", symbol));
    }
    let id = next_id(c, "watchlist_id_seq")?;
    c.execute(
        "INSERT INTO watchlist (id, symbol, exchange, display_name, added_at) \
         VALUES (?, ?, ?, ?, CAST(? AS TIMESTAMP))",
        params_from_iter(vec![
            Dv::BigInt(id),
            text(s),
            text(e),
            opt_text(display_name),
            text(ts_param(now)),
        ]),
    )?;
    Ok(format!("Added {} to watchlist", symbol))
}

/// One requested watchlist entry (already upper-cased by the caller or not).
#[derive(Debug, Clone, Default)]
pub struct SymbolReq {
    pub symbol: String,
    pub exchange: String,
    pub display_name: Option<String>,
}

fn failed(symbol: &str, exchange: &str, error: &str) -> Value {
    json!({"symbol": symbol, "exchange": exchange, "error": error})
}

/// Web `bulk_add_to_watchlist`: (added, skipped, failed).
pub fn watchlist_bulk_add(
    c: &Connection,
    items: &[SymbolReq],
    now: NaiveDateTime,
) -> Result<(i64, i64, Vec<Value>)> {
    let mut existing = watch_set(c)?;
    let (mut added, mut skipped, mut fails) = (0, 0, Vec::new());
    for it in items {
        let (s, e) = (it.symbol.to_uppercase(), it.exchange.to_uppercase());
        if s.is_empty() || e.is_empty() {
            fails.push(failed(&s, &e, "Missing symbol or exchange"));
            continue;
        }
        if !existing.insert((s.clone(), e.clone())) {
            skipped += 1;
            continue;
        }
        let id = next_id(c, "watchlist_id_seq")?;
        c.execute(
            "INSERT INTO watchlist (id, symbol, exchange, display_name, added_at) \
             VALUES (?, ?, ?, ?, CAST(? AS TIMESTAMP))",
            params_from_iter(vec![
                Dv::BigInt(id),
                text(s),
                text(e),
                opt_text(it.display_name.as_deref()),
                text(ts_param(now)),
            ]),
        )?;
        added += 1;
    }
    Ok((added, skipped, fails))
}

pub fn watchlist_remove(c: &Connection, symbol: &str, exchange: &str) -> Result<String> {
    c.execute(
        "DELETE FROM watchlist WHERE symbol = ? AND exchange = ?",
        params![symbol.to_uppercase(), exchange.to_uppercase()],
    )?;
    Ok(format!("Removed {} from watchlist", symbol))
}

/// Web `bulk_remove_from_watchlist`: (removed, skipped, failed).
pub fn watchlist_bulk_remove(
    c: &Connection,
    items: &[SymbolReq],
) -> Result<(i64, i64, Vec<Value>)> {
    let mut existing = watch_set(c)?;
    let (mut removed, mut skipped, mut fails) = (0, 0, Vec::new());
    for it in items {
        let (s, e) = (it.symbol.to_uppercase(), it.exchange.to_uppercase());
        if s.is_empty() || e.is_empty() {
            fails.push(failed(
                if s.is_empty() { "MISSING" } else { &s },
                if e.is_empty() { "MISSING" } else { &e },
                "Missing symbol or exchange",
            ));
            continue;
        }
        if !existing.remove(&(s.clone(), e.clone())) {
            skipped += 1;
            continue;
        }
        c.execute(
            "DELETE FROM watchlist WHERE symbol = ? AND exchange = ?",
            params![s, e],
        )?;
        removed += 1;
    }
    Ok((removed, skipped, fails))
}

// ----------------------------------------------------------- market data

/// Web `upsert_market_data`: candles and the catalog row in one
/// transaction (the caller's). Duplicate timestamps keep the last candle.
pub fn upsert_bars(
    c: &Connection,
    symbol: &str,
    exchange: &str,
    interval: &str,
    bars: &[Bar],
    now: NaiveDateTime,
) -> Result<usize> {
    if bars.is_empty() {
        return Ok(0);
    }
    let (s, e) = (symbol.to_uppercase(), exchange.to_uppercase());
    c.execute_batch(
        "CREATE OR REPLACE TEMP TABLE hfy_stage (seq BIGINT, timestamp BIGINT, open DOUBLE, \
         high DOUBLE, low DOUBLE, close DOUBLE, volume BIGINT, oi BIGINT)",
    )?;
    {
        let mut app = c.appender_to_catalog_and_db("hfy_stage", "temp", "main")?;
        for (i, b) in bars.iter().enumerate() {
            app.append_row(params![
                i as i64,
                b.timestamp,
                b.open,
                b.high,
                b.low,
                b.close,
                b.volume,
                b.oi
            ])?;
        }
        app.flush()?;
    }
    c.execute(
        "INSERT INTO market_data
            (symbol, exchange, interval, timestamp, open, high, low, close, volume, oi, created_at)
         SELECT ?, ?, ?, timestamp, open, high, low, close, volume, oi, CAST(? AS TIMESTAMP)
         FROM hfy_stage
         QUALIFY row_number() OVER (PARTITION BY timestamp ORDER BY seq DESC) = 1
         ON CONFLICT (symbol, exchange, interval, timestamp) DO UPDATE SET
            open = EXCLUDED.open, high = EXCLUDED.high, low = EXCLUDED.low,
            close = EXCLUDED.close, volume = EXCLUDED.volume, oi = EXCLUDED.oi",
        params![s, e, interval, ts_param(now)],
    )?;
    c.execute_batch("DROP TABLE IF EXISTS temp.main.hfy_stage")?;
    refresh_catalog(c, &s, &e, interval, now)?;
    Ok(bars.len())
}

/// Recompute one catalog row from the stored candles (insert or update).
fn refresh_catalog(
    c: &Connection,
    s: &str,
    e: &str,
    interval: &str,
    now: NaiveDateTime,
) -> Result<()> {
    let exists: i64 = c.query_row(
        "SELECT COUNT(*) FROM data_catalog WHERE symbol = ? AND exchange = ? AND interval = ?",
        params![s, e, interval],
        |r| r.get(0),
    )?;
    if exists > 0 {
        c.execute(
            "UPDATE data_catalog SET
                first_timestamp = (SELECT MIN(timestamp) FROM market_data
                                   WHERE symbol = $2 AND exchange = $3 AND interval = $4),
                last_timestamp = (SELECT MAX(timestamp) FROM market_data
                                  WHERE symbol = $2 AND exchange = $3 AND interval = $4),
                record_count = (SELECT COUNT(*) FROM market_data
                                WHERE symbol = $2 AND exchange = $3 AND interval = $4),
                last_download_at = CAST($1 AS TIMESTAMP)
             WHERE symbol = $2 AND exchange = $3 AND interval = $4",
            params![ts_param(now), s, e, interval],
        )?;
    } else {
        let id = next_id(c, "data_catalog_id_seq")?;
        c.execute(
            "INSERT INTO data_catalog
                (id, symbol, exchange, interval, first_timestamp, last_timestamp,
                 record_count, last_download_at)
             SELECT ?, ?, ?, ?, MIN(timestamp), MAX(timestamp), COUNT(*), CAST(? AS TIMESTAMP)
             FROM market_data WHERE symbol = ? AND exchange = ? AND interval = ?",
            params![id, s, e, interval, ts_param(now), s, e, interval],
        )?;
    }
    Ok(())
}

fn range_clause(start: Option<i64>, end: Option<i64>, params: &mut Vec<Dv>) -> String {
    let mut sql = String::new();
    // Web `if start_timestamp:` treats 0 as absent.
    if let Some(s) = start.filter(|s| *s != 0) {
        sql.push_str(" AND timestamp >= ?");
        params.push(Dv::BigInt(s));
    }
    if let Some(e) = end.filter(|e| *e != 0) {
        sql.push_str(" AND timestamp <= ?");
        params.push(Dv::BigInt(e));
    }
    sql
}

fn bars(c: &Connection, sql: &str, params: Vec<Dv>) -> Result<Vec<Bar>> {
    let mut st = c.prepare(sql)?;
    let rows = st.query_map(params_from_iter(params), |r| {
        Ok(Bar {
            timestamp: r.get(0)?,
            open: r.get(1)?,
            high: r.get(2)?,
            low: r.get(3)?,
            close: r.get(4)?,
            volume: r.get(5)?,
            oi: r.get::<_, Option<i64>>(6)?.unwrap_or(0),
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// The bucket expression aligning 1m candles to the exchange's open.
fn intraday_bucket(open_secs: i64, interval_secs: i64) -> String {
    format!(
        "CAST((FLOOR((timestamp + {o}) / 86400) * 86400 - {o}) + {m} + \
         FLOOR((((timestamp + {o}) % 86400) - {m}) / {i}) * {i} AS BIGINT)",
        o = IST_OFFSET_SECS,
        m = open_secs,
        i = interval_secs
    )
}

const AGG_COLS: &str = "FIRST(open ORDER BY timestamp), MAX(high), MIN(low), \
    LAST(close ORDER BY timestamp), CAST(SUM(volume) AS BIGINT), LAST(oi ORDER BY timestamp)";

/// Web `_get_aggregated_ohlcv` (`after_open` adds the export's filter that
/// drops candles before the open).
pub fn aggregated_intraday(
    c: &Connection,
    symbol: &str,
    exchange: &str,
    minutes: u32,
    start: Option<i64>,
    end: Option<i64>,
    after_open: bool,
) -> Result<Vec<Bar>> {
    let open = interval::market_open_seconds(exchange);
    let bucket = intraday_bucket(open, i64::from(minutes) * 60);
    let mut params = vec![text(symbol.to_uppercase()), text(exchange.to_uppercase())];
    let mut sql = format!(
        "SELECT {b} AS ts, {a} FROM market_data WHERE symbol = ? AND exchange = ? AND interval = '1m'",
        b = bucket,
        a = AGG_COLS
    );
    if after_open {
        sql.push_str(&format!(
            " AND ((timestamp + {}) % 86400) >= {}",
            IST_OFFSET_SECS, open
        ));
    }
    sql.push_str(&range_clause(start, end, &mut params));
    sql.push_str(&format!(" GROUP BY {} ORDER BY ts ASC", bucket));
    bars(c, &sql, params)
}

/// The group key for W/M/Q/Y over the IST calendar.
fn daily_group(kind: Kind, value: u32) -> Option<String> {
    let t = format!(
        "make_timestamp(CAST(timestamp + {} AS BIGINT) * 1000000)",
        IST_OFFSET_SECS
    );
    let n = i64::from(value);
    Some(match kind {
        Kind::Weekly if n == 1 => format!("DATE_TRUNC('week', {})", t),
        Kind::Weekly => format!(
            "DATE_TRUNC('week', {t}) - to_weeks(CAST((EXTRACT(WEEK FROM {t}) - 1) % {n} AS INTEGER))"
        ),
        Kind::Monthly if n == 1 => format!("DATE_TRUNC('month', {})", t),
        Kind::Monthly => format!(
            "DATE_TRUNC('month', {t}) - to_months(CAST((EXTRACT(MONTH FROM {t}) - 1) % {n} AS INTEGER))"
        ),
        Kind::Quarterly if n == 1 => format!("DATE_TRUNC('quarter', {})", t),
        Kind::Quarterly => format!(
            "DATE_TRUNC('quarter', {t}) - to_months(CAST(3 * ((EXTRACT(QUARTER FROM {t}) - 1) % {n}) AS INTEGER))"
        ),
        Kind::Yearly if n == 1 => format!("DATE_TRUNC('year', {})", t),
        Kind::Yearly => format!(
            "DATE_TRUNC('year', {t}) - to_years(CAST(EXTRACT(YEAR FROM {t}) % {n} AS INTEGER))"
        ),
        _ => return None,
    })
}

/// Web `_get_daily_aggregated_ohlcv`: W/M/Q/Y from stored D candles. The
/// timestamp is the UTC epoch of the IST period start date.
pub fn aggregated_daily(
    c: &Connection,
    symbol: &str,
    exchange: &str,
    target: &str,
    start: Option<i64>,
    end: Option<i64>,
) -> Result<Vec<Bar>> {
    let Some(p) = interval::parse(target) else {
        return Ok(Vec::new());
    };
    let Some(group) = daily_group(p.kind, p.value) else {
        return Ok(Vec::new());
    };
    let mut params = vec![text(symbol.to_uppercase()), text(exchange.to_uppercase())];
    let mut sql = format!(
        "SELECT CAST(epoch_ms({g}) // 1000 AS BIGINT) AS ts, {a} FROM market_data \
         WHERE symbol = ? AND exchange = ? AND interval = 'D'",
        g = group,
        a = AGG_COLS
    );
    sql.push_str(&range_clause(start, end, &mut params));
    sql.push_str(&format!(" GROUP BY {} ORDER BY ts ASC", group));
    bars(c, &sql, params)
}

/// Stored candles for one interval, as stored.
pub fn stored(
    c: &Connection,
    symbol: &str,
    exchange: &str,
    interval: &str,
    start: Option<i64>,
    end: Option<i64>,
) -> Result<Vec<Bar>> {
    let mut params = vec![
        text(symbol.to_uppercase()),
        text(exchange.to_uppercase()),
        text(interval),
    ];
    let mut sql = "SELECT timestamp, open, high, low, close, volume, oi FROM market_data \
                   WHERE symbol = ? AND exchange = ? AND interval = ?"
        .to_string();
    sql.push_str(&range_clause(start, end, &mut params));
    sql.push_str(" ORDER BY timestamp ASC");
    bars(c, &sql, params)
}

/// Web `get_ohlcv`: stored intervals read directly, intraday intervals
/// aggregated from 1m, W/M/Q/Y from D.
pub fn ohlcv(
    c: &Connection,
    symbol: &str,
    exchange: &str,
    interval: &str,
    start: Option<i64>,
    end: Option<i64>,
) -> Result<Vec<Bar>> {
    if interval::is_daily_aggregated(interval) {
        return aggregated_daily(c, symbol, exchange, interval, start, end);
    }
    if interval::is_intraday_computed(interval) {
        let Some(p) = interval::parse(interval) else {
            return Ok(Vec::new());
        };
        return aggregated_intraday(c, symbol, exchange, p.minutes, start, end, false);
    }
    stored(c, symbol, exchange, interval, start, end)
}

/// Stored candles of a source interval in a range (export pre-checks).
pub fn count_source(
    c: &Connection,
    symbol: &str,
    exchange: &str,
    source: &str,
    start: Option<i64>,
    end: Option<i64>,
) -> Result<i64> {
    let mut params = vec![text(symbol), text(exchange), text(source)];
    let mut sql =
        "SELECT COUNT(*) FROM market_data WHERE symbol = ? AND exchange = ? AND interval = ?"
            .to_string();
    sql.push_str(&range_clause(start, end, &mut params));
    Ok(c.query_row(&sql, params_from_iter(params), |r| r.get(0))?)
}

// ---------------------------------------------------------------- catalog

const CATALOG_COLS: &[(&str, Ty)] = &[
    ("symbol", Ty::Str),
    ("exchange", Ty::Str),
    ("interval", Ty::Str),
    ("first_timestamp", Ty::Int),
    ("last_timestamp", Ty::Int),
    ("record_count", Ty::Int),
    ("last_download_at", Ty::Http),
];

fn add_dates(rows: &mut [Value]) {
    for r in rows.iter_mut() {
        let f = r.get("first_timestamp").and_then(Value::as_i64);
        let l = r.get("last_timestamp").and_then(Value::as_i64);
        if let Some(o) = r.as_object_mut() {
            // Web `if item.get("first_timestamp"):` skips 0 as well as null.
            if let Some(f) = f.filter(|v| *v != 0) {
                o.insert("first_date".into(), json!(ist_date(f)));
            }
            if let Some(l) = l.filter(|v| *v != 0) {
                o.insert("last_date".into(), json!(ist_date(l)));
            }
        }
    }
}

/// Web `get_data_catalog` plus the service's readable dates.
pub fn catalog(c: &Connection) -> Result<Vec<Value>> {
    let mut rows = rows_json(
        c,
        &format!(
            "SELECT symbol, exchange, interval, first_timestamp, last_timestamp, record_count, {} \
             FROM data_catalog ORDER BY exchange, symbol, interval",
            iso("last_download_at")
        ),
        vec![],
        CATALOG_COLS,
    )?;
    add_dates(&mut rows);
    Ok(rows)
}

/// Web `get_catalog_with_metadata` plus readable dates.
pub fn catalog_with_metadata(c: &Connection) -> Result<Vec<Value>> {
    let mut cols = CATALOG_COLS.to_vec();
    cols.extend_from_slice(&[
        ("name", Ty::Str),
        ("expiry", Ty::Str),
        ("strike", Ty::Float),
        ("lotsize", Ty::Int),
        ("instrumenttype", Ty::Str),
        ("tick_size", Ty::Float),
    ]);
    let mut rows = rows_json(
        c,
        &format!(
            "SELECT c.symbol, c.exchange, c.interval, c.first_timestamp, c.last_timestamp,
                    c.record_count, {}, m.name, m.expiry, m.strike, m.lotsize,
                    m.instrumenttype, m.tick_size
             FROM data_catalog c
             LEFT JOIN symbol_metadata m ON c.symbol = m.symbol AND c.exchange = m.exchange
             ORDER BY c.exchange, m.name, c.symbol, c.interval",
            iso("c.last_download_at")
        ),
        vec![],
        &cols,
    )?;
    add_dates(&mut rows);
    Ok(rows)
}

/// Distinct (symbol, exchange) in the catalog, ordered for export.
pub fn catalog_symbols(c: &Connection) -> Result<Vec<(String, String)>> {
    let mut st =
        c.prepare("SELECT DISTINCT symbol, exchange FROM data_catalog ORDER BY symbol, exchange")?;
    let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// (first timestamp, last timestamp, record count).
pub type DataRange = (Option<i64>, Option<i64>, i64);

/// Web `get_data_range`: (first, last, count).
pub fn data_range(
    c: &Connection,
    symbol: &str,
    exchange: &str,
    interval: &str,
) -> Result<Option<DataRange>> {
    let mut st = c.prepare(
        "SELECT first_timestamp, last_timestamp, COALESCE(record_count, 0) FROM data_catalog \
         WHERE symbol = ? AND exchange = ? AND interval = ?",
    )?;
    let mut rows = st.query(params![
        symbol.to_uppercase(),
        exchange.to_uppercase(),
        interval
    ])?;
    Ok(match rows.next()? {
        Some(r) => Some((r.get(0)?, r.get(1)?, r.get(2)?)),
        None => None,
    })
}

/// Web `delete_market_data`.
pub fn delete_market_data(
    c: &Connection,
    symbol: &str,
    exchange: &str,
    interval: Option<&str>,
) -> Result<String> {
    let (s, e) = (symbol.to_uppercase(), exchange.to_uppercase());
    match interval {
        Some(i) => {
            c.execute(
                "DELETE FROM market_data WHERE symbol = ? AND exchange = ? AND interval = ?",
                params![s, e, i],
            )?;
            c.execute(
                "DELETE FROM data_catalog WHERE symbol = ? AND exchange = ? AND interval = ?",
                params![s, e, i],
            )?;
            Ok(format!("Deleted {}:{}:{} data", symbol, exchange, i))
        }
        None => {
            c.execute(
                "DELETE FROM market_data WHERE symbol = ? AND exchange = ?",
                params![s, e],
            )?;
            c.execute(
                "DELETE FROM data_catalog WHERE symbol = ? AND exchange = ?",
                params![s, e],
            )?;
            Ok(format!("Deleted all {}:{} data", symbol, exchange))
        }
    }
}

/// Web `bulk_delete_market_data`: (deleted, skipped, failed). A symbol with
/// no stored candles counts as skipped.
pub fn bulk_delete(c: &Connection, items: &[SymbolReq]) -> Result<(i64, i64, Vec<Value>)> {
    let (mut deleted, mut skipped, mut fails) = (0, 0, Vec::new());
    for it in items {
        let (s, e) = (it.symbol.to_uppercase(), it.exchange.to_uppercase());
        if s.is_empty() || e.is_empty() {
            fails.push(failed(
                if s.is_empty() { "MISSING" } else { &s },
                if e.is_empty() { "MISSING" } else { &e },
                "Missing symbol or exchange",
            ));
            continue;
        }
        let n = c.execute(
            "DELETE FROM market_data WHERE symbol = ? AND exchange = ?",
            params![s, e],
        )?;
        c.execute(
            "DELETE FROM data_catalog WHERE symbol = ? AND exchange = ?",
            params![s, e],
        )?;
        if n > 0 {
            deleted += 1;
        } else {
            skipped += 1;
        }
    }
    Ok((deleted, skipped, fails))
}

/// (total candles, distinct symbol-exchange pairs, watchlist size).
pub fn stats(c: &Connection) -> Result<(i64, i64, i64)> {
    let total: i64 = c.query_row("SELECT COUNT(*) FROM market_data", [], |r| r.get(0))?;
    let symbols: i64 = c.query_row(
        "SELECT COUNT(DISTINCT symbol || exchange) FROM market_data",
        [],
        |r| r.get(0),
    )?;
    let watch: i64 = c.query_row("SELECT COUNT(*) FROM watchlist", [], |r| r.get(0))?;
    Ok((total, symbols, watch))
}

// --------------------------------------------------------------- metadata

/// Web `upsert_symbol_metadata`.
pub fn upsert_metadata(c: &Connection, rows: &[Value], now: NaiveDateTime) -> Result<usize> {
    for m in rows {
        let s = |k: &str| m.get(k).and_then(Value::as_str).map(str::to_string);
        let sym = s("symbol").unwrap_or_default().to_uppercase();
        let exch = s("exchange").unwrap_or_default().to_uppercase();
        let strike = m.get("strike").and_then(Value::as_f64);
        let lot = m.get("lotsize").and_then(Value::as_i64);
        let tick = m.get("tick_size").and_then(Value::as_f64);
        c.execute(
            "INSERT INTO symbol_metadata
                (symbol, exchange, name, expiry, strike, lotsize, instrumenttype, tick_size, last_updated)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP))
             ON CONFLICT (symbol, exchange) DO UPDATE SET
                name = EXCLUDED.name, expiry = EXCLUDED.expiry, strike = EXCLUDED.strike,
                lotsize = EXCLUDED.lotsize, instrumenttype = EXCLUDED.instrumenttype,
                tick_size = EXCLUDED.tick_size, last_updated = EXCLUDED.last_updated",
            params_from_iter(vec![
                text(sym),
                text(exch),
                opt_text(s("name").as_deref()),
                opt_text(s("expiry").as_deref()),
                strike.map(Dv::Double).unwrap_or(Dv::Null),
                lot.map(Dv::BigInt).unwrap_or(Dv::Null),
                opt_text(s("instrumenttype").as_deref()),
                tick.map(Dv::Double).unwrap_or(Dv::Null),
                text(ts_param(now)),
            ]),
        )?;
    }
    Ok(rows.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::duckdb::HistorifyDb;
    use chrono::NaiveDate;

    fn now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, 7)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap()
    }

    fn bar(ts: i64, c: f64, v: i64) -> Bar {
        Bar {
            timestamp: ts,
            open: c,
            high: c + 1.0,
            low: c - 1.0,
            close: c,
            volume: v,
            oi: 0,
        }
    }

    /// Web #2117 `cancel_job`: a cancel never overwrites a job that has
    /// already finished; an open one (queued, running, paused) is cancelled.
    #[test]
    fn cancel_only_reaches_an_open_job() {
        let db = HistorifyDb::in_memory().unwrap();
        let status = |id: &str| {
            let id = id.to_string();
            db.read(move |c| job(c, &id)).unwrap().unwrap()["status"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let config = serde_json::json!({});
        let symbols = vec![("SBIN".to_string(), "NSE".to_string())];
        for (id, start, cancels) in [
            ("queued", "pending", true),
            ("live", "running", true),
            ("held", "paused", true),
            ("done", "completed", false),
            ("partial", "completed_with_errors", false),
            ("broken", "failed", false),
            ("gone", "cancelled", false),
        ] {
            db.mutate(|c| {
                create_job(
                    c,
                    &NewJob {
                        id,
                        job_type: "watchlist",
                        symbols: &symbols,
                        interval: "D",
                        start_date: None,
                        end_date: None,
                        config: &config,
                    },
                    now(),
                )?;
                set_job_status(c, id, start, None, now())
            })
            .unwrap();
            let did = db.mutate(|c| cancel_if_open(c, id, now())).unwrap();
            assert_eq!(did, cancels, "{}", id);
            let want = if cancels { "cancelled" } else { start };
            assert_eq!(status(id), want, "{}", id);
        }
    }

    #[test]
    fn upsert_keeps_last_duplicate_and_refreshes_catalog() {
        let db = HistorifyDb::in_memory().unwrap();
        let n = db
            .mutate(|c| {
                upsert_bars(
                    c,
                    "sbin",
                    "nse",
                    "D",
                    &[bar(100, 1.0, 5), bar(200, 2.0, 6), bar(100, 3.0, 7)],
                    now(),
                )
            })
            .unwrap();
        assert_eq!(n, 3);
        let rows = db
            .read(|c| stored(c, "SBIN", "NSE", "D", None, None))
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].close, 3.0);
        // A second download of the same symbol updates rows and catalog.
        db.mutate(|c| upsert_bars(c, "SBIN", "NSE", "D", &[bar(300, 4.0, 1)], now()))
            .unwrap();
        db.mutate(|c| upsert_bars(c, "SBIN", "NSE", "D", &[bar(300, 5.0, 1)], now()))
            .unwrap();
        let cat = db.read(catalog).unwrap();
        assert_eq!(cat[0]["record_count"], 3);
        assert_eq!(cat[0]["last_timestamp"], 300);
        db.mutate(|c| delete_market_data(c, "SBIN", "NSE", Some("D")))
            .unwrap();
        db.mutate(|c| {
            upsert_bars(
                c,
                "SBIN",
                "NSE",
                "D",
                &[bar(100, 3.0, 7), bar(200, 2.0, 6)],
                now(),
            )
        })
        .unwrap();
        let cat = db.read(catalog).unwrap();
        assert_eq!(cat[0]["record_count"], 2);
        assert_eq!(cat[0]["first_timestamp"], 100);
        assert_eq!(cat[0]["last_timestamp"], 200);
        assert_eq!(db.open_connections(), 0);
    }

    #[test]
    fn intraday_aggregation_aligns_to_the_open() {
        let db = HistorifyDb::in_memory().unwrap();
        // 2024-01-01 09:15 IST = 03:45 UTC.
        let open = 1_704_080_700;
        let bars: Vec<Bar> = (0..10)
            .map(|i| bar(open + i * 60, 100.0 + i as f64, 1))
            .collect();
        db.mutate(|c| upsert_bars(c, "SBIN", "NSE", "1m", &bars, now()))
            .unwrap();
        let five = db
            .read(|c| ohlcv(c, "SBIN", "NSE", "5m", None, None))
            .unwrap();
        assert_eq!(five.len(), 2);
        assert_eq!(five[0].timestamp, open);
        assert_eq!(five[1].timestamp, open + 300);
        assert_eq!(five[0].open, 100.0);
        assert_eq!(five[0].close, 104.0);
        assert_eq!(five[0].volume, 5);
    }

    #[test]
    fn weekly_aggregation_uses_ist_calendar() {
        let db = HistorifyDb::in_memory().unwrap();
        // Daily candles at IST midnight, 2024-01-01 (Mon) .. 2024-01-10.
        let day0 = 1_704_047_400; // 2024-01-01 00:00 IST
        let bars: Vec<Bar> = (0..10).map(|i| bar(day0 + i * 86_400, 10.0, 1)).collect();
        db.mutate(|c| upsert_bars(c, "SBIN", "NSE", "D", &bars, now()))
            .unwrap();
        let w = db
            .read(|c| ohlcv(c, "SBIN", "NSE", "W", None, None))
            .unwrap();
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].timestamp, 1_704_067_200); // 2024-01-01 00:00 UTC
        assert_eq!(w[0].volume, 7);
        let m = db
            .read(|c| ohlcv(c, "SBIN", "NSE", "M", None, None))
            .unwrap();
        assert_eq!(m.len(), 1);
        let q2 = db
            .read(|c| ohlcv(c, "SBIN", "NSE", "2Q", None, None))
            .unwrap();
        assert_eq!(q2.len(), 1);
        let y = db
            .read(|c| ohlcv(c, "SBIN", "NSE", "2Y", None, None))
            .unwrap();
        assert_eq!(y.len(), 1);
    }
}
