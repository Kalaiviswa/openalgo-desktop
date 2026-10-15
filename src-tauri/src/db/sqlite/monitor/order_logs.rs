//! The API order log page reads (`logs.db` `order_logs`).

use crate::error::Result;
use rusqlite::{params, Connection};

/// One `order_logs` row: (id, api_type, request_data, response_data, created_at).
pub type OrderLogRow = (i64, String, String, String, String);

/// `order_logs` (live API calls, request bodies already without the API key)
/// created in `[from, to)` (ISO UTC text), optionally matching `search`.
/// `page` is `(offset, limit)`; None returns every match.
pub fn order_logs_page(
    conn: &Connection,
    from: &str,
    to: &str,
    search: Option<&str>,
    page: Option<(i64, i64)>,
) -> Result<(Vec<OrderLogRow>, i64)> {
    let like = search.map(|s| {
        let esc = s
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        format!("%{}%", esc)
    });
    let filter = "created_at >= ?1 AND created_at < ?2 AND (?3 IS NULL
        OR api_type LIKE ?3 ESCAPE '\\' OR request_data LIKE ?3 ESCAPE '\\'
        OR response_data LIKE ?3 ESCAPE '\\')";
    let total: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM order_logs WHERE {filter}"),
        params![from, to, like],
        |r| r.get(0),
    )?;
    let (offset, limit) = page.unwrap_or((0, -1));
    let mut st = conn.prepare(&format!(
        "SELECT id, api_type, request_data, response_data, created_at FROM order_logs
         WHERE {filter} ORDER BY created_at DESC, id DESC LIMIT ?4 OFFSET ?5"
    ))?;
    let rows = st
        .query_map(params![from, to, like, limit, offset], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok((rows, total))
}
