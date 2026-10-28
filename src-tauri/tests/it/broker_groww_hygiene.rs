//! Resource hygiene of the Groww feed (CLAUDE.md "Measure, do not just
//! read"): a fake Groww accepts the NATS handshake and then drops every
//! connection, so the manager reconnects 150 times (each `prepare` minting
//! a fresh socket token and key pair). Descriptors and RSS must stay flat,
//! and disconnecting must release every socket (the test name is kept
//! from the loopback-relay design it replaced). The test runs in its own
//! process (`isolated!`) so the count is not disturbed.

use axum::routing::post;
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use openalgo_desktop_lib::brokers::common::streaming::{FeedMode, FeedSubscription};
use openalgo_desktop_lib::brokers::common::symbols::SymbolResolver;
use openalgo_desktop_lib::brokers::groww::GrowwBroker;
use openalgo_desktop_lib::brokers::types::AuthToken;
use openalgo_desktop_lib::brokers::Broker;
use openalgo_desktop_lib::websocket::{FeedConfig, WebSocketManager};
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

#[cfg(unix)]
fn open_fds() -> usize {
    std::fs::read_dir("/dev/fd").map(|d| d.count()).unwrap_or(0)
}

#[cfg(not(unix))]
fn open_fds() -> usize {
    0
}

fn rss_kb() -> u64 {
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_reconnects_do_not_leak() {
    crate::isolated!(relay_reconnects_do_not_leak);
    let app = Router::new().route(
        "/token",
        post(|| async { Json(json!({"token": "jwt", "subscriptionId": "s"})) }),
    );
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let token_url = format!("http://{}/token", l.local_addr().unwrap());
    let http = tokio::spawn(async move {
        let _ = axum::serve(l, app).await;
    });

    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws_url = format!("ws://{}", l.local_addr().unwrap());
    let accepted = Arc::new(AtomicU64::new(0));
    let acc = accepted.clone();
    let server = tokio::spawn(async move {
        while let Ok((tcp, _)) = l.accept().await {
            acc.fetch_add(1, Ordering::Relaxed);
            tokio::spawn(async move {
                if let Ok(mut ws) = tokio_tungstenite::accept_async(tcp).await {
                    let _ = ws
                        .send(Message::Text("INFO {\"nonce\":\"n\"}\r\n".into()))
                        .await;
                    // Read the CONNECT, then drop the connection.
                    let _ = tokio::time::timeout(Duration::from_millis(200), ws.next()).await;
                    let _ = ws.close(None).await;
                }
            });
        }
    });

    let broker = GrowwBroker::with_base_url(SymbolResolver::new(), "http://127.0.0.1:9")
        .with_feed_endpoints(&token_url, &ws_url);
    let auth = AuthToken::new("session");
    let manager = WebSocketManager::with_config(FeedConfig {
        backoff_base: Duration::from_millis(1),
        backoff_max: Duration::from_millis(4),
        stall_timeout: Duration::from_secs(5),
        connect_timeout: Duration::from_secs(2),
        stable_after: Duration::from_secs(60),
        ..FeedConfig::default()
    });
    manager
        .subscribe(vec![FeedSubscription {
            symbol: "SBIN".into(),
            exchange: "NSE".into(),
            token: "3045".into(),
            brsymbol: "SBIN".into(),
            brexchange: "NSE".into(),
            mode: FeedMode::Quote,
            depth: 5,
        }])
        .await
        .unwrap();
    let fds_idle = open_fds();
    manager
        .connect(broker.create_feed(&auth).unwrap())
        .await
        .unwrap();
    while accepted.load(Ordering::Relaxed) < 10 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let fds_before = open_fds();
    let rss_before = rss_kb();
    let mut peak = fds_before;
    while accepted.load(Ordering::Relaxed) < 160 {
        tokio::time::sleep(Duration::from_millis(5)).await;
        peak = peak.max(open_fds());
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let fds_after = open_fds();
    let rss_after = rss_kb();
    manager.disconnect().await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let fds_stopped = open_fds();
    server.abort();
    http.abort();

    eprintln!(
        "groww feed hygiene: fds idle={} before={} peak={} after={} stopped={}; rss {}KB -> {}KB",
        fds_idle, fds_before, peak, fds_after, fds_stopped, rss_before, rss_after
    );
    assert!(
        fds_after <= fds_before + 6,
        "descriptor leak: {} -> {}",
        fds_before,
        fds_after
    );
    assert!(
        peak <= fds_before + 12,
        "descriptor growth while reconnecting: {}",
        peak
    );
    // Disconnect drops the feed and closes its socket.
    assert!(
        fds_stopped <= fds_idle + 6,
        "sockets not released: idle {} -> stopped {}",
        fds_idle,
        fds_stopped
    );
    if rss_before > 0 {
        assert!(
            rss_after < rss_before + 16 * 1024,
            "RSS grew {} -> {} KB",
            rss_before,
            rss_after
        );
    }
}

/// A fake Groww whose socket-token endpoint answers `status`, counting the
/// requests, and a feed listener counting connections (none may arrive: the
/// login token is never used to connect).
async fn token_failure_endpoints(
    status: axum::http::StatusCode,
) -> (String, String, Arc<AtomicU64>, Arc<AtomicU64>) {
    let hits = Arc::new(AtomicU64::new(0));
    let h = hits.clone();
    let app = Router::new().route(
        "/token",
        post(move || {
            h.fetch_add(1, Ordering::Relaxed);
            async move { status }
        }),
    );
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let token_url = format!("http://{}/token", l.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(l, app).await;
    });
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws_url = format!("ws://{}", l.local_addr().unwrap());
    let accepted = Arc::new(AtomicU64::new(0));
    let acc = accepted.clone();
    tokio::spawn(async move {
        while let Ok((_tcp, _)) = l.accept().await {
            acc.fetch_add(1, Ordering::Relaxed);
        }
    });
    // Warm the shared broker client so its pooled connection to the fake
    // server (and that server's connection task) are in every baseline:
    // they belong to the one client per process, not to a retry.
    let _ = openalgo_desktop_lib::brokers::common::http::client()
        .post(&token_url)
        .send()
        .await;
    hits.store(0, Ordering::Relaxed);
    (token_url, ws_url, hits, accepted)
}

fn fast_manager() -> WebSocketManager {
    WebSocketManager::with_config(FeedConfig {
        backoff_base: Duration::from_millis(1),
        backoff_max: Duration::from_millis(4),
        stall_timeout: Duration::from_secs(5),
        connect_timeout: Duration::from_secs(2),
        stable_after: Duration::from_secs(60),
        ..FeedConfig::default()
    })
}

/// A socket-token outage: the manager backs off and asks again (150 times
/// here) without connecting with the login token, and without growing
/// descriptors, memory or tasks; disconnecting ends the supervisor.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn socket_token_outage_retries_do_not_leak() {
    crate::isolated!(socket_token_outage_retries_do_not_leak);
    let (token_url, ws_url, hits, accepted) =
        token_failure_endpoints(axum::http::StatusCode::INTERNAL_SERVER_ERROR).await;
    let broker = GrowwBroker::with_base_url(SymbolResolver::new(), "http://127.0.0.1:9")
        .with_feed_endpoints(&token_url, &ws_url);
    let manager = fast_manager();
    let metrics = tokio::runtime::Handle::current().metrics();
    let tasks_idle = metrics.num_alive_tasks();
    manager
        .connect(broker.create_feed(&AuthToken::new("session")).unwrap())
        .await
        .unwrap();
    while hits.load(Ordering::Relaxed) < 10 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let (fds_before, rss_before, tasks_before) = (open_fds(), rss_kb(), metrics.num_alive_tasks());
    while hits.load(Ordering::Relaxed) < 160 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (fds_after, rss_after, tasks_after) = (open_fds(), rss_kb(), metrics.num_alive_tasks());
    manager.disconnect().await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let tasks_stopped = metrics.num_alive_tasks();

    eprintln!(
        "groww token outage hygiene: fds {} -> {}; rss {}KB -> {}KB; tasks idle={} before={} after={} stopped={}",
        fds_before, fds_after, rss_before, rss_after, tasks_idle, tasks_before, tasks_after, tasks_stopped
    );
    assert_eq!(
        accepted.load(Ordering::Relaxed),
        0,
        "connected with the login token"
    );
    assert!(
        fds_after <= fds_before + 6,
        "descriptor leak: {} -> {}",
        fds_before,
        fds_after
    );
    assert!(
        tasks_after <= tasks_before,
        "task growth: {} -> {}",
        tasks_before,
        tasks_after
    );
    assert_eq!(
        tasks_stopped, tasks_idle,
        "supervisor still running after disconnect"
    );
    if rss_before > 0 {
        assert!(
            rss_after < rss_before + 16 * 1024,
            "RSS grew {} -> {} KB",
            rss_before,
            rss_after
        );
    }
}

/// A refused socket token (expired login) stops the feed at once: one
/// request, no retries, no connection with the login token.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refused_socket_token_stops_the_feed_without_retrying() {
    let (token_url, ws_url, hits, accepted) =
        token_failure_endpoints(axum::http::StatusCode::UNAUTHORIZED).await;
    let broker = GrowwBroker::with_base_url(SymbolResolver::new(), "http://127.0.0.1:9")
        .with_feed_endpoints(&token_url, &ws_url);
    let manager = fast_manager();
    let metrics = tokio::runtime::Handle::current().metrics();
    let tasks_idle = metrics.num_alive_tasks();
    manager
        .connect(broker.create_feed(&AuthToken::new("session")).unwrap())
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !matches!(
        manager.status(),
        openalgo_desktop_lib::websocket::FeedStatus::AuthFailed { .. }
    ) {
        assert!(tokio::time::Instant::now() < deadline, "feed never stopped");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(hits.load(Ordering::Relaxed), 1, "retried an expired login");
    assert_eq!(
        accepted.load(Ordering::Relaxed),
        0,
        "connected with the login token"
    );
    manager.disconnect().await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        metrics.num_alive_tasks(),
        tasks_idle,
        "supervisor still running after disconnect"
    );
}
