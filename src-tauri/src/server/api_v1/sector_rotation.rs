//! `/api/v1/sectorrotation` (web `restx_api/sector_rotation.py`, openalgo
//! b7cdc028e). Its own answers, as on the web: a body the schema refuses
//! is one sentence a trader can act on (a stale saved setting is the usual
//! cause), never the field errors; the API key is checked on every source;
//! the broker source needs a broker session.

use super::{authorize, send, Auth};
use crate::server::envelope::{json_response, read_json_object};
use crate::server::middleware::ClientIp;
use crate::services::core::INVALID_API_KEY;
use crate::services::schema::{Field, Schema, Validator};
use crate::services::sector_rotation_service as rotation;
use crate::state::AppState;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};

/// A body the schema refused (web: the field detail goes to the log).
pub const SETTINGS_REFUSED: &str = "These map settings were not accepted. Choose the benchmark and timeframe again, or use Reset to sectors if you added symbols.";

/// The broker source without a broker session.
pub const BROKER_LOGIN_NEEDED: &str = "Broker API needs an active broker login. Log in to your broker, or switch the data source to Historify.";

const fn max_len(max: usize) -> Validator {
    Validator::Length {
        min: None,
        max: Some(max),
        error: None,
    }
}

fn instrument_schema() -> &'static Schema {
    static S: OnceLock<Schema> = OnceLock::new();
    S.get_or_init(|| {
        Schema::new(vec![
            Field::str("symbol")
                .required()
                .validate(Validator::length(1, 64)),
            Field::str("exchange")
                .required()
                .validate(Validator::one_of(rotation::EXCHANGES)),
            Field::str("label")
                .default(|| Value::Null)
                .allow_none()
                .validate(max_len(64)),
        ])
    })
}

/// Web `SectorRotationSchema`.
fn schema() -> &'static Schema {
    static S: OnceLock<Schema> = OnceLock::new();
    S.get_or_init(|| {
        Schema::new(vec![
            Field::str("apikey")
                .required()
                .validate(Validator::length(1, 256)),
            Field::str("benchmark")
                .default(|| json!("NIFTY"))
                .validate(Validator::one_of(rotation::BENCHMARK_SYMBOLS)),
            Field::str("timeframe")
                .default(|| json!("daily"))
                .validate(Validator::one_of(rotation::TIMEFRAMES)),
            Field::str("source")
                .default(|| json!("db"))
                .validate(Validator::one_of(rotation::SOURCES)),
            Field::list("instruments", instrument_schema())
                .default(|| json!([]))
                .validate(max_len(rotation::MAX_INSTRUMENTS)),
        ])
    })
}

fn error(status: StatusCode, message: &str) -> Response {
    json_response(status, json!({"status": "error", "message": message}))
}

/// `POST /api/v1/sectorrotation`.
pub async fn rotation(
    State(ctx): State<Arc<AppState>>,
    ClientIp(ip): ClientIp,
    req: Request,
) -> Response {
    let body = match read_json_object(req, &()).await {
        Ok(b) => b,
        Err(r) => return *r,
    };
    let loaded = match schema().load(&body) {
        Ok(v) => v,
        Err(e) => {
            tracing::info!("Sector rotation request refused: {}", e.to_json());
            return error(StatusCode::BAD_REQUEST, SETTINGS_REFUSED);
        }
    };
    let key = loaded
        .get("apikey")
        .and_then(Value::as_str)
        .unwrap_or_default();
    // Checked on every source: Historify needs no broker session, but the
    // local history is still the trader's data.
    if !authorize(&ctx, key, ip, Auth::KeyOnly).await {
        return error(StatusCode::FORBIDDEN, INVALID_API_KEY);
    }
    if loaded.get("source").and_then(Value::as_str) == Some("api") && !ctx.is_broker_connected() {
        return error(StatusCode::FORBIDDEN, BROKER_LOGIN_NEEDED);
    }
    send(rotation::sector_rotation(&ctx, &loaded).await)
}

/// `GET /api/v1/sectorrotation/options`: the benchmarks, timeframes and
/// sectors the map supports (no key, as on the web).
pub async fn options() -> Response {
    send(rotation::options())
}
