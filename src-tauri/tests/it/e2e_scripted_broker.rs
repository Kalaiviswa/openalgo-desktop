//! The scripted broker of CI's native E2E test build
//! (`brokers::scripted`), through the routes the pages call, in the order
//! `scripts/ci/native_e2e.py` drives them through the app's window: save
//! the Dhan (Sandbox) keys, log in with them, read the API key, switch
//! to analyzer mode, open the Option Chain (underlyings, expiries, chain),
//! place the ATM call from its order form, then read the order book.
//!
//! This keeps the E2E run's market working on every pull request (the run
//! itself builds a test binary, so it runs after pushes to master): the
//! order fills in the sandbox, the order book has the web's analyze-mode
//! shape (`tests/fixtures/web/rest/orderbook/populated.json`), and no order
//! call reaches the broker.

use crate::api_v1_support::fixture;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{header, Method, Request, StatusCode};
use chrono::{NaiveDate, TimeZone};
use chrono_tz::Asia::Kolkata;
use http_body_util::BodyExt;
use openalgo_desktop_lib::brokers::mock::{MockBroker, MockCall};
use openalgo_desktop_lib::brokers::scripted::{self, LOT, SLOT};
use openalgo_desktop_lib::brokers::BrokerRegistry;
use openalgo_desktop_lib::clock::ManualClock;
use openalgo_desktop_lib::security::keystore::MemoryKeyStore;
use openalgo_desktop_lib::services::auth_service::AuthService;
use openalgo_desktop_lib::state::{AppState, OpenOptions};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

const USER: &str = "trader";

struct App {
    ctx: Arc<AppState>,
    scripted: Arc<MockBroker>,
    cookie: String,
    csrf: String,
    _dir: tempfile::TempDir,
}

impl App {
    /// The test build's start: every adapter, the scripted one in its slot,
    /// on Monday 19 October 2026, 10:00 IST; an account and a signed-in page.
    fn start(today: NaiveDate) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (brokers, scripted) = scripted::registry(&BrokerRegistry::new(), today);
        let at = Kolkata
            .from_local_datetime(&today.and_hms_opt(10, 0, 0).unwrap())
            .single()
            .unwrap();
        let ctx = AppState::open(
            dir.path(),
            OpenOptions {
                keystore: Arc::new(MemoryKeyStore::new()),
                clock: ManualClock::new(at.with_timezone(&chrono::Utc)),
                brokers: Arc::new(brokers),
            },
        )
        .unwrap();
        AuthService::setup(&ctx, USER, "trader@example.com", "Secret@123").unwrap();
        let s = ctx.sessions.create(ctx.now());
        ctx.sessions.update(&s.id, |x| x.user = Some(USER.into()));
        App {
            ctx,
            scripted,
            cookie: format!("session={}", s.id),
            csrf: s.csrf_token,
            _dir: dir,
        }
    }

    async fn send(&self, method: Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut b = Request::builder()
            .method(method)
            .uri(path)
            .header(header::ACCEPT, "application/json")
            .header(header::COOKIE, &self.cookie)
            .header("x-csrftoken", &self.csrf);
        let body = match body {
            Some(v) => {
                b = b.header(header::CONTENT_TYPE, "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let mut req = b.body(body).unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40000))));
        crate::with_host(&mut req, &self.ctx);
        let resp = openalgo_desktop_lib::server::app(self.ctx.clone())
            .oneshot(req)
            .await
            .unwrap();
        let s = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (s, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn get(&self, path: &str) -> Value {
        let (s, v) = self.send(Method::GET, path, None).await;
        assert_eq!(s, StatusCode::OK, "{}: {}", path, v);
        v
    }

    async fn post(&self, path: &str, body: Value) -> Value {
        let (s, v) = self.send(Method::POST, path, Some(body)).await;
        assert_eq!(s, StatusCode::OK, "{}: {}", path, v);
        v
    }

    /// Broker calls of the kinds `f` picks.
    fn calls(&self, f: impl Fn(&MockCall) -> bool) -> usize {
        self.scripted.calls().iter().filter(|c| f(c)).count()
    }
}

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object().unwrap().keys().cloned().collect()
}

#[tokio::test]
async fn a_sandbox_order_from_the_option_chain_reaches_the_order_book_and_never_the_broker() {
    let today = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
    // The counts the test build reads out; no other test here drives a
    // scripted broker.
    let before = scripted::counts();
    let app = App::start(today);

    // Broker Configuration: the slot's keys; Select Broker: log in with them.
    let (s, v) = app
        .send(
            Method::PUT,
            &format!("/api/broker/credentials/{}", SLOT),
            Some(json!({"api_key": "1000000001", "api_secret": "scripted"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{}", v);
    let v = app.post(&format!("/{}/callback", SLOT), json!({})).await;
    assert_eq!(v["status"], "success", "{}", v);
    assert_eq!(app.ctx.get_broker_session().unwrap().broker_id, SLOT);
    // The master contract loads in the background after the sign-in.
    let mut ready = false;
    for _ in 0..200 {
        if app.get("/api/master-contract/ready").await["ready"] == true {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(ready, "the scripted master contract did not load");

    // API Key: the key made with the account, as the page reads it; the
    // navigation bar: switch to analyzer mode.
    let key = app.get("/apikey").await["api_key"]
        .as_str()
        .unwrap()
        .to_string();
    let v = app.post("/auth/analyzer-toggle", json!({})).await;
    assert_eq!(v["data"]["analyze_mode"], true, "{}", v);

    // Option Chain: the underlying, its expiry, the chain around the money.
    let v = app.get("/search/api/underlyings?exchange=NFO").await;
    assert!(
        v["underlyings"]
            .as_array()
            .unwrap()
            .contains(&json!("NIFTY")),
        "{}",
        v
    );
    let expiry = scripted::expiry_after(today);
    let v = app
        .get("/search/api/expiries?exchange=NFO&underlying=NIFTY&instrumenttype=options")
        .await;
    assert_eq!(v["expiries"], json!(["27-OCT-26"]), "{}", v);
    let v = app
        .post(
            "/api/v1/optionchain",
            json!({"apikey": key, "underlying": "NIFTY", "exchange": "NFO",
                   "expiry_date": "27OCT26", "strike_count": 5}),
        )
        .await;
    assert_eq!(v["atm_strike"], 25_000.0, "{}", v);
    let atm = scripted::option_symbol(expiry, 25_000, "CE");
    let row = v["chain"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["strike"] == 25_000.0)
        .unwrap();
    assert_eq!(row["ce"]["symbol"], atm);
    assert_eq!(row["ce"]["lotsize"], LOT);

    // The order form's BUY of one lot, as it posts it.
    let placed = app
        .post(
            "/api/v1/placeorder",
            json!({"apikey": key, "strategy": "OptionChain", "symbol": atm, "exchange": "NFO",
                   "action": "BUY", "quantity": LOT, "pricetype": "MARKET", "product": "NRML"}),
        )
        .await;
    assert_eq!(
        keys(&placed),
        keys(&json!({"status": 0, "orderid": 0, "mode": 0}))
    );
    assert_eq!(placed["mode"], "analyze");
    let orderid = placed["orderid"].as_str().unwrap().to_string();
    assert!(
        orderid.len() == 14 && orderid.starts_with("261019"),
        "a sandbox order id: {}",
        orderid
    );

    // The order book: the web's analyze-mode shape, the order filled.
    let book = app.post("/api/v1/orderbook", json!({"apikey": key})).await;
    let web = &fixture("orderbook/populated.json")["response"]["body"];
    assert_eq!(keys(&book), keys(web));
    assert_eq!(keys(&book["data"]), keys(&web["data"]));
    assert_eq!(
        keys(&book["data"]["statistics"]),
        keys(&web["data"]["statistics"])
    );
    let orders = book["data"]["orders"].as_array().unwrap();
    assert_eq!(orders.len(), 1, "{}", book);
    let o = &orders[0];
    assert_eq!(keys(o), keys(&web["data"]["orders"][0]));
    assert_eq!(o["orderid"], orderid);
    assert_eq!(o["symbol"], atm);
    assert_eq!(o["strategy"], "OptionChain");
    assert_eq!(o["order_status"], "complete", "{}", o);
    assert_eq!(o["quantity"], LOT);
    // Filled at the scripted ATM call's price (last 130.00, ask 130.05).
    let avg = o["average_price"].as_f64().unwrap();
    assert!((130.0..=130.06).contains(&avg), "{}", o);
    assert_eq!(book["mode"], "analyze");

    // Quotes and the master came from the scripted broker; no order went to it.
    assert_eq!(app.calls(|c| *c == MockCall::Authenticate), 1);
    assert_eq!(app.calls(|c| *c == MockCall::MasterContract), 1);
    assert!(app.calls(|c| matches!(c, MockCall::Quote(_) | MockCall::MultiQuotes(_))) > 0);
    let orders_at_broker = app.calls(|c| {
        matches!(
            c,
            MockCall::PlaceOrder(_)
                | MockCall::ModifyOrder(_)
                | MockCall::CancelOrder(_)
                | MockCall::CancelAll
                | MockCall::CloseAll
                | MockCall::PlaceGtt
        )
    });
    assert_eq!(orders_at_broker, 0, "{:?}", app.scripted.calls());
    // The broker's own count, which the test build reads out, agrees: the
    // sign-in, the master and the quotes, and no order call.
    let counted = scripted::counts();
    assert!(counted.calls - before.calls >= 3, "{:?}", counted);
    assert_eq!(counted.order_calls, before.order_calls);
    app.ctx.shutdown().await;
}
