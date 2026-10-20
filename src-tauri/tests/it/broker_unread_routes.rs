//! BR-03 through the routes (NEW-PAR-01, NEW-PAR-02): `/api/v1/placesmartorder`
//! and `/api/v1/closeposition` on the real app, with a real adapter
//! signed in against a local fake broker.
//!
//! A position book in the wrong shape, or a row without a readable
//! quantity, is an unknown position: the smart order is refused and sends
//! nothing, and close all never answers "All Open Positions Squared Off".
//! Groww's close all with its F&O segment unread is not complete either.
//! Controls: the broker's own empty book reads as flat through the same
//! routes (the web's no-position answer, a smart order that places, close
//! all complete), so the refusals are for the shape alone. Replies keep
//! the web's envelope.

use crate::api_v1_support::{post_json, session_time};
use crate::broker_support::{fake, sbin, Fake};
use axum::extract::ConnectInfo;
use axum::http::StatusCode;
use http_body_util::BodyExt;
use openalgo_desktop_lib::brokers::common::symbols::SymbolResolver;
use openalgo_desktop_lib::brokers::{Broker, BrokerRegistry};
use openalgo_desktop_lib::clock::ManualClock;
use openalgo_desktop_lib::security::keystore::MemoryKeyStore;
use openalgo_desktop_lib::services::apikey_service::ApiKeyService;
use openalgo_desktop_lib::services::auth_service::AuthService;
use openalgo_desktop_lib::services::broker_auth_service::BrokerAuthService;
use openalgo_desktop_lib::services::order_service::NO_OPEN_POSITION;
use openalgo_desktop_lib::state::{AppState, BrokerSession, OpenOptions};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use tower::ServiceExt;

const SQUARED_OFF: &str = "All Open Positions Squared Off";

/// The app with one real adapter signed in, and its fake broker.
struct App {
    ctx: Arc<AppState>,
    key: String,
    fake: Fake,
    /// Where the adapter sends an order.
    place: &'static str,
    /// A fresh client address per request, so the per-IP limits never
    /// interfere.
    ip: AtomicU8,
    _dir: tempfile::TempDir,
}

impl App {
    async fn open(
        fake: Fake,
        broker: impl FnOnce(SymbolResolver, &str) -> Arc<dyn Broker>,
        token: String,
        place: &'static str,
    ) -> App {
        let dir = tempfile::tempdir().unwrap();
        let symbols = SymbolResolver::new();
        let adapter = broker(symbols.clone(), &fake.base);
        let id = adapter.id().to_string();
        let ctx = AppState::open(
            dir.path(),
            OpenOptions {
                keystore: Arc::new(MemoryKeyStore::new()),
                clock: ManualClock::new(session_time()),
                brokers: Arc::new(BrokerRegistry::with_symbols(symbols, vec![adapter])),
            },
        )
        .unwrap();
        ctx.load_symbol_cache(vec![sbin()]);
        AuthService::setup(&ctx, "trader", "trader@example.com", "Secret@123").unwrap();
        let key = ApiKeyService::current(&ctx)
            .unwrap()
            .unwrap()
            .expose()
            .to_string();
        BrokerAuthService::persist(
            &ctx,
            &BrokerSession {
                broker_id: id,
                auth_token: token.into(),
                feed_token: None,
                user_id: "<USER_ID>".into(),
                user_name: None,
                authenticated_at: ctx.now(),
            },
        )
        .unwrap();
        App {
            ctx,
            key,
            fake,
            place,
            ip: AtomicU8::new(1),
            _dir: dir,
        }
    }

    async fn post(&self, path: &str, mut body: Value) -> (StatusCode, Value) {
        body["apikey"] = json!(self.key);
        let mut req = post_json(path, body);
        let ip = self.ip.fetch_add(1, Ordering::Relaxed);
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([10, 0, 0, ip], 40000))));
        crate::with_host(&mut req, &self.ctx);
        let resp = openalgo_desktop_lib::server::app(self.ctx.clone())
            .oneshot(req)
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// A smart order for SBIN NSE MIS to a position of `size`.
    async fn smart_order(&self, size: i64) -> (StatusCode, Value) {
        self.post(
            "/api/v1/placesmartorder",
            json!({
                "strategy": "br03",
                "exchange": "NSE",
                "symbol": "SBIN",
                "product": "MIS",
                "pricetype": "MARKET",
                "action": "BUY",
                "quantity": size,
                "position_size": size,
            }),
        )
        .await
    }

    async fn close_all(&self) -> (StatusCode, Value) {
        self.post("/api/v1/closeposition", json!({"strategy": "br03"}))
            .await
    }

    fn orders_sent(&self) -> usize {
        self.fake.sent(self.place)
    }

    async fn shutdown(self) {
        self.ctx.shutdown().await;
    }
}

/// The reply is the web's error envelope, with a trader's message.
fn refused(what: &str, (status, v): &(StatusCode, Value)) {
    assert!(!status.is_success(), "{}: {} {}", what, status, v);
    assert_eq!(v["status"], "error", "{}: {}", what, v);
    let m = v["message"].as_str().unwrap_or_default();
    assert!(!m.is_empty() && m != SQUARED_OFF, "{}: {}", what, v);
}

/// Every unknown position in `unread`: the smart order and close all are
/// refused and nothing is sent. Then each of the broker's empty books:
/// flat, and a smart order goes through to the broker.
async fn check(
    app: &App,
    positions: &[&'static str],
    unread: &[(&str, &str)],
    empties: &[(&str, &str)],
) {
    for (what, body) in unread {
        for p in positions {
            app.fake.route(p, 200, body);
        }
        // Refused for the position, before any order: a refused placement
        // would also answer an error.
        let reply = app.smart_order(5).await;
        assert_eq!(app.orders_sent(), 0, "{}: the smart order was sent", what);
        refused(&format!("{}: smart order", what), &reply);
        let m = reply.1["message"].as_str().unwrap_or_default();
        assert!(m.contains("so no order was sent"), "{}: {}", what, m);
        refused(&format!("{}: close all", what), &app.close_all().await);
        assert_eq!(app.orders_sent(), 0, "{}: an exit was sent", what);
    }
    for (n, (what, body)) in empties.iter().enumerate() {
        for p in positions {
            app.fake.route(p, 200, body);
        }
        let (s, v) = app.smart_order(0).await;
        assert_eq!(s, StatusCode::OK, "{}: {}", what, v);
        let flat = json!({"status": "success", "message": NO_OPEN_POSITION});
        assert_eq!(v, flat, "{}", what);
        let (s, v) = app.close_all().await;
        assert_eq!(s, StatusCode::OK, "{}: {}", what, v);
        let done = json!({"status": "success", "message": SQUARED_OFF});
        assert_eq!(v, done, "{}", what);
        assert_eq!(
            app.orders_sent(),
            n,
            "{}: close all of nothing sent an order",
            what
        );
        // The same empty book lets a smart order through to the broker.
        app.smart_order(5).await;
        assert_eq!(app.orders_sent(), n + 1, "{}: not read as flat", what);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn motilal_unknown_positions_are_never_flat_through_the_routes() {
    use openalgo_desktop_lib::brokers::motilal::{paths, MotilalBroker};
    let app = App::open(
        fake().await,
        |s, base| {
            Arc::new(MotilalBroker::with_urls(
                s,
                base,
                "ws://127.0.0.1:1",
                "ws://127.0.0.1:1",
            ))
        },
        "AT1:::ACC1:::<USER_ID>:::KEY1:::SEC1".into(),
        paths::PLACE,
    )
    .await;
    let row = |q: &str| {
        format!(
            r#"{{"status":"SUCCESS","message":"","errorcode":"","data":[{{"symboltoken":3045,"exchange":"NSE","productname":"VALUEPLUS"{}}}]}}"#,
            q
        )
    };
    let unread = [
        (
            "a text book",
            r#"{"status":"SUCCESS","data":"x"}"#.to_string(),
        ),
        (
            "an object book",
            r#"{"status":"SUCCESS","data":{"rows":[]}}"#.to_string(),
        ),
        ("a row without its quantities", row("")),
        (
            "a row whose quantity is not a number",
            row(r#","buyquantity":"abc","sellquantity":0"#),
        ),
    ];
    let unread: Vec<(&str, &str)> = unread.iter().map(|(w, b)| (*w, b.as_str())).collect();
    let empties = [
        (
            "data null",
            r#"{"status":"SUCCESS","message":"","errorcode":"","data":null}"#,
        ),
        ("an empty list", r#"{"status":"SUCCESS","data":[]}"#),
    ];
    check(&app, &[paths::POSITIONS], &unread, &empties).await;
    app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kotak_unknown_positions_are_never_flat_through_the_routes() {
    use openalgo_desktop_lib::brokers::kotak::KotakBroker;
    let f = fake().await;
    let token = format!("trade-tok:::trade-sid:::{}/:::acc-tok:::E43", f.base);
    let app = App::open(
        f,
        |s, base| {
            Arc::new(
                KotakBroker::with_urls(s, base.to_string(), format!("{}/5config", base))
                    .with_retry_base(std::time::Duration::from_millis(2)),
            )
        },
        token,
        "/quick/order/rule/ms/place",
    )
    .await;
    let unread = [
        ("a text book", r#"{"stat":"Ok","stCode":200,"data":"x"}"#),
        (
            "an object book",
            r#"{"stat":"Ok","stCode":200,"data":{"rows":[]}}"#,
        ),
        (
            "a row without its quantities",
            r#"{"stat":"Ok","stCode":200,"data":[{"trdSym":"SBIN-EQ","exSeg":"nse_cm","prod":"MIS"}]}"#,
        ),
        (
            "a row whose quantity is not a number",
            r#"{"stat":"Ok","stCode":200,"data":[{"trdSym":"SBIN-EQ","exSeg":"nse_cm","prod":"MIS","flBuyQty":"abc","flSellQty":"0","cfBuyQty":"0","cfSellQty":"0"}]}"#,
        ),
    ];
    let empties = [
        ("an empty list", r#"{"stat":"Ok","stCode":200,"data":[]}"#),
        ("data null", r#"{"stat":"Ok","stCode":200,"data":null}"#),
        (
            "a no-data refusal",
            r#"{"stat":"Not_Ok","emsg":"No Data","stCode":5203}"#,
        ),
    ];
    check(&app, &["/quick/user/positions"], &unread, &empties).await;
    app.shutdown().await;
}

const GROWW_CASH: &str = "/v1/positions/user?segment=CASH";
const GROWW_FNO: &str = "/v1/positions/user?segment=FNO";
const GROWW_EMPTY: &str = r#"{"status":"SUCCESS","payload":{"positions":[]}}"#;

async fn groww() -> App {
    use openalgo_desktop_lib::brokers::groww::GrowwBroker;
    App::open(
        fake().await,
        |s, base| Arc::new(GrowwBroker::with_base_url(s, base)),
        "groww-token".into(),
        "/v1/order/create",
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn groww_unknown_positions_are_never_flat_through_the_routes() {
    let app = groww().await;
    let unread = [
        ("a text payload", r#"{"status":"SUCCESS","payload":"x"}"#),
        (
            "a payload without positions",
            r#"{"status":"SUCCESS","payload":{}}"#,
        ),
        (
            "a row without its quantity",
            r#"{"status":"SUCCESS","payload":{"positions":[{"trading_symbol":"SBIN","segment":"CASH","exchange":"NSE","product":"INTRADAY"}]}}"#,
        ),
    ];
    let empties = [("an empty list", GROWW_EMPTY)];
    check(&app, &[GROWW_CASH, GROWW_FNO], &unread, &empties).await;
    app.shutdown().await;
}

/// NEW-PAR-01: Groww refuses the F&O segment's positions while the cash
/// segment reads empty. Close all cannot know the F&O positions are
/// closed, so it is not reported complete (the web leaves the segment out
/// and reports success); with both segments read empty it is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn groww_close_all_with_the_fno_segment_unread_is_not_complete() {
    let app = groww().await;
    app.fake.route(GROWW_CASH, 200, GROWW_EMPTY);
    app.fake.route(
        GROWW_FNO,
        400,
        r#"{"status":"FAILURE","error":{"code":"GA005","message":"Access denied"}}"#,
    );
    let reply = app.close_all().await;
    refused("close all with F&O unread", &reply);
    let m = reply.1["message"].as_str().unwrap_or_default();
    assert!(m.contains("F&O positions"), "{}", m);
    assert_eq!(app.orders_sent(), 0);

    app.fake.route(GROWW_FNO, 200, GROWW_EMPTY);
    let (s, v) = app.close_all().await;
    assert_eq!(s, StatusCode::OK, "{}", v);
    assert_eq!(v, json!({"status": "success", "message": SQUARED_OFF}));
    app.shutdown().await;
}
