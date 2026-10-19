//! The one request path for authenticated Vortex REST calls (web
//! `api/client.py`, `api/rate_limiter.py`).
//!
//! Every call carries `Authorization: Bearer <token>` and the app's
//! `x-api-key`, and waits for its category's pacer. A read (GET) gets a
//! short first attempt and one retry on a fresh connection when the pooled
//! one was dead, and up to two retries of a 429 (`Retry-After` in seconds
//! or as an HTTP date, refused past the queue bound). A write is sent once:
//! Vortex can take an order and still answer with an error, and a resent
//! order could be placed twice.

use super::RupeezyBroker;
use crate::brokers::common::http;
use crate::brokers::common::ratelimit::MAX_QUEUE_WAIT;
use crate::brokers::common::redact;
use crate::brokers::types::AuthToken;
use crate::error::{AppError, Result};
use chrono::{DateTime, Utc};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
use std::time::Duration;

const MAX_429_RETRIES: u32 = 2;
/// A pooled connection Vortex dropped while idle hangs until the timeout;
/// a read's first attempt gives up sooner and retries once.
const READ_FIRST_TIMEOUT: Duration = Duration::from_secs(10);

/// Vortex's rate-limit classes (the Rate Limit column of each docs page).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Category {
    /// `/data/quotes` (up to 1000 tickers a call), 1/s.
    Quotes,
    /// `/data/history`, 1/s.
    History,
    /// `/user/*`, 1/s.
    Account,
    /// Regular and GTT orders, books: 10/s.
    Trading,
}

pub(crate) fn category_for(path: &str) -> Category {
    if path.starts_with("/data/quotes") {
        Category::Quotes
    } else if path.starts_with("/data/history") {
        Category::History
    } else if path.starts_with("/user/") {
        Category::Account
    } else {
        Category::Trading
    }
}

/// The endpoint of one order or GTT. Vortex order ids can carry `?` and
/// `:` (`OFPIM00001?:`), so the id is percent-encoded: pasted in, the part
/// after `?` would go out as a query and the call would miss the order.
pub fn order_path(base: &str, order_id: &str) -> String {
    format!("{}/{}", base, urlencoding::encode(order_id))
}

/// One request: method, path under the REST host, query pairs, JSON body.
pub(crate) struct Call<'a> {
    pub method: Method,
    pub path: &'a str,
    pub query: &'a [(&'a str, String)],
    pub body: Option<&'a Value>,
}

impl<'a> Call<'a> {
    pub fn get(path: &'a str) -> Self {
        Self::get_with(path, &[])
    }

    pub fn get_with(path: &'a str, query: &'a [(&'a str, String)]) -> Self {
        Self {
            method: Method::GET,
            path,
            query,
            body: None,
        }
    }

    pub fn write(method: Method, path: &'a str, body: Option<&'a Value>) -> Self {
        Self {
            method,
            path,
            query: &[],
            body,
        }
    }
}

/// What Vortex answered: the HTTP status and the JSON body (an empty
/// success answers `{"status": "success"}`, as the web reads a 204).
#[derive(Debug, Clone)]
pub(crate) struct Answer {
    pub status: StatusCode,
    pub body: Value,
}

impl Answer {
    /// A success status with Vortex's own success marker.
    pub fn succeeded(&self) -> bool {
        self.status.is_success() && is_success(&self.body)
    }

    /// The body says the request failed (`status: "error"`).
    pub fn failed(&self) -> bool {
        self.body
            .get("status")
            .and_then(Value::as_str)
            .is_some_and(|s| s.eq_ignore_ascii_case("error"))
    }

    pub fn message(&self) -> String {
        message_of(&self.body)
    }

    /// The refusal a read reports (`what`: what was asked, for the log).
    pub fn refused(&self, what: &str) -> AppError {
        let msg = self.message();
        tracing::warn!(
            status = self.status.as_u16(),
            "Rupeezy refused {}: {}",
            what,
            msg
        );
        AppError::Broker(if msg.is_empty() {
            "Rupeezy did not answer this request. Try again shortly.".into()
        } else {
            msg
        })
    }
}

/// `status` is `success` (Vortex capitalises it on some books).
pub(crate) fn is_success(v: &Value) -> bool {
    v.get("status")
        .and_then(Value::as_str)
        .is_some_and(|s| s.eq_ignore_ascii_case("success"))
}

/// Vortex's error text: `message`, else the history API's `errmsg`.
pub(crate) fn message_of(v: &Value) -> String {
    ["message", "errmsg"]
        .iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str))
        .unwrap_or_default()
        .trim()
        .to_string()
}

pub(crate) fn session_expired() -> AppError {
    AppError::Auth("Your Rupeezy session has expired. Log in to Rupeezy again.".into())
}

/// How long a 429 asks to wait: `Retry-After` in seconds or as an HTTP
/// date, else 1 s, 2 s (web `_retry_after_seconds`).
pub(crate) fn retry_after(raw: Option<&str>, attempt: u32, now: DateTime<Utc>) -> Duration {
    if let Some(r) = raw.map(str::trim).filter(|s| !s.is_empty()) {
        if let Ok(secs) = r.parse::<f64>() {
            return Duration::try_from_secs_f64(secs.max(0.0)).unwrap_or(Duration::MAX);
        }
        if let Ok(when) = DateTime::parse_from_rfc2822(r) {
            return (when.with_timezone(&Utc) - now)
                .to_std()
                .unwrap_or(Duration::ZERO);
        }
    }
    Duration::from_secs(1u64 << attempt.min(16))
}

/// A read failure worth one retry on a fresh connection.
fn dead_connection(e: &AppError) -> bool {
    matches!(e, AppError::Http(h) if h.is_timeout() || h.is_connect() || h.is_request())
}

impl RupeezyBroker {
    async fn pace(&self, category: Category) -> Result<()> {
        match category {
            Category::Quotes => self.pacers.quotes.acquire().await,
            Category::History => self.pacers.history.acquire().await,
            Category::Account => self.pacers.account.acquire().await,
            Category::Trading => self.pacers.trading.acquire().await,
        }
    }

    /// One attempt, after its pacing slot.
    async fn send(
        &self,
        auth: &AuthToken,
        call: &Call<'_>,
        timeout: Duration,
    ) -> Result<reqwest::Response> {
        let token = auth.raw().trim();
        if token.is_empty() {
            return Err(session_expired());
        }
        self.pace(category_for(call.path)).await?;
        let mut req = self
            .http
            .request(
                call.method.clone(),
                format!("{}{}", self.urls.rest, call.path),
            )
            .bearer_auth(token)
            .timeout(timeout);
        if let Some(key) = self.api_key() {
            req = req.header("x-api-key", key.expose());
        }
        if !call.query.is_empty() {
            req = req.query(call.query);
        }
        if let Some(body) = call.body {
            req = req.json(body);
        }
        req.send().await.map_err(redact::http)
    }

    /// A read, with the dead-connection retry and the bounded 429 retries.
    async fn read(&self, auth: &AuthToken, call: &Call<'_>) -> Result<reqwest::Response> {
        let mut attempt = 0;
        loop {
            let resp = match self.send(auth, call, READ_FIRST_TIMEOUT).await {
                Ok(r) => r,
                Err(e) if dead_connection(&e) => {
                    tracing::warn!(
                        "Rupeezy read of {} failed ({}); retrying once",
                        call.path,
                        e.code()
                    );
                    self.send(auth, call, http::REQUEST_TIMEOUT).await?
                }
                Err(e) => return Err(e),
            };
            if resp.status() != StatusCode::TOO_MANY_REQUESTS || attempt == MAX_429_RETRIES {
                return Ok(resp);
            }
            let header = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            let wait = retry_after(header.as_deref(), attempt, Utc::now());
            if wait > MAX_QUEUE_WAIT {
                return Ok(resp);
            }
            tracing::warn!("Rupeezy limited {}; retrying in {:?}", call.path, wait);
            tokio::time::sleep(wait).await;
            attempt += 1;
        }
    }

    /// Send `call` and decode the answer. A refused session is an error;
    /// every other status comes back with its body for the caller to judge.
    pub(crate) async fn call(&self, auth: &AuthToken, call: Call<'_>) -> Result<Answer> {
        let resp = if call.method == Method::GET {
            self.read(auth, &call).await?
        } else {
            self.send(auth, &call, http::REQUEST_TIMEOUT).await?
        };
        let status = resp.status();
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            tracing::warn!(status = status.as_u16(), "Rupeezy refused the session");
            return Err(session_expired());
        }
        if status.is_success() && resp.content_length() == Some(0) {
            return Ok(Answer {
                status,
                body: json!({"status": "success"}),
            });
        }
        let (status, body) = http::read_json::<Value>("rupeezy", resp)
            .await
            .map_err(redact::redact)?;
        Ok(Answer { status, body })
    }

    /// [`Self::call`] for a read that must succeed: the body on success,
    /// Vortex's message otherwise.
    pub(crate) async fn ok(&self, auth: &AuthToken, call: Call<'_>) -> Result<Value> {
        let what = call.path.to_string();
        let answer = self.call(auth, call).await?;
        if answer.succeeded() {
            Ok(answer.body)
        } else {
            Err(answer.refused(&what))
        }
    }
}
