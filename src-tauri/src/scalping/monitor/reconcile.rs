//! Following exit orders in the broker's book (the order reconciler's
//! owner side): which orders the monitor owns, folding a book read into
//! each exit attempt, holding a Live exit whose placement is unconfirmed,
//! and clearing a leg once its exits ended and the position is flat.

use super::{
    flatness, fold_child, leg_position, run_mode, ChildState, ExitAttempt, ExitChild, Flatness,
    Gen, RiskMonitor, BOOK_LAG_GRACE_MS,
};
use crate::scalping::store::{SlState, MODE_LIVE};
use crate::services::order_reconciler::{
    held_message, Absence, BookSnapshot, Found, HeldOrder, OrderOwner, Unconfirmed,
};
use crate::strategy::dispatch::{Book, RunMode};
use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::sync::Weak;

impl RiskMonitor {
    /// An attempt the reconciler still follows: an order of it may still
    /// fill, or every order ended a moment ago and the position book may
    /// not show it yet.
    fn followed(&self, a: &ExitAttempt, now: DateTime<Utc>) -> bool {
        a.working()
            || a.settled_at
                .is_some_and(|t| (now - t).num_milliseconds() < BOOK_LAG_GRACE_MS)
    }

    /// Whether a leg of this mode has an exit the reconciler follows.
    pub fn has_working_exits(&self, mode: RunMode) -> bool {
        let now = self.clock.now();
        let inner = self.inner.lock();
        inner.attempts.iter().any(|(k, a)| {
            self.followed(a, now)
                && inner
                    .states
                    .get(k)
                    .is_some_and(|l| run_mode(&l.sl.mode) == mode)
        })
    }

    /// Whether a broker order id is one of this monitor's exit orders.
    pub fn owns_order(&self, order_id: &str) -> bool {
        self.inner.lock().attempts.values().any(|a| {
            a.children
                .iter()
                .any(|c| c.order_id.as_deref() == Some(order_id))
        })
    }

    /// Follow every working exit order of `mode` in one order-book read
    /// (the order reconciler's pass). A leg whose exit orders have all
    /// ended is cleared once the position book shows it flat. Returns how
    /// many unconfirmed orders are still unresolved.
    pub async fn reconcile_book(&self, mode: RunMode, book: &BookSnapshot) -> usize {
        let now = self.clock.now();
        let pending: Vec<(String, Gen, SlState, ExitAttempt)> = {
            let inner = self.inner.lock();
            inner
                .attempts
                .iter()
                .filter(|(_, a)| self.followed(a, now))
                .filter_map(|(k, a)| {
                    let l = inner.states.get(k)?;
                    (run_mode(&l.sl.mode) == mode)
                        .then(|| (k.clone(), l.gen, l.sl.clone(), a.clone()))
                })
                .collect()
        };
        let mut unresolved = 0;
        let mut live = HashSet::new();
        let mut claimed: HashSet<String> = HashSet::new();
        for (key, gen, sl, mut a) in pending {
            if !a.working() {
                // Every order ended: the position book may only now show
                // the leg flat.
                self.settle(&key, gen, &sl, mode).await;
                continue;
            }
            let mut changed = false;
            let mut touched: Vec<usize> = Vec::new();
            for (i, child) in a.children.iter_mut().enumerate() {
                if child.terminal() {
                    continue;
                }
                match (child.state, child.order_id.clone()) {
                    (ChildState::Working, Some(id)) => {
                        let akey = format!("{}#{}#{}", key, a.id, i);
                        if let Some(b) = book.by_id(&id) {
                            self.absence.forget(&akey);
                            let before = child.clone();
                            fold_child(child, b);
                            if *child != before {
                                changed = true;
                                touched.push(i);
                            }
                            continue;
                        }
                        // Accepted, and the book no longer shows it (a day
                        // order past the session, a book that dropped it):
                        // never left working silently (review P1-5).
                        live.insert(akey.clone());
                        let u = exit_unconfirmed(&sl, child);
                        match self.absence.judge(&akey, &akey, mode, book, &u).await {
                            Absence::NotPlaced => {
                                self.absence.forget(&akey);
                                tracing::warn!(
                                    "Scalping exit order {} for {} is no longer in the broker's books; it ended",
                                    id,
                                    sl.symbol
                                );
                                child.state = ChildState::Dead;
                                changed = true;
                                touched.push(i);
                            }
                            Absence::Wait => {}
                            Absence::Hold { alert } => {
                                unresolved += 1;
                                if alert {
                                    let msg = format!(
                                        "Scalping stop for {}: exit order {} ({} {}) was accepted, but the broker's order book no longer shows it. It is not sent again. Check the order book and trade book; if it is not there, mark it as not placed on the Health page to resume protection.",
                                        sl.symbol, id, child.action, child.qty
                                    );
                                    self.absence.describe(&akey, &msg);
                                    book.alert_held(msg);
                                }
                            }
                        }
                    }
                    _ => {
                        let akey = format!("{}#{}#{}", key, a.id, i);
                        live.insert(akey.clone());
                        let u = exit_unconfirmed(&sl, child);
                        let bound = |id: &str| claimed.contains(id) || book.owned(id);
                        match book.find(&u, &bound) {
                            Found::Order(b) => {
                                self.absence.forget(&akey);
                                claimed.insert(b.order_id.clone());
                                child.order_id = Some(b.order_id.clone());
                                fold_child(child, b);
                                changed = true;
                                touched.push(i);
                            }
                            Found::Absent => {
                                match self.absence.judge(&akey, &akey, mode, book, &u).await {
                                    Absence::NotPlaced => {
                                        self.absence.forget(&akey);
                                        tracing::warn!(
                                            "Unconfirmed scalping exit for {} is not in the broker's books after repeated checks: it was not placed",
                                            sl.symbol
                                        );
                                        child.state = ChildState::Dead;
                                        changed = true;
                                        touched.push(i);
                                    }
                                    Absence::Wait => unresolved += 1,
                                    Absence::Hold { alert } => {
                                        // Live: never on absence alone
                                        // (P0-3). The leg stays protected and
                                        // nothing more is sent for it.
                                        unresolved += 1;
                                        if alert {
                                            let msg = held_message(
                                                &format!("Scalping exit for {}", sl.symbol),
                                                &u,
                                            );
                                            self.absence.describe(&akey, &msg);
                                            book.alert_held(msg);
                                        }
                                    }
                                }
                            }
                            Found::Ambiguous(n) => {
                                unresolved += 1;
                                self.alert_once(
                                    &key,
                                    "ambiguous",
                                    format!(
                                        "Scalping stop for {}: {} orders in the broker's order book match an exit order the broker did not confirm, and nothing tells them apart. It is not sent again and the stop stays; check the order book at the broker.",
                                        sl.symbol, n
                                    ),
                                );
                            }
                        }
                    }
                }
            }
            if !changed {
                continue;
            }
            // Only for the stop these orders belong to.
            if self.inner.lock().states.get(&key).map(|l| l.gen.0) != Some(gen.0) {
                continue;
            }
            // Fold this pass's facts into the attempt as it stands now: an
            // exit may have added orders since the snapshot (they are kept).
            let (now_attempt, _) = self.update_attempt(&key, sl.id, |slot| {
                if let Some(c) = slot.as_mut().filter(|c| c.id == a.id) {
                    // Only the orders this pass learned about: an exit may
                    // have recorded answers on others meanwhile.
                    for &i in &touched {
                        if let (Some(cur), Some(folded)) =
                            (c.children.get_mut(i), a.children.get(i))
                        {
                            *cur = folded.clone();
                        }
                    }
                    if !c.working() && c.settled_at.is_none() {
                        c.settled_at = Some(book.read_at);
                    }
                }
            });
            if now_attempt.is_some_and(|c| c.id == a.id && !c.working()) {
                self.settle(&key, gen, &sl, mode).await;
            }
        }
        // This destination's keys only: the other's pass keeps its own.
        let scope = format!(
            "{}:",
            if mode == RunMode::Live {
                MODE_LIVE
            } else {
                crate::scalping::store::MODE_ANALYZE
            }
        );
        self.absence.retain_scope(&scope, &live);
        unresolved
    }

    /// Live exit orders held for the trader (P0-3).
    pub fn held_orders(&self) -> Vec<HeldOrder> {
        self.absence.held()
    }

    /// The trader checked the broker's books: the held exit order `id` was
    /// not placed. It ends as never placed; the leg stays protected and its
    /// next breach sends what is not covered.
    pub async fn confirm_not_placed(&self, id: &str) -> Result<bool, String> {
        if self.absence.held_key(id).is_none() {
            return Ok(false);
        }
        let mut parts = id.rsplitn(3, '#');
        let (Some(idx), Some(attempt_id), Some(key)) = (
            parts.next().and_then(|i| i.parse::<usize>().ok()),
            parts.next(),
            parts.next(),
        ) else {
            return Ok(false);
        };
        let now = self.clock.now();
        self.absence.forget(id);
        let leg = {
            let inner = self.inner.lock();
            inner.states.get(key).map(|l| (l.gen, l.sl.clone()))
        };
        let Some((gen, sl)) = leg else {
            return Ok(false);
        };
        let mut hit = false;
        let (now_attempt, _) = self.update_attempt(key, sl.id, |slot| {
            if let Some(a) = slot.as_mut().filter(|a| a.id == attempt_id) {
                if let Some(c) = a.children.get_mut(idx).filter(|c| !c.terminal()) {
                    c.state = ChildState::Dead;
                    hit = true;
                    if !a.working() {
                        a.settled_at = Some(now);
                    }
                }
            }
        });
        if !hit {
            return Ok(false);
        }
        tracing::warn!(
            "Trader marked an unconfirmed scalping exit for {} as not placed",
            sl.symbol
        );
        if now_attempt.is_some_and(|a| !a.working()) {
            self.settle(key, gen, &sl, run_mode(&sl.mode)).await;
        }
        Ok(true)
    }

    /// Every exit order of a leg has ended: clear it if the position book
    /// shows it flat; otherwise it stays protected, and its next breach
    /// sends only what was not filled.
    async fn settle(&self, key: &str, gen: Gen, sl: &SlState, mode: RunMode) {
        let Ok(book) = self.gateway.book(mode, Book::Positions).await else {
            return;
        };
        let attempt = self.inner.lock().attempts.get(key).cloned();
        // Flat only on proof (a zero row the exits made, or exits that
        // filled the whole position): a missing row is not flat.
        if flatness(
            leg_position(&book, &sl.symbol, &sl.exchange, &sl.product),
            attempt.as_ref(),
        ) != Flatness::Flat
        {
            return;
        }
        // The installed stop may have been edited meanwhile (same row, new
        // version): clear that one, the position it protects is flat.
        let current = self.inner.lock().states.get(key).map(|l| l.gen);
        if let Some(g) = current.filter(|g| g.0 == gen.0) {
            if self.clear(key, g, sl).await {
                tracing::info!(
                    "Scalping exit for {} filled and the position is flat; cleared",
                    sl.symbol
                );
            }
        }
    }
}

/// An exit order as the reconciler looks it up: exits are MARKET orders on
/// the leg's symbol, exchange and product.
fn exit_unconfirmed<'a>(sl: &'a SlState, child: &'a ExitChild) -> Unconfirmed<'a> {
    Unconfirmed {
        symbol: &sl.symbol,
        exchange: &sl.exchange,
        action: &child.action,
        quantity: child.qty,
        product: Some(&sl.product),
        client_tag: child.client_tag.as_deref(),
        placed_at: Some(child.placed_at),
        net_before: child.net_before,
        pricetype: Some("MARKET"),
        price: None,
        trigger_price: None,
    }
}

/// The scalping monitor as an owner of orders (its exit orders).
pub struct ScalpingOrders(pub Weak<RiskMonitor>);

#[async_trait::async_trait]
impl OrderOwner for ScalpingOrders {
    fn name(&self) -> &'static str {
        "scalping"
    }

    fn has_open_orders(&self, mode: RunMode) -> bool {
        self.0.upgrade().is_some_and(|m| m.has_working_exits(mode))
    }

    fn owns_order(&self, order_id: &str) -> bool {
        self.0.upgrade().is_some_and(|m| m.owns_order(order_id))
    }

    async fn reconcile(&self, mode: RunMode, book: &BookSnapshot) -> usize {
        match self.0.upgrade() {
            Some(m) => m.reconcile_book(mode, book).await,
            None => 0,
        }
    }

    fn held_orders(&self) -> Vec<HeldOrder> {
        self.0
            .upgrade()
            .map(|m| m.held_orders())
            .unwrap_or_default()
    }

    async fn confirm_not_placed(&self, id: &str) -> Result<bool, String> {
        match self.0.upgrade() {
            Some(m) => m.confirm_not_placed(id).await,
            None => Ok(false),
        }
    }
}
