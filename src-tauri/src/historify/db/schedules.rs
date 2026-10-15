//! Schedule rows (web `historify_schedules` and their executions): read,
//! create, update and delete a schedule, count its runs, and record each
//! execution.

use super::{iso, opt_text, rows_json, text, ts_param, Ty};
use crate::db::duckdb::migrations::next_id;
use crate::error::Result;
use chrono::NaiveDateTime;
use duckdb::types::Value as Dv;
use duckdb::{params, params_from_iter, Connection};
use serde_json::Value;

const SCHEDULE_COLS: &[(&str, Ty)] = &[
    ("id", Ty::Str),
    ("name", Ty::Str),
    ("description", Ty::Str),
    ("schedule_type", Ty::Str),
    ("interval_value", Ty::Int),
    ("interval_unit", Ty::Str),
    ("time_of_day", Ty::Str),
    ("download_source", Ty::Str),
    ("data_interval", Ty::Str),
    ("lookback_days", Ty::Int),
    ("is_enabled", Ty::Bool),
    ("is_paused", Ty::Bool),
    ("status", Ty::Str),
    ("apscheduler_job_id", Ty::Str),
    ("created_at", Ty::Str),
    ("last_run_at", Ty::Str),
    ("next_run_at", Ty::Str),
    ("last_run_status", Ty::Str),
    ("total_runs", Ty::Int),
    ("successful_runs", Ty::Int),
    ("failed_runs", Ty::Int),
];

fn schedule_select() -> String {
    format!(
        "SELECT id, name, description, schedule_type, interval_value, interval_unit, time_of_day, \
         download_source, data_interval, lookback_days, is_enabled, is_paused, status, \
         apscheduler_job_id, {}, {}, {}, last_run_status, total_runs, successful_runs, failed_runs \
         FROM historify_schedules",
        iso("created_at"),
        iso("last_run_at"),
        iso("next_run_at")
    )
}

pub fn schedule(c: &Connection, id: &str) -> Result<Option<Value>> {
    Ok(rows_json(
        c,
        &format!("{} WHERE id = ?", schedule_select()),
        vec![text(id)],
        SCHEDULE_COLS,
    )?
    .into_iter()
    .next())
}

pub fn schedules(c: &Connection, active_only: bool) -> Result<Vec<Value>> {
    let filter = if active_only {
        " WHERE is_enabled = TRUE AND is_paused = FALSE"
    } else {
        ""
    };
    rows_json(
        c,
        &format!("{}{} ORDER BY created_at DESC", schedule_select(), filter),
        vec![],
        SCHEDULE_COLS,
    )
}

/// A new schedule (web `create_schedule`).
pub struct NewSchedule<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub description: Option<&'a str>,
    pub schedule_type: &'a str,
    pub interval_value: Option<i64>,
    pub interval_unit: Option<&'a str>,
    pub time_of_day: Option<&'a str>,
    pub data_interval: &'a str,
    pub lookback_days: i64,
}

/// Err(message) when the id is taken.
pub fn create_schedule(
    c: &Connection,
    s: &NewSchedule<'_>,
    now: NaiveDateTime,
) -> Result<std::result::Result<(), String>> {
    let n: i64 = c.query_row(
        "SELECT COUNT(*) FROM historify_schedules WHERE id = ?",
        [s.id],
        |r| r.get(0),
    )?;
    if n > 0 {
        return Ok(Err(format!("Schedule ID '{}' already exists", s.id)));
    }
    c.execute(
        "INSERT INTO historify_schedules
            (id, name, description, schedule_type, interval_value, interval_unit, time_of_day,
             download_source, data_interval, lookback_days, is_enabled, is_paused, status, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, 'watchlist', ?, ?, TRUE, FALSE, 'idle', CAST(? AS TIMESTAMP))",
        params_from_iter(vec![
            text(s.id),
            text(s.name),
            opt_text(s.description),
            text(s.schedule_type),
            s.interval_value.map(Dv::BigInt).unwrap_or(Dv::Null),
            opt_text(s.interval_unit),
            opt_text(s.time_of_day),
            text(s.data_interval),
            Dv::BigInt(s.lookback_days),
            text(ts_param(now)),
        ]),
    )?;
    Ok(Ok(()))
}

/// Fields of a schedule update; `None` leaves the column alone.
#[derive(Debug, Default, Clone)]
pub struct ScheduleUpdate {
    pub name: Option<String>,
    pub description: Option<String>,
    pub schedule_type: Option<String>,
    pub interval_value: Option<i64>,
    pub interval_unit: Option<String>,
    pub time_of_day: Option<String>,
    pub data_interval: Option<String>,
    pub lookback_days: Option<i64>,
    pub is_enabled: Option<bool>,
    pub is_paused: Option<bool>,
    pub status: Option<String>,
    pub apscheduler_job_id: Option<String>,
    pub next_run_at: Option<NaiveDateTime>,
    pub last_run_at: Option<NaiveDateTime>,
    pub last_run_status: Option<String>,
}

impl ScheduleUpdate {
    /// Touches one of the trigger fields (web `config_fields`).
    pub fn changes_trigger(&self) -> bool {
        self.schedule_type.is_some()
            || self.interval_value.is_some()
            || self.interval_unit.is_some()
            || self.time_of_day.is_some()
    }
}

/// Web `update_schedule`: Err("No fields to update") when empty.
pub fn update_schedule(
    c: &Connection,
    id: &str,
    u: &ScheduleUpdate,
) -> Result<std::result::Result<(), String>> {
    let mut sets: Vec<&str> = Vec::new();
    let mut p: Vec<Dv> = Vec::new();
    let mut add = |col: &'static str, v: Dv| {
        sets.push(col);
        p.push(v);
    };
    if let Some(v) = &u.name {
        add("name = ?", text(v.as_str()));
    }
    if let Some(v) = &u.description {
        add("description = ?", text(v.as_str()));
    }
    if let Some(v) = &u.schedule_type {
        add("schedule_type = ?", text(v.as_str()));
    }
    if let Some(v) = u.interval_value {
        add("interval_value = ?", Dv::BigInt(v));
    }
    if let Some(v) = &u.interval_unit {
        add("interval_unit = ?", text(v.as_str()));
    }
    if let Some(v) = &u.time_of_day {
        add("time_of_day = ?", text(v.as_str()));
    }
    if let Some(v) = &u.data_interval {
        add("data_interval = ?", text(v.as_str()));
    }
    if let Some(v) = u.lookback_days {
        add("lookback_days = ?", Dv::BigInt(v));
    }
    if let Some(v) = u.is_enabled {
        add("is_enabled = ?", Dv::Boolean(v));
    }
    if let Some(v) = u.is_paused {
        add("is_paused = ?", Dv::Boolean(v));
    }
    if let Some(v) = &u.status {
        add("status = ?", text(v.as_str()));
    }
    if let Some(v) = &u.apscheduler_job_id {
        add("apscheduler_job_id = ?", text(v.as_str()));
    }
    if let Some(v) = u.next_run_at {
        add("next_run_at = CAST(? AS TIMESTAMP)", text(ts_param(v)));
    }
    if let Some(v) = u.last_run_at {
        add("last_run_at = CAST(? AS TIMESTAMP)", text(ts_param(v)));
    }
    if let Some(v) = &u.last_run_status {
        add("last_run_status = ?", text(v.as_str()));
    }
    if sets.is_empty() {
        return Ok(Err("No fields to update".into()));
    }
    p.push(text(id));
    c.execute(
        &format!(
            "UPDATE historify_schedules SET {} WHERE id = ?",
            sets.join(", ")
        ),
        params_from_iter(p),
    )?;
    Ok(Ok(()))
}

pub fn clear_next_run(c: &Connection, id: &str) -> Result<()> {
    c.execute(
        "UPDATE historify_schedules SET next_run_at = NULL WHERE id = ?",
        [id],
    )?;
    Ok(())
}

pub fn delete_schedule(c: &Connection, id: &str) -> Result<()> {
    c.execute(
        "DELETE FROM historify_schedule_executions WHERE schedule_id = ?",
        [id],
    )?;
    c.execute("DELETE FROM historify_schedules WHERE id = ?", [id])?;
    Ok(())
}

/// Web `increment_schedule_run_counts`.
pub fn count_run(c: &Connection, id: &str, success: bool, now: NaiveDateTime) -> Result<()> {
    let col = if success {
        "successful_runs"
    } else {
        "failed_runs"
    };
    c.execute(
        &format!(
            "UPDATE historify_schedules SET total_runs = total_runs + 1, {c} = {c} + 1, \
             last_run_at = CAST(? AS TIMESTAMP) WHERE id = ?",
            c = col
        ),
        params![ts_param(now), id],
    )?;
    Ok(())
}

pub fn create_execution(c: &Connection, schedule_id: &str, now: NaiveDateTime) -> Result<i64> {
    let id = next_id(c, "historify_schedule_executions_id_seq")?;
    c.execute(
        "INSERT INTO historify_schedule_executions (id, schedule_id, status, started_at) \
         VALUES (?, ?, 'running', CAST(? AS TIMESTAMP))",
        params![id, schedule_id, ts_param(now)],
    )?;
    Ok(id)
}

/// Fields of an execution update.
#[derive(Debug, Default, Clone)]
pub struct ExecutionUpdate {
    pub status: Option<String>,
    pub completed_at: Option<NaiveDateTime>,
    pub symbols_processed: Option<i64>,
    pub symbols_success: Option<i64>,
    pub symbols_failed: Option<i64>,
    pub records_downloaded: Option<i64>,
    pub error_message: Option<String>,
    pub download_job_id: Option<String>,
}

pub fn update_execution(c: &Connection, id: i64, u: &ExecutionUpdate) -> Result<()> {
    let mut sets: Vec<&str> = Vec::new();
    let mut p: Vec<Dv> = Vec::new();
    let mut add = |col: &'static str, v: Dv| {
        sets.push(col);
        p.push(v);
    };
    if let Some(v) = &u.status {
        add("status = ?", text(v.as_str()));
    }
    if let Some(v) = u.completed_at {
        add("completed_at = CAST(? AS TIMESTAMP)", text(ts_param(v)));
    }
    if let Some(v) = u.symbols_processed {
        add("symbols_processed = ?", Dv::BigInt(v));
    }
    if let Some(v) = u.symbols_success {
        add("symbols_success = ?", Dv::BigInt(v));
    }
    if let Some(v) = u.symbols_failed {
        add("symbols_failed = ?", Dv::BigInt(v));
    }
    if let Some(v) = &u.download_job_id {
        add("download_job_id = ?", text(v.as_str()));
    }
    if let Some(v) = u.records_downloaded {
        add("records_downloaded = ?", Dv::BigInt(v));
    }
    if let Some(v) = &u.error_message {
        add("error_message = ?", text(v.as_str()));
    }
    if sets.is_empty() {
        return Ok(());
    }
    p.push(Dv::BigInt(id));
    c.execute(
        &format!(
            "UPDATE historify_schedule_executions SET {} WHERE id = ?",
            sets.join(", ")
        ),
        params_from_iter(p),
    )?;
    Ok(())
}

pub fn executions(c: &Connection, schedule_id: &str, limit: i64) -> Result<Vec<Value>> {
    rows_json(
        c,
        &format!(
            "SELECT id, schedule_id, download_job_id, status, {}, {}, symbols_processed, \
             symbols_success, symbols_failed, records_downloaded, error_message \
             FROM historify_schedule_executions WHERE schedule_id = ? \
             ORDER BY started_at DESC, id DESC LIMIT ?",
            iso("started_at"),
            iso("completed_at")
        ),
        vec![text(schedule_id), Dv::BigInt(limit)],
        &[
            ("id", Ty::Int),
            ("schedule_id", Ty::Str),
            ("download_job_id", Ty::Str),
            ("status", Ty::Str),
            ("started_at", Ty::Str),
            ("completed_at", Ty::Str),
            ("symbols_processed", Ty::Int),
            ("symbols_success", Ty::Int),
            ("symbols_failed", Ty::Int),
            ("records_downloaded", Ty::Int),
            ("error_message", Ty::Str),
        ],
    )
}
