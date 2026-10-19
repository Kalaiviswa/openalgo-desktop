//! Credentials never leak (web `test_feed_logs_never_carry_the_access_token`,
//! the broker-integration secrets check): every sign-in, REST and feed
//! error path runs with sentinel credentials against a closed port
//! (transport errors carry the URL) and a fake that refuses everything,
//! and the sentinel must appear in no log line at any level, no error
//! `Display` or `Debug`, and no trader-facing message.

use super::master;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Router;
use chrono::NaiveDate;
use openalgo_desktop_lib::brokers::common::outcome::placing;
use openalgo_desktop_lib::brokers::common::streaming::{FeedMode, FeedSubscription};
use openalgo_desktop_lib::brokers::rupeezy::streaming::RupeezyFeed;
use openalgo_desktop_lib::brokers::rupeezy::{RupeezyBroker, RupeezyUrls};
use openalgo_desktop_lib::brokers::types::*;
use openalgo_desktop_lib::brokers::{Broker, BrokerCredentials};
use openalgo_desktop_lib::error::AppError;
use openalgo_desktop_lib::websocket::{FeedConfig, WebSocketManager};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const S: &str = "SENTINELq7Zx";

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn capture() -> (Captured, tracing::subscriber::DefaultGuard) {
    let c = Captured::default();
    let w = c.clone();
    let sub = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || w.clone())
        .finish();
    (c, tracing::subscriber::set_default(sub))
}

fn clean(what: &str, text: &str) {
    assert!(!text.contains(S), "{} leaked the sentinel: {}", what, text);
}

fn check<T>(what: &str, r: openalgo_desktop_lib::error::Result<T>) {
    if let Err(e) = r {
        check_err(what, &e);
    }
}

fn check_err(what: &str, e: &AppError) {
    clean(what, &format!("{} | {:?} | {}", e, e, e.client_message()));
}

fn closed_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

async fn refusing_http() -> String {
    let app = Router::new().fallback(|| async {
        (
            StatusCode::UNAUTHORIZED,
            [("content-type", "application/json")],
            r#"{"status":"error","code":"e-101","message":"unauthorized"}"#,
        )
            .into_response()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{}", addr)
}

fn creds() -> BrokerCredentials {
    BrokerCredentials {
        api_key: S.into(),
        api_secret: Some(S.into()),
        client_id: Some(S.into()),
        auth_code: Some(S.into()),
        request_token: Some(S.into()),
        ..Default::default()
    }
}

async fn drive(host: &str) {
    let b = RupeezyBroker::with_base_url(master(), RupeezyUrls::rebased(host, "ws://127.0.0.1:1"));
    check("authenticate", b.authenticate(creds()).await);
    b.restore_session(&creds());
    let auth = AuthToken::new(S);
    let o = ResolvedOrder::resolve(
        &OrderRequest {
            symbol: "SBIN".into(),
            exchange: "NSE".into(),
            side: "BUY".into(),
            quantity: 1,
            price: 0.0,
            order_type: "MARKET".into(),
            product: "MIS".into(),
            validity: "DAY".into(),
            trigger_price: None,
            disclosed_quantity: None,
            amo: false,
        },
        &master(),
    )
    .unwrap();
    check("place", placing(b.place_order(&auth, &o)).await);
    check("cancel", b.cancel_order(&auth, S).await);
    check("orders", b.get_order_book(&auth).await);
    check("trades", b.get_trade_book(&auth).await);
    check("positions", b.get_positions(&auth).await);
    check("holdings", b.get_holdings(&auth).await);
    check("funds", b.get_funds(&auth).await);
    check(
        "quote",
        b.get_quote(&auth, &QuoteKey::new("NSE", "SBIN")).await,
    );
    check(
        "history",
        b.get_history(
            &auth,
            &HistoryRequest {
                key: QuoteKey::new("NSE", "SBIN"),
                interval: "D".into(),
                start: NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
                end: NaiveDate::from_ymd_opt(2026, 10, 2).unwrap(),
            },
        )
        .await,
    );
    check("gtt book", b.get_gtt_book(&auth, true).await);
    check("cancel gtt", b.cancel_gtt(&auth, S).await);
    check("master", b.download_master_contract(&auth).await);
}

#[tokio::test]
async fn no_credential_reaches_a_log_or_an_error() {
    let (log, _guard) = capture();
    drive(&format!("http://127.0.0.1:{}", closed_port())).await;
    drive(&refusing_http().await).await;

    // The feed's address carries the token: a connect that fails is logged
    // without it.
    let manager = WebSocketManager::with_config(FeedConfig {
        backoff_base: Duration::from_millis(5),
        backoff_max: Duration::from_millis(20),
        connect_timeout: Duration::from_millis(500),
        ..FeedConfig::default()
    });
    manager
        .subscribe(vec![FeedSubscription {
            symbol: "SBIN".into(),
            exchange: "NSE".into(),
            token: "3045".into(),
            brsymbol: "SBIN".into(),
            brexchange: "NSE_EQ".into(),
            mode: FeedMode::Ltp,
            depth: 5,
        }])
        .await
        .unwrap();
    let feed = RupeezyFeed::new(&format!("ws://127.0.0.1:{}/ws", closed_port()), S);
    check("feed connect", manager.connect(Box::new(feed)).await);
    tokio::time::sleep(Duration::from_millis(300)).await;
    check("feed disconnect", manager.disconnect().await);

    let text = String::from_utf8_lossy(&log.0.lock().unwrap()).into_owned();
    assert!(!text.is_empty(), "nothing was captured");
    assert!(
        text.contains("Rupeezy"),
        "the adapter's logging was not captured"
    );
    clean("logs", &text);
}
