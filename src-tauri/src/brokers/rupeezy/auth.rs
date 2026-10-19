//! Vortex SSO sign-in (web `api/auth_api.py`,
//! https://vortex.rupeezy.in/docs/latest/authentication/).
//!
//! 1. The trader is sent to `https://flow.rupeezy.in?applicationId=<id>`.
//! 2. Rupeezy redirects back to `/rupeezy/callback` with an `auth` code.
//! 3. `checksum = SHA256(application_id + auth + api_key)`, no separators.
//! 4. `POST /user/session {token, applicationId, checksum}` with the
//!    `x-api-key` header; the access token is `data.access_token`.

use super::RupeezyBroker;
use crate::brokers::common::http;
use crate::brokers::{AuthResponse, BrokerCredentials};
use crate::error::{AppError, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// `SHA256(application_id + auth + api_key)` as lowercase hex.
pub fn checksum(application_id: &str, auth_code: &str, api_key: &str) -> String {
    let mut h = Sha256::new();
    h.update(application_id.as_bytes());
    h.update(auth_code.as_bytes());
    h.update(api_key.as_bytes());
    hex::encode(h.finalize())
}

fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn missing(what: &str) -> AppError {
    AppError::Validation(format!(
        "Your Rupeezy {} is missing. Add it on the Broker Configuration page.",
        what
    ))
}

pub async fn authenticate(b: &RupeezyBroker, creds: BrokerCredentials) -> Result<AuthResponse> {
    let code = creds
        .auth_code
        .or(creds.request_token)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            AppError::Validation(
                "Rupeezy did not return a login code. Start the Rupeezy login again.".into(),
            )
        })?;
    let application_id = creds.api_key.trim().to_string();
    if application_id.is_empty() {
        return Err(missing("application ID"));
    }
    let api_key = creds
        .api_secret
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| missing("API key"))?;
    let body = json!({
        "token": code,
        "applicationId": application_id,
        "checksum": checksum(&application_id, &code, api_key.trim()),
    });
    // `/user/session` shares the one-a-second limit of the `/user` calls.
    b.pacers.account.acquire().await?;
    let resp = b
        .http
        .post(format!("{}/user/session", b.urls.rest))
        .header("x-api-key", api_key.trim())
        .json(&body)
        .send()
        .await
        .map_err(crate::brokers::common::redact::http)?;
    let (status, v): (_, Value) = http::read_json("rupeezy", resp)
        .await
        .map_err(crate::brokers::common::redact::redact)?;
    let data = v.get("data").cloned().unwrap_or(Value::Null);
    let Some(token) = text(&data, "access_token") else {
        let msg = text(&v, "message").unwrap_or_default().to_string();
        tracing::warn!(status = status.as_u16(), "Rupeezy login refused: {}", msg);
        return Err(AppError::Auth(if msg.is_empty() {
            "Rupeezy did not accept the login. Check your application ID and API key, then log in again."
                .into()
        } else {
            format!(
                "Rupeezy did not accept the login: {}. Log in to Rupeezy again.",
                msg
            )
        }));
    };
    b.remember_api_key(Some(&api_key));
    Ok(AuthResponse {
        auth_token: token.to_string(),
        feed_token: None,
        // The account the sign-in is checked against: Vortex's own answer,
        // never the saved client id (that would pass any account).
        user_id: text(&data, "user_id").unwrap_or_default().to_string(),
        user_name: text(&data, "user_name").map(str::to_string),
    })
}
