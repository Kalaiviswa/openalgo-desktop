//! Server warnings and errors and browser error reports (the Diagnostics error log).

use super::{cap_text, ts};
use crate::error::Result;
use rusqlite::{params, Connection};
use serde_json::{json, Value};

#[derive(Debug, Clone, Default)]
pub struct ErrorEntry {
    pub ts: String,
    pub level: String,
    pub logger: Option<String>,
    pub module: Option<String>,
    pub file: Option<String>,
    pub message: String,
    pub exception: Option<Value>,
    pub request: Option<Value>,
}

impl ErrorEntry {
    /// The web's sanitized errors.jsonl entry: only allowed keys, capped.
    pub fn to_json(&self) -> Value {
        let mut m = serde_json::Map::new();
        m.insert("ts".into(), json!(self.ts));
        m.insert("level".into(), json!(self.level));
        if let Some(v) = &self.logger {
            m.insert("logger".into(), json!(v));
        }
        if let Some(v) = &self.module {
            m.insert("module".into(), json!(v));
        }
        if let Some(v) = &self.file {
            m.insert("file".into(), json!(v));
        }
        m.insert("message".into(), json!(cap_text(&self.message, 20_000)));
        if let Some(v) = &self.exception {
            m.insert("exception".into(), v.clone());
        }
        if let Some(v) = &self.request {
            m.insert("request".into(), v.clone());
        }
        Value::Object(m)
    }
}

pub fn insert_error(conn: &Connection, e: &ErrorEntry) -> Result<()> {
    conn.execute(
        "INSERT INTO error_logs (ts, level, logger, module, file, message, exception, request)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            e.ts,
            cap_text(&e.level, 16),
            e.logger.as_deref().map(|s| cap_text(s, 200)),
            e.module.as_deref().map(|s| cap_text(s, 200)),
            e.file.as_deref().map(|s| cap_text(s, 300)),
            cap_text(&e.message, 40_000),
            e.exception
                .as_ref()
                .map(|v| cap_text(&v.to_string(), 20_000)),
            e.request.as_ref().map(|v| cap_text(&v.to_string(), 2_000)),
        ],
    )?;
    Ok(())
}

/// All error entries, oldest first (bounded by the table cap).
pub fn all_errors(conn: &Connection) -> Result<Vec<ErrorEntry>> {
    let mut st = conn.prepare(
        "SELECT ts, level, logger, module, file, message, exception, request FROM error_logs ORDER BY id",
    )?;
    let rows = st
        .query_map([], |r| {
            let parse = |s: Option<String>| s.and_then(|s| serde_json::from_str::<Value>(&s).ok());
            Ok(ErrorEntry {
                ts: r.get(0)?,
                level: r.get(1)?,
                logger: r.get(2)?,
                module: r.get(3)?,
                file: r.get(4)?,
                message: r.get(5)?,
                exception: parse(r.get(6)?),
                request: parse(r.get(7)?),
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}
