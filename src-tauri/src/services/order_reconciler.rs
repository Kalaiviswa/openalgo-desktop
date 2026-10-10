//! The order reconciler: one service that settles, from the broker's order
//! book, what an order answer or update did not (Codex LOG-08).
//!
//! The broker's order book is the authority. Every [`PASS_INTERVAL`], and at
//! once when asked, the reconciler reads it **once per destination** (Live
//! through the broker session, Sandbox through the sandbox engine), and only
//! when an owner holds an open order there, and hands that one read to every
//! owner of orders (strategies, scalping, OpenScript) to fold:
//!
//! * **an order with a broker id** is folded through the owner's own fill
//!   path, which is idempotent on cumulative quantities, so a fill applied
//!   twice is booked once;
//! * **an unconfirmed placement** (sent, no definite answer from the
//!   broker) is looked up by its client tag where the broker reports one,
//!   else by its order fields among the orders nobody owns
//!   ([`find_placed`]). Found, it is bound and folded. Only an absence
//!   confirmed by [`ABSENT_READS`] reads over at least [`ABSENT_SPAN_SECS`]
//!   seconds releases the owner's claim: until then nothing is placed again.
//!
//! Hygiene: one owned task (a `JoinHandle`), aborted on shutdown and
//! restarted on every broker session change (logout, switch), so a pass
//! never applies one session's facts after another began. Owners hold their
//! module weakly. Every per-order map is pruned to the orders still
//! unconfirmed.

use crate::clock::Clock;
use crate::events::{Event, EventBus, Lane, Subscriber, Topic};
use crate::strategy::dispatch::{BookOrder, OrderGateway, RunMode};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use parking_lot::{Mutex, RwLock};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

/// How often open orders are checked against the broker's order book.
pub const PASS_INTERVAL: Duration = Duration::from_secs(5);
/// Consecutive reads that must miss an unconfirmed order before it is
/// taken as never placed.
pub const ABSENT_READS: u32 = 4;
/// ... spanning at least this long (a broker's book can lag its orders).
pub const ABSENT_SPAN_SECS: i64 = 20;
/// A book row timestamped this long before the placement cannot be it.
const TIMESTAMP_SLACK_SECS: i64 = 120;

const MODES: [RunMode; 2] = [RunMode::Live, RunMode::Sandbox];

/// One order-book read, shared by every owner in a pass.
#[derive(Clone)]
pub struct BookSnapshot {
    pub rows: Vec<BookOrder>,
    pub read_at: DateTime<Utc>,
    /// Every owner, asked whether a book order already belongs to it.
    pub owners: Vec<Arc<dyn OrderOwner>>,
}

impl BookSnapshot {
    pub fn new(rows: Vec<BookOrder>, read_at: DateTime<Utc>) -> Self {
        Self {
            rows,
            read_at,
            owners: Vec::new(),
        }
    }

    pub fn by_id(&self, order_id: &str) -> Option<&BookOrder> {
        self.rows.iter().find(|r| r.order_id == order_id)
    }

    /// Whether a book order already belongs to some owner (never a match
    /// for an unconfirmed placement).
    pub fn owned(&self, order_id: &str) -> bool {
        self.owners.iter().any(|o| o.owns_order(order_id))
    }
}

/// An owner of orders: folds a book read into its own state.
#[async_trait::async_trait]
pub trait OrderOwner: Send + Sync {
    fn name(&self) -> &'static str;
    /// Whether this owner holds working or unconfirmed orders in `mode`
    /// (a pass reads a destination's book only when someone does).
    fn has_open_orders(&self, mode: RunMode) -> bool;
    /// Whether this broker order id is already one of this owner's orders.
    fn owns_order(&self, order_id: &str) -> bool;
    /// Fold one book read. Returns how many unconfirmed placements are
    /// still unresolved.
    async fn reconcile(&self, mode: RunMode, book: &BookSnapshot) -> usize;
}

// ------------------------------------------------------------ matching

/// An unconfirmed placement, as its owner recorded it.
#[derive(Debug, Clone, Copy)]
pub struct Unconfirmed<'a> {
    pub symbol: &'a str,
    pub exchange: &'a str,
    /// `BUY` / `SELL`.
    pub action: &'a str,
    pub quantity: i64,
    /// Compared only when both sides name one.
    pub product: Option<&'a str>,
    pub client_tag: Option<&'a str>,
    pub placed_at: Option<DateTime<Utc>>,
}

/// What the book says about an unconfirmed placement.
#[derive(Debug, Clone, PartialEq)]
pub enum Found<'a> {
    Order(&'a BookOrder),
    /// Not in this read.
    Absent,
    /// Several orders fit and nothing tells them apart: never guessed.
    Ambiguous(usize),
}

/// A book row's own timestamp, read as IST (the brokers' local time).
pub fn book_time(row: &Value) -> Option<DateTime<Utc>> {
    let text = row.get("timestamp").and_then(Value::as_str)?.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(t) = DateTime::parse_from_rfc3339(text) {
        return Some(t.with_timezone(&Utc));
    }
    for f in [
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M:%S%.f",
        "%d-%m-%Y %H:%M:%S",
        "%d-%b-%Y %H:%M:%S",
        "%d/%m/%Y %H:%M:%S",
        "%H:%M:%S %d-%m-%Y",
        "%Y-%m-%dT%H:%M:%S",
    ] {
        if let Ok(n) = NaiveDateTime::parse_from_str(text, f) {
            return chrono_tz::Asia::Kolkata
                .from_local_datetime(&n)
                .single()
                .map(|t| t.with_timezone(&Utc));
        }
    }
    None
}

/// Find an unconfirmed placement in one book read.
///
/// * With a client tag and a book that reports tags, only that tag matches.
/// * Otherwise the order's symbol, exchange, side, quantity and product must
///   match, the row must not already belong to anyone (`bound`), and its own
///   timestamp, when readable, must not predate the placement.
pub fn find_placed<'a>(
    book: &'a [BookOrder],
    u: &Unconfirmed<'_>,
    bound: &dyn Fn(&str) -> bool,
) -> Found<'a> {
    let same = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b.trim());
    let fields = |r: &&BookOrder| {
        same(&r.symbol, u.symbol)
            && same(&r.exchange, u.exchange)
            && same(&r.action, u.action)
            && r.quantity == u.quantity
            && match (u.product, r.product.trim()) {
                (Some(p), rp) if !p.trim().is_empty() && !rp.is_empty() => same(p, rp),
                _ => true,
            }
    };
    let tags_reported = book.iter().any(|r| r.client_tag.is_some());
    if let (Some(tag), true) = (u.client_tag.filter(|t| !t.is_empty()), tags_reported) {
        let hits: Vec<&BookOrder> = book
            .iter()
            .filter(|r| r.client_tag.as_deref() == Some(tag))
            .filter(fields)
            .collect();
        return match hits.len() {
            0 => Found::Absent,
            1 => Found::Order(hits[0]),
            n => Found::Ambiguous(n),
        };
    }
    let earliest = u
        .placed_at
        .map(|t| t - chrono::Duration::seconds(TIMESTAMP_SLACK_SECS));
    let hits: Vec<&BookOrder> = book
        .iter()
        .filter(fields)
        .filter(|r| !bound(&r.order_id))
        .filter(|r| match (earliest, book_time(&r.row)) {
            (Some(e), Some(t)) => t >= e,
            _ => true,
        })
        .collect();
    match hits.len() {
        0 => Found::Absent,
        1 => Found::Order(hits[0]),
        n => Found::Ambiguous(n),
    }
}

/// Counts reads that missed an unconfirmed placement. Bounded by the
/// placements still unconfirmed: owners prune it every pass.
#[derive(Default)]
pub struct AbsenceTracker {
    misses: Mutex<HashMap<String, (DateTime<Utc>, u32)>>,
}

impl AbsenceTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a read without the order; true once its absence is confirmed
    /// ([`ABSENT_READS`] reads over at least [`ABSENT_SPAN_SECS`]).
    pub fn absent(&self, key: &str, now: DateTime<Utc>) -> bool {
        let mut m = self.misses.lock();
        let e = m.entry(key.to_string()).or_insert((now, 0));
        e.1 += 1;
        e.1 >= ABSENT_READS && (now - e.0).num_seconds() >= ABSENT_SPAN_SECS
    }

    /// The order was seen (or settled): start over.
    pub fn forget(&self, key: &str) {
        self.misses.lock().remove(key);
    }

    /// Keep only the keys still unconfirmed.
    pub fn retain(&self, live: &HashSet<String>) {
        self.misses.lock().retain(|k, _| live.contains(k));
    }

    /// As `retain`, for the keys of one scope (one destination's pass)
    /// only: keys starting with `scope` and not in `live` go.
    pub fn retain_scope(&self, scope: &str, live: &HashSet<String>) {
        self.misses
            .lock()
            .retain(|k, _| !k.starts_with(scope) || live.contains(k));
    }

    pub fn len(&self) -> usize {
        self.misses.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ------------------------------------------------------------ the service

/// Counters health reads.
#[derive(Debug, Default)]
pub struct ReconcileStats {
    pub passes: AtomicU64,
    pub book_reads: AtomicU64,
    pub book_read_failures: AtomicU64,
    pub unresolved: AtomicU64,
}

pub struct OrderReconciler {
    gateway: Arc<dyn OrderGateway>,
    clock: Arc<dyn Clock>,
    owners: RwLock<Vec<Arc<dyn OrderOwner>>>,
    wake: Notify,
    task: Mutex<Option<JoinHandle<()>>>,
    /// One pass at a time.
    pass_lock: tokio::sync::Mutex<()>,
    pub stats: ReconcileStats,
}

impl OrderReconciler {
    pub fn new(gateway: Arc<dyn OrderGateway>, clock: Arc<dyn Clock>) -> Arc<Self> {
        Arc::new(Self {
            gateway,
            clock,
            owners: RwLock::new(Vec::new()),
            wake: Notify::new(),
            task: Mutex::new(None),
            pass_lock: tokio::sync::Mutex::new(()),
            stats: ReconcileStats::default(),
        })
    }

    /// Add an owner of orders. Owners hold their module weakly, so this
    /// keeps nothing alive.
    pub fn register(&self, owner: Arc<dyn OrderOwner>) {
        self.owners.write().push(owner);
    }

    fn owners(&self) -> Vec<Arc<dyn OrderOwner>> {
        self.owners.read().clone()
    }

    /// Ask for a pass now (an uncertain placement was just recorded, a
    /// broker session started).
    pub fn request_pass(&self) {
        self.wake.notify_one();
    }

    /// One pass: per destination with open orders, one book read, folded
    /// by every owner.
    pub async fn pass(&self) -> Value {
        let _one = self.pass_lock.lock().await;
        self.stats.passes.fetch_add(1, Ordering::Relaxed);
        let owners = self.owners();
        let mut report = serde_json::Map::new();
        let mut unresolved_total = 0usize;
        for mode in MODES {
            let holders: Vec<Arc<dyn OrderOwner>> = owners
                .iter()
                .filter(|o| o.has_open_orders(mode))
                .cloned()
                .collect();
            let read_ok = if holders.is_empty() {
                true
            } else {
                self.stats.book_reads.fetch_add(1, Ordering::Relaxed);
                match self.gateway.order_book_rows(mode).await {
                    Ok(rows) => {
                        let book = BookSnapshot {
                            rows,
                            read_at: self.clock.now(),
                            owners: owners.clone(),
                        };
                        for o in &holders {
                            unresolved_total += o.reconcile(mode, &book).await;
                        }
                        true
                    }
                    Err(e) => {
                        self.stats
                            .book_read_failures
                            .fetch_add(1, Ordering::Relaxed);
                        tracing::warn!(
                            "Order book for {} could not be read to reconcile open orders: {}",
                            mode.as_str(),
                            e
                        );
                        false
                    }
                }
            };
            report.insert(
                mode.as_str().into(),
                json!({"owners": holders.len(), "read": read_ok}),
            );
        }
        self.stats
            .unresolved
            .store(unresolved_total as u64, Ordering::Relaxed);
        report.insert("unresolved".into(), json!(unresolved_total));
        Value::Object(report)
    }

    // ------------------------------------------------------------ lifecycle

    /// Start the owned task (idempotent).
    pub fn start(self: &Arc<Self>) {
        let mut task = self.task.lock();
        if task.as_ref().is_some_and(|t| !t.is_finished()) {
            return;
        }
        let weak = Arc::downgrade(self);
        *task = Some(tokio::spawn(async move {
            let mut tick = tokio::time::interval(PASS_INTERVAL);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                let Some(me) = weak.upgrade() else { break };
                tokio::select! {
                    _ = tick.tick() => {}
                    _ = me.wake.notified() => {}
                }
                me.pass().await;
                drop(me);
            }
        }));
    }

    /// Abort the owned task (shutdown).
    pub fn stop(&self) {
        if let Some(t) = self.task.lock().take() {
            t.abort();
        }
    }

    /// A broker session changed (logout, switch, sign-in): abort any pass in
    /// flight so it cannot apply the old session's facts, and start again.
    pub fn restart(self: &Arc<Self>) {
        self.stop();
        self.start();
    }

    pub fn is_running(&self) -> bool {
        self.task.lock().as_ref().is_some_and(|t| !t.is_finished())
    }
}

/// Restarts the reconciler on broker session changes, and reads the book at
/// once when a session starts (fills may have happened while signed out).
struct Lifecycle {
    reconciler: Weak<OrderReconciler>,
}

#[async_trait::async_trait]
impl Subscriber for Lifecycle {
    fn name(&self) -> &'static str {
        "order-reconciler"
    }

    fn topics(&self) -> Vec<Topic> {
        vec![Topic::BrokerConnected, Topic::BrokerSessionEnded]
    }

    async fn handle(&self, event: Arc<Event>) {
        let Some(r) = self.reconciler.upgrade() else {
            return;
        };
        match event.as_ref() {
            Event::BrokerConnected { .. } => {
                r.restart();
                r.request_pass();
            }
            Event::BrokerSessionEnded { .. } => r.restart(),
            _ => {}
        }
    }
}

/// Subscribe the reconciler's lifecycle on the bus.
pub fn register(bus: &EventBus, reconciler: &Arc<OrderReconciler>) {
    bus.subscribe(
        Arc::new(Lifecycle {
            reconciler: Arc::downgrade(reconciler),
        }),
        Lane::Critical,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn row(
        id: &str,
        symbol: &str,
        action: &str,
        qty: i64,
        tag: Option<&str>,
        ts: &str,
    ) -> BookOrder {
        BookOrder {
            order_id: id.into(),
            symbol: symbol.into(),
            exchange: "NFO".into(),
            action: action.into(),
            quantity: qty,
            product: "NRML".into(),
            status: "complete".into(),
            filled_quantity: qty,
            average_price: 100.0,
            client_tag: tag.map(str::to_string),
            row: json!({"orderid": id, "timestamp": ts}),
        }
    }

    fn u<'a>(tag: Option<&'a str>) -> Unconfirmed<'a> {
        Unconfirmed {
            symbol: "NIFTY24500CE",
            exchange: "NFO",
            action: "BUY",
            quantity: 75,
            product: Some("NRML"),
            client_tag: tag,
            placed_at: Some(Utc.with_ymd_and_hms(2026, 10, 7, 4, 30, 0).unwrap()),
        }
    }

    #[test]
    fn a_tag_finds_exactly_its_order() {
        let book = vec![
            row("1", "NIFTY24500CE", "BUY", 75, Some("oaother"), ""),
            row("2", "NIFTY24500CE", "BUY", 75, Some("oamine"), ""),
        ];
        assert_eq!(
            find_placed(&book, &u(Some("oamine")), &|_| false),
            Found::Order(&book[1])
        );
        assert_eq!(
            find_placed(&book, &u(Some("oamissing")), &|_| false),
            Found::Absent
        );
    }

    #[test]
    fn without_a_tag_owned_and_older_orders_are_never_taken() {
        // 10:00 IST is 04:30 UTC: the placement time.
        let book = vec![
            row(
                "old",
                "NIFTY24500CE",
                "BUY",
                75,
                None,
                "2026-10-07 09:15:00",
            ),
            row(
                "owned",
                "NIFTY24500CE",
                "BUY",
                75,
                None,
                "2026-10-07 10:00:01",
            ),
            row(
                "new",
                "NIFTY24500CE",
                "BUY",
                75,
                None,
                "2026-10-07 10:00:02",
            ),
            row(
                "other",
                "NIFTY24500CE",
                "SELL",
                75,
                None,
                "2026-10-07 10:00:02",
            ),
        ];
        let found = find_placed(&book, &u(None), &|id| id == "owned");
        assert_eq!(found, Found::Order(&book[2]));
        // Two unowned candidates: never a guess.
        let found = find_placed(&book, &u(None), &|_| false);
        assert_eq!(found, Found::Ambiguous(2));
    }

    #[test]
    fn absence_needs_several_reads_over_time() {
        let t = AbsenceTracker::new();
        let t0 = Utc.with_ymd_and_hms(2026, 10, 7, 4, 30, 0).unwrap();
        for i in 0..ABSENT_READS {
            assert!(!t.absent("a", t0 + chrono::Duration::seconds(i as i64)));
        }
        assert!(t.absent("a", t0 + chrono::Duration::seconds(ABSENT_SPAN_SECS)));
        t.retain(&HashSet::new());
        assert!(t.is_empty());
    }
}
