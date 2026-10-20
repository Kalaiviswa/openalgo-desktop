//! BF-01 end to end: Samco's order updates come from polling its order
//! book (web `_POLLING_BROKERS`). On the real `AppState`, a Samco session
//! started by the broker runtime polls a fake Samco through the real
//! adapter. A fill there reaches the bus (`order.update`), the Socket.IO
//! `order_update` push the order pages refresh on, the 8765 order stream,
//! and the order reconciler, whose pass reads the same book through the
//! live order gateway. Logout stops the poller. Ports are ephemeral.

use super::samco::{book_with, samco_routes};
use super::support::*;
use crate::broker_session_e2e::{free_port, pin_ports, OrderEvents, SocketPushes};
use crate::feed_support::{eventually, Client};
use openalgo_desktop_lib::brokers::samco::SamcoBroker;
use openalgo_desktop_lib::brokers::BrokerRegistry;
use openalgo_desktop_lib::clock::SystemClock;
use openalgo_desktop_lib::db::sqlite::user;
use openalgo_desktop_lib::events::subscribers::socketio::SocketIoSubscriber;
use openalgo_desktop_lib::events::{Event, Lane, SessionEndReason};
use openalgo_desktop_lib::feed::FeedService;
use openalgo_desktop_lib::security::keystore::MemoryKeyStore;
use openalgo_desktop_lib::security::Secret;
use openalgo_desktop_lib::services::apikey_service::ApiKeyService;
use openalgo_desktop_lib::services::broker_auth_service::BrokerAuthService;
use openalgo_desktop_lib::services::order_reconciler::{BookSnapshot, OrderOwner};
use openalgo_desktop_lib::state::{AppState, BrokerSession, OpenOptions, ServerStatus};
use openalgo_desktop_lib::strategy::dispatch::RunMode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The open SBIN order in the recorded Samco book.
const ORDER: &str = "261003000000101";

/// The poller's interval is 5 s in the app; a change shows within two.
const WAIT: Duration = Duration::from_secs(30);

/// An owner holding the Samco order as working, recording what each
/// reconcile pass read for it.
#[derive(Default)]
struct HoldsTheOrder(Mutex<Vec<String>>);

#[async_trait::async_trait]
impl OrderOwner for HoldsTheOrder {
    fn name(&self) -> &'static str {
        "bf01-owner"
    }
    fn has_open_orders(&self, mode: RunMode) -> bool {
        mode == RunMode::Live
    }
    fn owns_order(&self, order_id: &str) -> bool {
        order_id == ORDER
    }
    async fn reconcile(&self, _mode: RunMode, book: &BookSnapshot) -> usize {
        if let Some(o) = book.by_id(ORDER) {
            self.0.lock().push(o.status.clone());
        }
        0
    }
}

/// A fake Samco whose order book shows ORDER filled once `filled` is set.
async fn samco(filled: Arc<AtomicBool>) -> Fake {
    Fake::start(move |r: &Req| match r.path.as_str() {
        "/order/orderBook" if filled.load(Ordering::SeqCst) => ok(book_with(
            &[
                (ORDER, "filledQuantity", "10"),
                (ORDER, "unfilledQuantity", "0"),
                (ORDER, "averagePrice", "812.45"),
                (ORDER, "orderStatus", "Executed"),
            ],
            None,
        )),
        _ => samco_routes(r),
    })
    .await
}

fn open(samco: Arc<SamcoBroker>, symbols: SymbolResolver) -> (tempfile::TempDir, Arc<AppState>) {
    let dir = tempfile::tempdir().unwrap();
    let registry = BrokerRegistry::with_symbols(symbols, vec![samco as Arc<dyn Broker>]);
    let ctx = AppState::open(
        dir.path(),
        OpenOptions {
            keystore: Arc::new(MemoryKeyStore::new()),
            clock: Arc::new(SystemClock),
            brokers: Arc::new(registry),
        },
    )
    .unwrap();
    (dir, ctx)
}

/// A feed client signed in with a fresh API key, subscribed to orders.
async fn order_stream(ctx: &Arc<AppState>) -> (Arc<FeedService>, Client) {
    {
        let conn = ctx.sqlite.conn().unwrap();
        user::insert(
            &conn,
            &ctx.security,
            "alice",
            "alice@example.com",
            "not-a-real-hash",
            "JBSWY3DPEHPK3PXP",
        )
        .unwrap();
    }
    let key = ApiKeyService::regenerate(ctx, "alice").unwrap();
    pin_ports(ctx, free_port().await, free_port().await);
    let feed = FeedService::new(ctx.clone());
    assert!(matches!(feed.start().await, ServerStatus::Running { .. }));
    let mut client = Client::connect(&format!("ws://{}", feed.local_addr().await.unwrap())).await;
    let v = client
        .request(json!({"action": "authenticate", "api_key": key.expose()}))
        .await;
    assert_eq!(v["status"], "success", "{}", v);
    let v = client
        .request(json!({"action": "subscribe_orders", "request_id": "o1"}))
        .await;
    assert_eq!(v["status"], "success", "{}", v);
    (feed, client)
}

fn order_update(events: &OrderEvents) -> Option<openalgo_desktop_lib::events::OrderUpdate> {
    events.0.lock().iter().find_map(|e| match &**e {
        Event::OrderUpdate(u) if u.orderid == ORDER => Some(u.clone()),
        _ => None,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_samco_fill_reaches_the_bus_the_page_the_stream_and_the_reconciler() {
    let filled = Arc::new(AtomicBool::new(false));
    let fake = samco(filled.clone()).await;
    let symbols = SymbolResolver::new();
    let adapter = Arc::new(SamcoBroker::with_urls(
        symbols.clone(),
        &fake.base,
        format!("{}/doc/ScripMaster.csv", fake.base),
        "ws://127.0.0.1:9",
    ));
    let (_dir, ctx) = open(adapter.clone(), symbols);
    let pushes = Arc::new(SocketPushes::default());
    ctx.bus.subscribe(
        Arc::new(SocketIoSubscriber::new(pushes.clone())),
        Lane::BestEffort,
    );
    let events = Arc::new(OrderEvents::default());
    ctx.bus.subscribe(events.clone(), Lane::BestEffort);

    // A Samco session: the runtime downloads the master and starts the
    // order poller through `create_order_feed`.
    let session = BrokerSession {
        broker_id: "samco".into(),
        auth_token: Secret::new("<SESSION_TOKEN>"),
        feed_token: None,
        user_id: "<USER_ID>".into(),
        user_name: None,
        authenticated_at: ctx.now(),
    };
    ctx.set_broker_session(Some(session.clone()));
    ctx.runtime.activate(&ctx, &session).await;
    // The first poll reads the book as it stands and publishes nothing.
    assert!(
        eventually(WAIT, || !fake.calls("/order/orderBook").is_empty()).await,
        "the order poller never read the book"
    );
    assert!(adapter.order_updates_running());
    assert!(order_update(&events).is_none());
    // An 8765 client on the session's order stream.
    let (feed, mut client) = order_stream(&ctx).await;

    // The reconciler's owner holds the order from here; then it fills.
    let owner = Arc::new(HoldsTheOrder::default());
    ctx.reconciler.register(owner.clone());
    filled.store(true, Ordering::SeqCst);

    // The bus: one `order.update` for this session, as the web's event.
    assert!(
        eventually(WAIT, || order_update(&events).is_some()).await,
        "the fill never reached the bus"
    );
    let u = order_update(&events).unwrap();
    assert_eq!((u.mode.as_str(), u.broker.as_str()), ("live", "samco"));
    assert_eq!(u.order_status, "complete");
    assert_eq!((u.filled_quantity, u.pending_quantity), (10, 0));
    assert_eq!(u.average_price, 812.45);
    assert_eq!((u.symbol.as_str(), u.exchange.as_str()), ("SBIN", "NSE"));

    // The page: the Socket.IO push the order book refreshes on.
    assert!(
        eventually(WAIT, || !pushes.named("order_update").is_empty()).await,
        "no order_update push"
    );
    assert_eq!(pushes.named("order_update")[0]["orderid"], ORDER);

    // The 8765 order stream.
    let frame = loop {
        let v = client.recv().await;
        if v["type"] == "order_update" {
            break v;
        }
    };
    assert_eq!(frame["orderid"], ORDER);
    assert_eq!(frame["broker"], "samco");
    assert_eq!(frame["order_status"], "complete");

    // The reconciler reads the same book through the live gateway.
    ctx.reconciler.pass().await;
    assert_eq!(owner.0.lock().last().map(String::as_str), Some("complete"));

    // Logout stops the poller with the session.
    BrokerAuthService::revoke(&ctx, SessionEndReason::Logout)
        .await
        .unwrap();
    assert!(!adapter.order_updates_running());
    assert_eq!(ctx.runtime.task_count(), 0);

    drop(client);
    feed.stop().await;
    ctx.shutdown().await;
}
