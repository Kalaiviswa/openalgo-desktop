//! Sign-in attempts (web `auth_db.LoginAttempt`).

use super::{cap_text, ts};
use crate::error::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

#[derive(Debug, Clone, Default)]
pub struct LoginAttempt {
    pub username: String,
    pub ip_address: Option<String>,
    pub device_info: Option<String>,
    pub status: String,
    pub login_type: Option<String>,
    pub broker: Option<String>,
    pub failure_reason: Option<String>,
}

pub fn insert_login_attempt(conn: &Connection, a: &LoginAttempt, now: DateTime<Utc>) -> Result<()> {
    conn.execute(
        "INSERT INTO login_attempts (username, ip_address, device_info, status, login_type, broker, failure_reason, timestamp)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            cap_text(&a.username, 255),
            a.ip_address,
            a.device_info.as_deref().map(|d| cap_text(d, 500)),
            cap_text(&a.status, 20),
            a.login_type,
            a.broker,
            a.failure_reason.as_deref().map(|d| cap_text(d, 255)),
            ts(now)
        ],
    )?;
    Ok(())
}

pub fn login_attempts(
    conn: &Connection,
    limit: i64,
    status: Option<&str>,
) -> Result<Vec<(LoginAttempt, String)>> {
    let mut st = conn.prepare(
        "SELECT username, ip_address, device_info, status, login_type, broker, failure_reason, timestamp
         FROM login_attempts WHERE (?1 IS NULL OR status = ?1) ORDER BY id DESC LIMIT ?2",
    )?;
    let rows = st
        .query_map(params![status, limit], |r| {
            Ok((
                LoginAttempt {
                    username: r.get(0)?,
                    ip_address: r.get(1)?,
                    device_info: r.get(2)?,
                    status: r.get(3)?,
                    login_type: r.get(4)?,
                    broker: r.get(5)?,
                    failure_reason: r.get(6)?,
                },
                r.get::<_, String>(7)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn clear_login_attempts(conn: &Connection) -> Result<usize> {
    Ok(conn.execute("DELETE FROM login_attempts", [])?)
}
