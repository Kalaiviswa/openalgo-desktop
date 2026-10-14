//! In-app updates for the signed-in trader (desktop only: the web is
//! updated with `git pull`). The decisions live in
//! `services::update_service`; the restart itself is the shell's
//! `restart_to_update` command, because only the desktop window can do it.
//!
//! | Route | Answer |
//! | --- | --- |
//! | `GET /api/update/status` | the update state, with the offered version's release notes |
//! | `POST /api/update/check` | checks now, answers with the state |
//! | `POST /api/update/install` | `status: confirm` with the warning when something runs that `confirmed` does not cover; otherwise starts the download |
//! | `POST /api/update/cancel` | forgets a downloaded update (Later) |
//!
//! Every route needs the signed-in user; the POSTs carry the CSRF token
//! like every other state change.

use crate::server::envelope::error;
use crate::server::routes::webui::{no_store, ok, JsonBody};
use crate::services::update_service::{Activity, Decision};
use crate::state::AppState;
use axum::{extract::State, http::StatusCode, response::Response};
use serde_json::json;
use std::sync::Arc;

type Ctx = State<Arc<AppState>>;

/// GET /api/update/status
pub async fn status(State(ctx): Ctx) -> Response {
    no_store(ok(
        json!({"status": "success", "data": ctx.updates.status()}),
    ))
}

/// POST /api/update/check
pub async fn check(State(ctx): Ctx) -> Response {
    let st = ctx.updates.check(&ctx).await;
    no_store(ok(json!({"status": "success", "data": st})))
}

/// POST /api/update/install, body `{"confirmed": <activity>}` once the
/// trader has seen the warning.
pub async fn install(State(ctx): Ctx, body: JsonBody) -> Response {
    let confirmed = body
        .0
        .get("confirmed")
        .and_then(|v| serde_json::from_value::<Activity>(v.clone()).ok());
    match ctx.updates.request_install(&ctx, confirmed).await {
        Decision::Proceed(st) => ok(json!({"status": "success", "data": st})),
        Decision::Confirm { activity, warning } => ok(json!({
            "status": "confirm",
            "activity": activity,
            "warning": warning,
        })),
        Decision::Refused(message) => error(StatusCode::CONFLICT, message),
    }
}

/// POST /api/update/cancel
pub async fn cancel(State(ctx): Ctx) -> Response {
    let st = ctx.updates.cancel(&ctx).await;
    ok(json!({"status": "success", "data": st}))
}
