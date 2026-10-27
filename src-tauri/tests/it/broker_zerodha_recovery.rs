//! Web openalgo#2305 ("reconcile unanswered Kite order writes"): when Kite's
//! answer to an order is lost after the order was sent, the adapter looks
//! for it in Kite's order book by the order's own tag for a short window
//! (web `_recover_order_by_tag`), never sending it again. Found, the order
//! is placed with Kite's id; not found, it stays uncertain with its tag for
//! the reconciler. A raw local fake Kite reads the order, keeps its tag and
//! drops the connection without answering.

use crate::broker_support::symbols;
use openalgo_desktop_lib::brokers::common::outcome::{classify, placing, PlaceOutcome};
use openalgo_desktop_lib::brokers::types::*;
use openalgo_desktop_lib::brokers::zerodha::ZerodhaBroker;
use openalgo_desktop_lib::brokers::Broker;
use parking_lot::Mutex;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Default)]
struct Kite {
    /// The tag of the order Kite took.
    tag: Mutex<Option<String>>,
    posts: AtomicUsize,
    book_reads: AtomicUsize,
    /// The order book shows the order from this read on (1 = the first);
    /// 0 never.
    shows_from: AtomicUsize,
}

/// One request: the request line and the body.
async fn read_request(s: &mut TcpStream) -> Option<(String, String)> {
    let mut got = Vec::new();
    let mut buf = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = got.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        let n = s.read(&mut buf).await.ok()?;
        if n == 0 {
            return None;
        }
        got.extend_from_slice(&buf[..n]);
    };
    let head = String::from_utf8_lossy(&got[..head_end]).to_string();
    let len = head
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0);
    while got.len() < head_end + len {
        let n = s.read(&mut buf).await.ok()?;
        if n == 0 {
            return None;
        }
        got.extend_from_slice(&buf[..n]);
    }
    let line = head.lines().next().unwrap_or("").to_string();
    let body = String::from_utf8_lossy(&got[head_end..head_end + len]).to_string();
    Some((line, body))
}

fn book(kite: &Kite) -> String {
    let read = kite.book_reads.fetch_add(1, Ordering::SeqCst) + 1;
    let from = kite.shows_from.load(Ordering::SeqCst);
    let rows = match kite.tag.lock().clone() {
        Some(tag) if from > 0 && read >= from => vec![json!({
            "order_id": "261027000000777", "exchange_order_id": null,
            "status": "OPEN", "order_timestamp": "2026-10-27 10:00:00",
            "exchange": "NSE", "tradingsymbol": "SBIN", "order_type": "LIMIT",
            "transaction_type": "BUY", "validity": "DAY", "product": "MIS",
            "quantity": 1, "price": 800, "trigger_price": 0, "average_price": 0,
            "filled_quantity": 0, "pending_quantity": 1, "tag": tag
        })],
        _ => Vec::new(),
    };
    json!({"status": "success", "data": rows}).to_string()
}

async fn serve(kite: Arc<Kite>) -> String {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            let kite = kite.clone();
            tokio::spawn(async move {
                while let Some((line, body)) = read_request(&mut s).await {
                    if line.starts_with("POST /orders/regular") {
                        kite.posts.fetch_add(1, Ordering::SeqCst);
                        let tag = body
                            .split('&')
                            .find_map(|kv| kv.strip_prefix("tag="))
                            .map(str::to_string);
                        *kite.tag.lock() = tag;
                        // The order is taken; the answer never comes back.
                        return;
                    }
                    let answer = if line.starts_with("GET /orders ") {
                        book(&kite)
                    } else {
                        json!({"status": "error", "message": "unexpected"}).to_string()
                    };
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                        answer.len(),
                        answer
                    );
                    if s.write_all(reply.as_bytes()).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    base
}

fn order() -> ResolvedOrder {
    ResolvedOrder::resolve(
        &OrderRequest {
            symbol: "SBIN".into(),
            exchange: "NSE".into(),
            side: "BUY".into(),
            quantity: 1,
            price: 800.0,
            order_type: "LIMIT".into(),
            product: "MIS".into(),
            validity: "DAY".into(),
            trigger_price: None,
            disclosed_quantity: None,
            amo: false,
        },
        &symbols(),
    )
    .unwrap()
}

async fn place(kite: &Arc<Kite>, window: Duration) -> PlaceOutcome {
    let base = serve(kite.clone()).await;
    let b = ZerodhaBroker::with_base_url(symbols(), base).with_recovery_window(window);
    let auth = AuthToken::new("kitekey:accesstok");
    classify(&placing(b.place_order(&auth, &order())).await)
}

#[tokio::test]
async fn an_order_whose_answer_was_lost_is_found_by_its_tag() {
    let kite = Arc::new(Kite::default());
    kite.shows_from.store(1, Ordering::SeqCst);
    let o = place(&kite, Duration::from_secs(2)).await;
    assert_eq!(
        o,
        PlaceOutcome::Accepted {
            order_id: "261027000000777".into()
        }
    );
    assert_eq!(kite.posts.load(Ordering::SeqCst), 1, "never sent twice");
}

#[tokio::test]
async fn the_book_is_read_again_until_the_order_shows() {
    let kite = Arc::new(Kite::default());
    kite.shows_from.store(3, Ordering::SeqCst);
    let o = place(&kite, Duration::from_secs(2)).await;
    assert!(matches!(o, PlaceOutcome::Accepted { .. }), "{:?}", o);
    assert_eq!(kite.book_reads.load(Ordering::SeqCst), 3);
    assert_eq!(kite.posts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_order_missing_from_the_book_stays_uncertain_with_its_tag() {
    let kite = Arc::new(Kite::default());
    let o = place(&kite, Duration::from_millis(300)).await;
    let sent = kite.tag.lock().clone();
    assert!(
        sent.as_deref().is_some_and(|t| t.starts_with("oa")),
        "{:?}",
        sent
    );
    match o {
        PlaceOutcome::Uncertain { client_tag, .. } => assert_eq!(client_tag, sent),
        other => panic!("{:?}", other),
    }
    assert!(kite.book_reads.load(Ordering::SeqCst) >= 1);
    assert_eq!(kite.posts.load(Ordering::SeqCst), 1, "never sent twice");
}
