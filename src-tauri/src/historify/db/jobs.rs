//! Download job rows (web `historify_jobs` and `job_items`): create a
//! job with its items, read them, and move a job and each item through
//! their statuses.

use super::{iso, opt_text, rows_json, text, ts_param, Ty};
use crate::db::duckdb::migrations::next_id;
use crate::error::Result;
use chrono::NaiveDateTime;
use duckdb::types::Value as Dv;
use duckdb::{params, params_from_iter, Connection};
use serde_json::Value;

pub const JOB_COLS_FULL: &str = "id, job_type, status, total_symbols, completed_symbols, \
    failed_symbols, interval, start_date, end_date, config";

fn job_cols(full: bool) -> Vec<(&'static str, Ty)> {
    let mut v = vec![
        ("id", Ty::Str),
        ("job_type", Ty::Str),
        ("status", Ty::Str),
        ("total_symbols", Ty::Int),
        ("completed_symbols", Ty::Int),
        ("failed_symbols", Ty::Int),
        ("interval", Ty::Str),
        ("start_date", Ty::Str),
        ("end_date", Ty::Str),
    ];
    if full {
        v.push(("config", Ty::Str));
    }
    v.extend_from_slice(&[
        ("created_at", Ty::Str),
        ("started_at", Ty::Str),
        ("completed_at", Ty::Str),
    ]);
    if full {
        v.push(("error_message", Ty::Str));
    }
    v
}

fn job_select(full: bool) -> String {
    let base = "id, job_type, status, total_symbols, completed_symbols, failed_symbols, \
                interval, start_date, end_date";
    format!(
        "SELECT {}{}, {}, {}, {}{} FROM download_jobs",
        base,
        if full { ", config" } else { "" },
        iso("created_at"),
        iso("started_at"),
        iso("completed_at"),
        if full { ", error_message" } else { "" }
    )
}

/// One new job and its items.
pub struct NewJob<'a> {
    pub id: &'a str,
    pub job_type: &'a str,
    pub symbols: &'a [(String, String)],
    pub interval: &'a str,
    pub start_date: Option<&'a str>,
    pub end_date: Option<&'a str>,
    pub config: &'a Value,
}

/// Web `create_download_job`.
pub fn create_job(c: &Connection, j: &NewJob<'_>, now: NaiveDateTime) -> Result<()> {
    c.execute(
        "INSERT INTO download_jobs
            (id, job_type, status, total_symbols, interval, start_date, end_date, config, created_at)
         VALUES (?, ?, 'pending', ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP))",
        params_from_iter(vec![
            text(j.id),
            text(j.job_type),
            Dv::BigInt(j.symbols.len() as i64),
            text(j.interval),
            opt_text(j.start_date),
            opt_text(j.end_date),
            if j.config.is_null() {
                Dv::Null
            } else {
                text(j.config.to_string())
            },
            text(ts_param(now)),
        ]),
    )?;
    for (s, e) in j.symbols {
        let id = next_id(c, "job_items_id_seq")?;
        c.execute(
            "INSERT INTO job_items (id, job_id, symbol, exchange, status) VALUES (?, ?, ?, ?, 'pending')",
            params![id, j.id, s.to_uppercase(), e.to_uppercase()],
        )?;
    }
    Ok(())
}

/// Web `get_download_job`.
pub fn job(c: &Connection, id: &str) -> Result<Option<Value>> {
    Ok(rows_json(
        c,
        &format!("{} WHERE id = ?", job_select(true)),
        vec![text(id)],
        &job_cols(true),
    )?
    .into_iter()
    .next())
}

/// Web `get_all_download_jobs`.
pub fn jobs(c: &Connection, status: Option<&str>, limit: i64) -> Result<Vec<Value>> {
    let (filter, mut params) = match status {
        Some(s) => (" WHERE status = ?", vec![text(s)]),
        None => ("", vec![]),
    };
    params.push(Dv::BigInt(limit));
    rows_json(
        c,
        &format!(
            "{}{} ORDER BY created_at DESC LIMIT ?",
            job_select(false),
            filter
        ),
        params,
        &job_cols(false),
    )
}

/// Ids of jobs in any of `statuses`.
pub fn job_ids_with_status(c: &Connection, statuses: &[&str]) -> Result<Vec<String>> {
    let marks = vec!["?"; statuses.len()].join(", ");
    let mut st = c.prepare(&format!(
        "SELECT id FROM download_jobs WHERE status IN ({}) ORDER BY created_at",
        marks
    ))?;
    let rows = st.query_map(params_from_iter(statuses.iter()), |r| r.get(0))?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// One job item as the processor sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: i64,
    pub symbol: String,
    pub exchange: String,
    pub status: String,
}

pub fn job_items(c: &Connection, job_id: &str, status: Option<&str>) -> Result<Vec<Value>> {
    let (filter, mut params) = match status {
        Some(s) => (" AND status = ?", vec![text(s)]),
        None => ("", vec![]),
    };
    params.insert(0, text(job_id));
    rows_json(
        c,
        &format!(
            "SELECT id, job_id, symbol, exchange, status, records_downloaded, error_message, {}, {} \
             FROM job_items WHERE job_id = ?{} ORDER BY id",
            iso("started_at"),
            iso("completed_at"),
            filter
        ),
        params,
        &[
            ("id", Ty::Int),
            ("job_id", Ty::Str),
            ("symbol", Ty::Str),
            ("exchange", Ty::Str),
            ("status", Ty::Str),
            ("records_downloaded", Ty::Int),
            ("error_message", Ty::Str),
            ("started_at", Ty::Str),
            ("completed_at", Ty::Str),
        ],
    )
}

pub fn items(c: &Connection, job_id: &str) -> Result<Vec<Item>> {
    let mut st = c.prepare(
        "SELECT id, symbol, exchange, status FROM job_items WHERE job_id = ? ORDER BY id",
    )?;
    let rows = st.query_map([job_id], |r| {
        Ok(Item {
            id: r.get(0)?,
            symbol: r.get(1)?,
            exchange: r.get(2)?,
            status: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// Web `update_job_status`.
pub fn set_job_status(
    c: &Connection,
    id: &str,
    status: &str,
    error: Option<&str>,
    now: NaiveDateTime,
) -> Result<()> {
    match status {
        "running" => c.execute(
            "UPDATE download_jobs SET status = ?, started_at = CAST(? AS TIMESTAMP) WHERE id = ?",
            params![status, ts_param(now), id],
        )?,
        "completed" | "completed_with_errors" | "failed" | "cancelled" => c.execute(
            "UPDATE download_jobs SET status = ?, completed_at = CAST(? AS TIMESTAMP), \
             error_message = ? WHERE id = ?",
            params_from_iter(vec![
                text(status),
                text(ts_param(now)),
                opt_text(error),
                text(id),
            ]),
        )?,
        _ => c.execute(
            "UPDATE download_jobs SET status = ? WHERE id = ?",
            params![status, id],
        )?,
    };
    Ok(())
}

/// Mark a job cancelled only while it is still open (pending, running or
/// paused). False when it finished first: a cancel never overwrites a
/// finished job (web #2117 `cancel_job`, "Job has already finished").
pub fn cancel_if_open(c: &Connection, id: &str, now: NaiveDateTime) -> Result<bool> {
    let n = c.execute(
        "UPDATE download_jobs SET status = 'cancelled', completed_at = CAST(? AS TIMESTAMP), \
         error_message = NULL WHERE id = ? AND status IN ('pending', 'running', 'paused')",
        params![ts_param(now), id],
    )?;
    Ok(n == 1)
}

/// Web `update_job_item_status`.
pub fn set_item_status(
    c: &Connection,
    item_id: i64,
    status: &str,
    records: i64,
    error: Option<&str>,
    now: NaiveDateTime,
) -> Result<()> {
    match status {
        "downloading" => c.execute(
            "UPDATE job_items SET status = ?, started_at = CAST(? AS TIMESTAMP) WHERE id = ?",
            params![status, ts_param(now), item_id],
        )?,
        "success" | "error" | "skipped" => c.execute(
            "UPDATE job_items SET status = ?, records_downloaded = ?, error_message = ?, \
             completed_at = CAST(? AS TIMESTAMP) WHERE id = ?",
            params_from_iter(vec![
                text(status),
                Dv::BigInt(records),
                opt_text(error),
                text(ts_param(now)),
                Dv::BigInt(item_id),
            ]),
        )?,
        _ => c.execute(
            "UPDATE job_items SET status = ? WHERE id = ?",
            params![status, item_id],
        )?,
    };
    Ok(())
}

/// Reset a job's interrupted or failed items to pending.
pub fn reset_items(c: &Connection, job_id: &str, from: &[&str]) -> Result<usize> {
    let marks = vec!["?"; from.len()].join(", ");
    let mut p: Vec<Dv> = vec![text(job_id)];
    p.extend(from.iter().map(|s| text(*s)));
    Ok(c.execute(
        &format!(
            "UPDATE job_items SET status = 'pending' WHERE job_id = ? AND status IN ({})",
            marks
        ),
        params_from_iter(p),
    )?)
}

pub fn set_job_progress(c: &Connection, id: &str, completed: i64, failed: i64) -> Result<()> {
    c.execute(
        "UPDATE download_jobs SET completed_symbols = ?, failed_symbols = ? WHERE id = ?",
        params![completed, failed, id],
    )?;
    Ok(())
}

pub fn delete_job(c: &Connection, id: &str) -> Result<()> {
    c.execute("DELETE FROM job_items WHERE job_id = ?", [id])?;
    c.execute("DELETE FROM download_jobs WHERE id = ?", [id])?;
    Ok(())
}

/// Records downloaded by a job's items (execution summary).
pub fn job_records(c: &Connection, job_id: &str) -> Result<i64> {
    Ok(c.query_row(
        "SELECT CAST(COALESCE(SUM(records_downloaded), 0) AS BIGINT) FROM job_items WHERE job_id = ?",
        [job_id],
        |r| r.get(0),
    )?)
}
