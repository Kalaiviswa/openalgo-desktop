//! Rupeezy (Vortex API) end to end against a local fake Vortex on an
//! ephemeral port (web `broker/rupeezy`, openalgo 9aa6f2e30): sign-in,
//! headers, order bodies and paths, books, funds, quotes, history, GTT, the
//! master download, read retries and placement outcomes (`rest`), the price
//! feed through the shared feed manager (`feed`), and credentials that
//! never reach a log (`secrets`).

mod feed;
mod rest;
mod secrets;

use axum::body::Bytes;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Router;
use openalgo_desktop_lib::brokers::common::symbols::SymbolResolver;
use openalgo_desktop_lib::brokers::rupeezy::master_contract::parse_master;
use openalgo_desktop_lib::brokers::rupeezy::{checksum, RupeezyBroker, RupeezyUrls};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::sync::Arc;

pub const CSV: &str = include_str!("../../fixtures/brokers/rupeezy/master.csv");
const RESPONSES: &str = include_str!("../../fixtures/brokers/rupeezy/responses.json");

/// The fake app's application id and Vortex API key.
pub const APP: &str = "APP1";
pub const KEY: &str = "KEY1";

pub fn fixture(key: &str) -> Value {
    let all: Value = serde_json::from_str(RESPONSES).unwrap();
    all[key].clone()
}

#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    /// As sent, percent-encoding kept.
    pub path: String,
    pub query: Vec<(String, String)>,
    pub authorization: String,
    pub api_key: String,
    pub body: Value,
}

impl Seen {
    pub fn q(&self, key: &str) -> Vec<String> {
        self.query
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .collect()
    }
}

/// An answer given instead of the default route, `times` times.
#[derive(Clone)]
pub struct Script {
    pub method: &'static str,
    pub path: String,
    pub status: u16,
    pub retry_after: Option<String>,
    pub body: String,
    pub times: usize,
}

impl Script {
    pub fn new(method: &'static str, path: &str, status: u16, body: &str) -> Self {
        Self {
            method,
            path: path.into(),
            status,
            retry_after: None,
            body: body.into(),
            times: usize::MAX,
        }
    }

    pub fn once(mut self) -> Self {
        self.times = 1;
        self
    }

    pub fn retry_after(mut self, v: &str) -> Self {
        self.retry_after = Some(v.into());
        self
    }
}

#[derive(Default)]
pub struct Fake {
    pub seen: Mutex<Vec<Seen>>,
    pub scripts: Mutex<Vec<Script>>,
    /// Orders for these tickers are refused.
    pub refuse_tickers: Mutex<Vec<String>>,
}

impl Fake {
    pub fn calls(&self, method: &str, path: &str) -> Vec<Seen> {
        self.seen
            .lock()
            .iter()
            .filter(|s| s.method == method && s.path == path)
            .cloned()
            .collect()
    }

    pub fn calls_under(&self, prefix: &str) -> Vec<Seen> {
        self.seen
            .lock()
            .iter()
            .filter(|s| s.path.starts_with(prefix))
            .cloned()
            .collect()
    }

    pub fn script(&self, s: Script) {
        self.scripts.lock().push(s);
    }

    fn scripted(&self, method: &str, path: &str) -> Option<Script> {
        let mut scripts = self.scripts.lock();
        let i = scripts
            .iter()
            .position(|s| s.method == method && s.path == path && s.times > 0)?;
        scripts[i].times -= 1;
        Some(scripts[i].clone())
    }
}

pub fn json_resp(status: u16, v: Value) -> Response {
    (
        StatusCode::from_u16(status).unwrap(),
        [("content-type", "application/json")],
        v.to_string(),
    )
        .into_response()
}

fn ok(v: Value) -> Response {
    json_resp(200, v)
}

/// The quotes of `fixture` for the tickers asked, as Vortex leaves out the
/// ones it has no quote for.
fn quotes(fixture_key: &str, asked: &[String]) -> Response {
    let all = fixture(fixture_key);
    let data: serde_json::Map<String, Value> = all["data"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(k, _)| asked.contains(k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    ok(json!({"status": "success", "data": data}))
}

fn route(fake: &Fake, method: &str, path: &str, seen: &Seen, headers: &HeaderMap) -> Response {
    let body = &seen.body;
    match (method, path) {
        ("POST", "/user/session") => {
            let token = body["token"].as_str().unwrap_or("");
            let key = headers
                .get("x-api-key")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if key == KEY
                && body["applicationId"] == APP
                && body["checksum"] == checksum(APP, token, KEY).as_str()
            {
                ok(fixture("session_ok"))
            } else {
                json_resp(400, fixture("session_refused"))
            }
        }
        ("POST", "/trading/orders/regular") => {
            let ticker = body["ticker"].as_str().unwrap_or("").to_string();
            if fake.refuse_tickers.lock().contains(&ticker) {
                json_resp(400, fixture("place_refused"))
            } else {
                ok(fixture("place_ok"))
            }
        }
        ("PUT", p) if p.starts_with("/trading/orders/regular/") => ok(fixture("modify_ok")),
        ("DELETE", p) if p.starts_with("/trading/orders/regular/") => ok(fixture("cancel_ok")),
        ("GET", "/trading/orders") => ok(fixture("orders")),
        ("GET", "/trading/orders/gtt") => ok(fixture("gtt_book")),
        ("POST", "/trading/orders/gtt") => ok(fixture("gtt_place_ok")),
        ("PUT", p) if p.starts_with("/trading/orders/gtt/") => ok(fixture("gtt_modify_ok")),
        ("DELETE", p) if p.starts_with("/trading/orders/gtt/") => ok(fixture("gtt_cancel_ok")),
        ("GET", p) if p.starts_with("/trading/orders/") => ok(fixture("order_history_live")),
        ("GET", "/trading/trades") => ok(fixture("trades")),
        ("GET", "/trading/portfolio/positions") => ok(fixture("positions")),
        ("GET", "/trading/portfolio/holdings") => ok(fixture("holdings")),
        ("GET", "/user/funds") => ok(fixture("funds_combined")),
        ("GET", "/data/quotes") => {
            let mode = seen.q("mode").first().cloned().unwrap_or_default();
            let key = if mode == "ltp" {
                "quotes_ltp"
            } else {
                "quote_full"
            };
            quotes(key, &seen.q("q"))
        }
        ("GET", "/data/history") => match seen.q("resolution").first().map(String::as_str) {
            Some("1D") => ok(fixture("history_day")),
            Some("1W") => json_resp(400, fixture("history_error")),
            Some("1M") => ok(fixture("history_no_data")),
            _ => ok(fixture("history_minute")),
        },
        ("GET", "/master.csv") => (StatusCode::OK, CSV).into_response(),
        _ => (StatusCode::NOT_FOUND, "no such endpoint").into_response(),
    }
}

fn scripted_resp(s: Script) -> Response {
    let mut r = (
        StatusCode::from_u16(s.status).unwrap(),
        [("content-type", "application/json")],
        s.body,
    )
        .into_response();
    if let Some(v) = s.retry_after {
        r.headers_mut().insert("retry-after", v.parse().unwrap());
    }
    r
}

pub async fn serve(fake: Arc<Fake>) -> String {
    let app = Router::new().fallback(
        move |method: Method, uri: Uri, headers: HeaderMap, body: Bytes| {
            let fake = fake.clone();
            async move {
                let h = |k: &str| {
                    headers
                        .get(k)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_string()
                };
                let seen = Seen {
                    method: method.to_string(),
                    path: uri.path().to_string(),
                    query: url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
                        .into_owned()
                        .collect(),
                    authorization: h("authorization"),
                    api_key: h("x-api-key"),
                    body: serde_json::from_slice(&body).unwrap_or(Value::Null),
                };
                fake.seen.lock().push(seen.clone());
                match fake.scripted(method.as_str(), uri.path()) {
                    Some(s) => scripted_resp(s),
                    None => route(&fake, method.as_str(), uri.path(), &seen, &headers),
                }
            }
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{}", addr)
}

pub fn master() -> SymbolResolver {
    let r = SymbolResolver::new();
    r.load(parse_master(CSV).unwrap());
    r
}

/// A broker on a fresh fake, its `x-api-key` known as after a sign-in.
pub async fn setup() -> (RupeezyBroker, Arc<Fake>) {
    use openalgo_desktop_lib::brokers::{Broker, BrokerCredentials};
    let fake = Arc::new(Fake::default());
    let host = serve(fake.clone()).await;
    let b = RupeezyBroker::with_base_url(master(), RupeezyUrls::rebased(&host, "ws://127.0.0.1:1"));
    b.restore_session(&BrokerCredentials {
        api_key: APP.into(),
        api_secret: Some(KEY.into()),
        ..Default::default()
    });
    (b, fake)
}
