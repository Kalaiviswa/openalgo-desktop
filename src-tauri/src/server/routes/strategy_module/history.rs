//! Read-only history of a strategy (web `blueprints/strategy_module.py`
//! read routes): its runs, orders, events, webhook calls, books, positions
//! and checkpoints.

use super::{
    data_or_failed, err, int_arg, ok200, read_failed, resolve, Ctx, EVENTS_DEFAULT_LIMIT,
    EVENTS_MAX_LIMIT, Q,
};
use crate::server::envelope::json_response;
use crate::server::middleware::User;
use crate::state::AppState;
use crate::strategy::store::{EVENT_KINDS, EVENT_SEVERITIES};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Response,
};
use serde_json::Value;
use std::collections::HashMap;

/// GET .../runs
pub async fn runs(State(ctx): Ctx, User(u): User, Path(sid): Path<String>) -> Response {
    let row = match resolve(&ctx, &u.username, &sid) {
        Ok(r) => r,
        Err(r) => return *r,
    };
    let runs = ctx.strategy.store.list_runs(row.id, 100);
    data_or_failed(format_args!("the runs of strategy {}", row.id), runs)
}

/// GET .../orders?run_id=
pub async fn orders(
    State(ctx): Ctx,
    User(u): User,
    Path(sid): Path<String>,
    Query(q): Q,
) -> Response {
    let row = match resolve(&ctx, &u.username, &sid) {
        Ok(r) => r,
        Err(r) => return *r,
    };
    let run_id = match int_arg(&q, "run_id") {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let rows = ctx
        .strategy
        .store
        .list_orders_for_strategy(row.id, run_id)
        .map(|rows| rows.iter().map(|o| o.to_dict()).collect::<Vec<Value>>());
    data_or_failed(format_args!("the orders of strategy {}", row.id), rows)
}

/// GET .../events?run_id=&kind=&severity=&limit=
pub async fn events(
    State(ctx): Ctx,
    User(u): User,
    Path(sid): Path<String>,
    Query(q): Q,
) -> Response {
    let row = match resolve(&ctx, &u.username, &sid) {
        Ok(r) => r,
        Err(r) => return *r,
    };
    let run_id = match int_arg(&q, "run_id") {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let kind = q.get("kind").filter(|s| !s.is_empty());
    if let Some(k) = kind {
        if !EVENT_KINDS.contains(&k.as_str()) {
            return err(
                format!("kind must be one of: {}", EVENT_KINDS.join(", ")),
                StatusCode::BAD_REQUEST,
            );
        }
    }
    let severity = q.get("severity").filter(|s| !s.is_empty());
    if let Some(s) = severity {
        if !EVENT_SEVERITIES.contains(&s.as_str()) {
            return err(
                format!("severity must be one of: {}", EVENT_SEVERITIES.join(", ")),
                StatusCode::BAD_REQUEST,
            );
        }
    }
    let requested = match int_arg(&q, "limit") {
        Ok(v) => v,
        Err(r) => return *r,
    };
    // Clamped: SQLite reads a negative LIMIT as no limit.
    let limit = requested
        .filter(|v| *v != 0)
        .unwrap_or(EVENTS_DEFAULT_LIMIT)
        .clamp(1, EVENTS_MAX_LIMIT);
    let rows = ctx.strategy.store.list_events(
        row.id,
        run_id,
        kind.map(String::as_str),
        severity.map(String::as_str),
        limit,
    );
    data_or_failed(format_args!("the events of strategy {}", row.id), rows)
}

/// GET .../webhook_events
pub async fn webhook_events(State(ctx): Ctx, User(u): User, Path(sid): Path<String>) -> Response {
    let row = match resolve(&ctx, &u.username, &sid) {
        Ok(r) => r,
        Err(r) => return *r,
    };
    let rows = ctx.strategy.store.list_webhook_events(row.id, 200);
    data_or_failed(
        format_args!("the webhook events of strategy {}", row.id),
        rows,
    )
}

async fn book(
    ctx: &AppState,
    user: &str,
    sid: &str,
    q: &HashMap<String, String>,
    which: u8,
) -> Response {
    let row = match resolve(ctx, user, sid) {
        Ok(r) => r,
        Err(r) => return *r,
    };
    let run_id = match int_arg(q, "run_id") {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let payload = match which {
        0 => ctx.strategy.strategy_orderbook(row.id, run_id).await,
        1 => ctx.strategy.strategy_tradebook(row.id, run_id).await,
        _ => ctx.strategy.strategy_positions(row.id, run_id).await,
    };
    let payload = match payload {
        Ok(p) => p,
        Err(e) => return read_failed(format_args!("the books of strategy {}", row.id), e),
    };
    let code = if payload["status"] == "success" {
        StatusCode::OK
    } else {
        StatusCode::BAD_GATEWAY
    };
    json_response(code, payload)
}

/// GET .../orderbook
pub async fn orderbook(
    State(ctx): Ctx,
    User(u): User,
    Path(sid): Path<String>,
    Query(q): Q,
) -> Response {
    book(&ctx, &u.username, &sid, &q, 0).await
}

/// GET .../tradebook
pub async fn tradebook(
    State(ctx): Ctx,
    User(u): User,
    Path(sid): Path<String>,
    Query(q): Q,
) -> Response {
    book(&ctx, &u.username, &sid, &q, 1).await
}

/// GET .../positions
pub async fn positions(
    State(ctx): Ctx,
    User(u): User,
    Path(sid): Path<String>,
    Query(q): Q,
) -> Response {
    book(&ctx, &u.username, &sid, &q, 2).await
}

/// GET .../checkpoints?run_id=: defaults to the current, then the latest run.
pub async fn checkpoints(
    State(ctx): Ctx,
    User(u): User,
    Path(sid): Path<String>,
    Query(q): Q,
) -> Response {
    let row = match resolve(&ctx, &u.username, &sid) {
        Ok(r) => r,
        Err(r) => return *r,
    };
    let requested = match int_arg(&q, "run_id") {
        Ok(v) => v,
        Err(r) => return *r,
    };
    match ctx.strategy.checkpoint_page(&row, requested) {
        Ok(Some(page)) => ok200(page),
        Ok(None) => err("Run not found", StatusCode::NOT_FOUND),
        Err(e) => read_failed(format_args!("the checkpoints of strategy {}", row.id), e),
    }
}
