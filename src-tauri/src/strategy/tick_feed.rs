//! Prices for live runs (web `services/strategy_module/tick_feed.py`).
//!
//! Subscriptions are refcounted per run, so two strategies holding one
//! contract share one and it is released when the last lets go. Both the
//! streaming feed and the REST fallback drive `process_tick`: a leg that has
//! fallen back to polling is risk-evaluated on polled prices too.
//!
//! Two owned tasks, ended on module shutdown:
//!
//! * the **reader** takes every tick of the market-data broadcast and keeps
//!   the prices of held symbols in their paths (`pending`), never waiting
//!   on an evaluation, so a slow decision cannot make the channel overrun it
//!   and skip a deciding price. A tick it does miss is counted for health;
//! * the **evaluator** takes the waiting paths and evaluates each price in
//!   order, and polls stale symbols every 2 s (stale after 10 s without a
//!   streamed tick).

mod pending;

use super::StrategyModule;
use crate::brokers::common::streaming::{FeedEvent, FeedMode, FeedSubscription, MarketEvent};
use crate::brokers::types::QuoteKey;
use crate::state::AppState;
use async_trait::async_trait;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

pub const STALE_AFTER: Duration = Duration::from_secs(10);
pub const POLL_EVERY: Duration = Duration::from_secs(2);
pub const POLL_BATCH_MAX: usize = 50;

pub type Key = (String, String);

/// Where prices come from. Production is the market-data feed plus broker
/// multiquotes; tests inject ticks directly.
#[async_trait]
pub trait PriceSource: Send + Sync {
    async fn subscribe(&self, keys: &[Key]);
    async fn unsubscribe(&self, keys: &[Key]);
    fn ticks(&self) -> broadcast::Receiver<MarketEvent>;
    /// Last prices over REST, for symbols the stream has gone quiet on.
    async fn poll(&self, keys: &[Key]) -> Vec<(Key, f64)>;
    /// The reader fell behind the ticks and skipped `n` (health counts it
    /// against the feed, NEW-REL-05).
    fn note_skipped(&self, _n: u64) {}
}

#[derive(Default)]
struct Inner {
    refs: HashMap<Key, u32>,
    runs: HashMap<i64, Vec<Key>>,
    last_tick: HashMap<Key, Instant>,
}

/// Refcounted run subscriptions over one price source, and the prices
/// waiting for the evaluator.
pub struct TickFeed {
    source: Option<Arc<dyn PriceSource>>,
    inner: Mutex<Inner>,
    pending: pending::Pending,
}

impl TickFeed {
    pub fn new(source: Option<Arc<dyn PriceSource>>) -> Self {
        Self {
            source,
            inner: Mutex::new(Inner::default()),
            pending: pending::Pending::default(),
        }
    }

    /// What the strategy tick path queued, folded and missed (health).
    pub fn health(&self) -> serde_json::Value {
        self.pending.health()
    }

    /// The count of ticks the reader missed since start, when it grew
    /// since the last call (the health alert raises once per growth).
    pub fn ticks_missed_unreported(&self) -> Option<u64> {
        self.pending.missed_since_alert()
    }

    /// Add this run's instruments; subscribes only those newly referenced.
    pub async fn add_run(&self, run_id: i64, keys: &[Key]) {
        let fresh: Vec<Key> = {
            let mut guard = self.inner.lock();
            let inner = &mut *guard;
            let mine = inner.runs.entry(run_id).or_default();
            let mut new_for_run = Vec::new();
            for k in keys {
                if !mine.contains(k) {
                    mine.push(k.clone());
                    new_for_run.push(k.clone());
                }
            }
            let mut fresh = Vec::new();
            for k in new_for_run {
                let n = inner.refs.entry(k.clone()).or_insert(0);
                *n += 1;
                if *n == 1 {
                    fresh.push(k);
                }
            }
            fresh
        };
        if let (Some(src), false) = (&self.source, fresh.is_empty()) {
            src.subscribe(&fresh).await;
        }
    }

    /// Release this run's instruments; unsubscribes those no run holds.
    pub async fn remove_run(&self, run_id: i64) {
        let gone: Vec<Key> = {
            let mut inner = self.inner.lock();
            let keys = inner.runs.remove(&run_id).unwrap_or_default();
            let mut gone = Vec::new();
            for k in keys {
                if let Some(n) = inner.refs.get_mut(&k) {
                    *n -= 1;
                    if *n == 0 {
                        inner.refs.remove(&k);
                        inner.last_tick.remove(&k);
                        gone.push(k);
                    }
                }
            }
            gone
        };
        if !gone.is_empty() {
            self.pending.retain(|k| !gone.contains(k));
        }
        if let (Some(src), false) = (&self.source, gone.is_empty()) {
            src.unsubscribe(&gone).await;
        }
    }

    /// Release every subscription (shutdown).
    pub async fn release_all(&self) {
        let all: Vec<Key> = {
            let mut inner = self.inner.lock();
            inner.runs.clear();
            inner.last_tick.clear();
            inner.refs.drain().map(|(k, _)| k).collect()
        };
        self.pending.take();
        if let (Some(src), false) = (&self.source, all.is_empty()) {
            src.unsubscribe(&all).await;
        }
    }

    /// Instruments currently subscribed (hygiene tests).
    pub fn subscribed(&self) -> Vec<Key> {
        let mut v: Vec<Key> = self.inner.lock().refs.keys().cloned().collect();
        v.sort();
        v
    }

    pub fn runs_tracked(&self) -> usize {
        self.inner.lock().runs.len()
    }

    /// Queue a streamed price of a held instrument for the evaluator,
    /// recording the instrument as fresh; a price nobody holds is dropped.
    fn queue_tick(&self, key: Key, price: f64) {
        {
            let mut inner = self.inner.lock();
            if !inner.refs.contains_key(&key) {
                return;
            }
            inner.last_tick.insert(key.clone(), Instant::now());
        }
        self.pending.queue(key, price);
    }

    fn stale(&self) -> Vec<Key> {
        let now = Instant::now();
        let inner = self.inner.lock();
        inner
            .refs
            .keys()
            .filter(|k| {
                inner
                    .last_tick
                    .get(*k)
                    .map(|t| now.saturating_duration_since(*t) >= STALE_AFTER)
                    .unwrap_or(true)
            })
            .take(POLL_BATCH_MAX)
            .cloned()
            .collect()
    }
}

/// Start the owned reader and evaluator (a no-op without a price source).
pub fn start(module: &Arc<StrategyModule>) {
    let Some(source) = module.feed.source.clone() else {
        return;
    };
    spawn_reader(module, source.clone());
    spawn_evaluator(module, source);
}

/// Read every tick as it comes and queue the held ones; nothing here
/// waits on an evaluation.
fn spawn_reader(module: &Arc<StrategyModule>, source: Arc<dyn PriceSource>) {
    let mut rx = source.ticks();
    let weak = Arc::downgrade(module);
    let token = module.shutdown_token();
    module.spawn(async move {
        loop {
            let ev = tokio::select! {
                _ = token.cancelled() => break,
                ev = rx.recv() => ev,
            };
            let Some(m) = weak.upgrade() else { break };
            match ev {
                Ok(ev) => {
                    if let FeedEvent::Tick(t) = ev.as_ref() {
                        if t.ltp.is_finite() && t.ltp > 0.0 {
                            m.feed
                                .queue_tick((t.symbol.clone(), t.exchange.clone()), t.ltp);
                        }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    m.feed.pending.note_lagged(n);
                    source.note_skipped(n);
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// Evaluate the waiting prices whenever the reader queues some, and price
/// stale symbols over REST.
fn spawn_evaluator(module: &Arc<StrategyModule>, source: Arc<dyn PriceSource>) {
    let weak = Arc::downgrade(module);
    let token = module.shutdown_token();
    let wake = module.feed.pending.wake.clone();
    module.spawn(async move {
        let mut poll = tokio::time::interval(POLL_EVERY);
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = token.cancelled() => break,
                _ = wake.notified() => {
                    let Some(m) = weak.upgrade() else { break };
                    m.evaluate_waiting().await;
                }
                _ = poll.tick() => {
                    let Some(m) = weak.upgrade() else { break };
                    let stale = m.feed.stale();
                    if stale.is_empty() {
                        continue;
                    }
                    for ((symbol, exchange), ltp) in source.poll(&stale).await {
                        if ltp.is_finite() && ltp > 0.0 {
                            m.process_tick(&symbol, &exchange, ltp).await;
                        }
                    }
                }
            }
        }
    });
}

impl StrategyModule {
    /// Evaluate every price waiting, each symbol's in the order they came,
    /// until none is left.
    async fn evaluate_waiting(&self) {
        loop {
            let waiting = self.feed.pending.take();
            if waiting.is_empty() {
                return;
            }
            for ((symbol, exchange), prices) in waiting {
                for ltp in prices {
                    self.process_tick(&symbol, &exchange, ltp).await;
                }
            }
        }
    }
}

/// The production source: the market-data feed and broker multiquotes.
pub struct FeedPrices {
    ctx: Weak<AppState>,
    feed: Arc<crate::websocket::WebSocketManager>,
    /// Who reads these prices, for the feed's skip counts.
    reader: crate::websocket::Reader,
}

impl FeedPrices {
    pub fn new(
        ctx: Weak<AppState>,
        feed: Arc<crate::websocket::WebSocketManager>,
        reader: crate::websocket::Reader,
    ) -> Self {
        Self { ctx, feed, reader }
    }

    fn subscriptions(&self, keys: &[Key]) -> Vec<FeedSubscription> {
        let Some(ctx) = self.ctx.upgrade() else {
            return vec![];
        };
        keys.iter()
            .filter_map(|(s, e)| {
                let row = ctx.symbols.by_symbol(e, s)?;
                Some(FeedSubscription {
                    symbol: s.clone(),
                    exchange: e.clone(),
                    token: row.token.clone(),
                    brsymbol: row.br_symbol().to_string(),
                    brexchange: row.br_exchange().to_string(),
                    mode: FeedMode::Ltp,
                    depth: 5,
                })
            })
            .collect()
    }
}

#[async_trait]
impl PriceSource for FeedPrices {
    async fn subscribe(&self, keys: &[Key]) {
        let subs = self.subscriptions(keys);
        if !subs.is_empty() {
            if let Err(e) = self.feed.subscribe(subs).await {
                // The REST fallback keeps the run priced.
                tracing::warn!("Strategy prices could not be streamed: {}", e.code());
            }
        }
    }

    async fn unsubscribe(&self, keys: &[Key]) {
        let subs = self.subscriptions(keys);
        if !subs.is_empty() {
            let _ = self.feed.unsubscribe(subs).await;
        }
    }

    fn ticks(&self) -> broadcast::Receiver<MarketEvent> {
        self.feed.subscribe_ticks()
    }

    fn note_skipped(&self, n: u64) {
        self.feed.stats().note_skipped(self.reader, n);
    }

    async fn poll(&self, keys: &[Key]) -> Vec<(Key, f64)> {
        let Some(ctx) = self.ctx.upgrade() else {
            return vec![];
        };
        let Ok(h) = crate::services::core::broker_handle(&ctx) else {
            return vec![];
        };
        drop(ctx);
        let qk: Vec<QuoteKey> = keys
            .iter()
            .map(|(s, e)| QuoteKey::new(e.clone(), s.clone()))
            .collect();
        match h.broker.get_multiquotes(&h.auth, &qk).await {
            Ok(rows) => rows
                .into_iter()
                .filter_map(|r| {
                    let q = r.data?;
                    Some(((r.symbol, r.exchange), q.ltp))
                })
                .collect(),
            Err(e) => {
                tracing::debug!("Strategy price poll failed: {}", e.code());
                vec![]
            }
        }
    }
}
