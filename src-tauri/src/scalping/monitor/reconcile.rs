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

/// One leg as a reconcile pass follows its exit orders.
struct LegPass<'a> {
    key: &'a str,
    sl: &'a SlState,
    mode: RunMode,
    book: &'a BookSnapshot,
}

/// What following one exit order found: whether it changed, and whether it
/// is still unresolved.
#[derive(Default)]
struct Followed {
    changed: bool,
    unresolved: usize,
}

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

    /// Which of these broker order ids are this monitor's exit orders.
    pub fn owned_among(&self, order_ids: &[String]) -> HashSet<String> {
        let inner = self.inner.lock();
        let mine: HashSet<&str> = inner
            .attempts
            .values()
            .flat_map(|a| a.children.iter().filter_map(|c| c.order_id.as_deref()))
            .collect();
        order_ids
            .iter()
            .filter(|id| mine.contains(id.as_str()))
            .cloned()
            .collect()
    }

    /// An accepted exit order: folded from the book or, once the book no
    /// longer shows it (a day order past the session, a book that dropped
    /// it), judged absent; never left working silently (review P1-5).
    async fn follow_accepted(
        &self,
        p: &LegPass<'_>,
        akey: &str,
        id: &str,
        child: &mut ExitChild,
        live: &mut HashSet<String>,
    ) -> Followed {
        let mut f = Followed::default();
        if let Some(b) = p.book.by_id(id) {
            self.absence.forget(akey);
            let before = child.clone();
            fold_child(child, b);
            f.changed = *child != before;
            return f;
        }
        live.insert(akey.to_string());
        let u = exit_unconfirmed(p.sl, child);
        match self.absence.judge(akey, akey, p.mode, p.book, &u).await {
            Absence::NotPlaced => {
                self.absence.forget(akey);
                tracing::warn!(
                    "Scalping exit order {} for {} is no longer in the broker's books; it ended",
                    id,
                    p.sl.symbol
                );
                child.state = ChildState::Dead;
                f.changed = true;
            }
            Absence::Wait => {}
            Absence::Hold { alert } => {
                f.unresolved += 1;
                if alert {
                    let msg = format!(
                        "Scalping stop for {}: exit order {} ({} {}) was accepted, but the broker's order book no longer shows it. It is not sent again. Check the order book and trade book; if it is not there, mark it as not placed on the Health page to resume protection.",
                        p.sl.symbol, id, child.action, child.qty
                    );
                    self.absence.describe(akey, &msg);
                    p.book.alert_held(msg);
                }
            }
        }
        f
    }

    /// An exit order the broker did not confirm: matched to one order in
    /// the book, judged absent, or held when several orders match it.
    async fn find_unconfirmed(
        &self,
        p: &LegPass<'_>,
        akey: &str,
        child: &mut ExitChild,
        live: &mut HashSet<String>,
    ) -> Followed {
        let mut f = Followed::default();
        live.insert(akey.to_string());
        let u = exit_unconfirmed(p.sl, child);
        match p.book.find(&u, &|id: &str| p.book.owned(id)) {
            Found::Order(b) => {
                self.absence.forget(akey);
                p.book.claim(&b.order_id);
                child.order_id = Some(b.order_id.clone());
                fold_child(child, b);
                f.changed = true;
            }
            Found::Absent => {
                match self.absence.judge(akey, akey, p.mode, p.book, &u).await {
                    Absence::NotPlaced => {
                        self.absence.forget(akey);
                        tracing::warn!(
                            "Unconfirmed scalping exit for {} is not in the broker's books after repeated checks: it was not placed",
                            p.sl.symbol
                        );
                        child.state = ChildState::Dead;
                        f.changed = true;
                    }
                    Absence::Wait => f.unresolved += 1,
                    Absence::Hold { alert } => {
                        // Live: never on absence alone (P0-3). The leg
                        // stays protected and nothing more is sent for it.
                        f.unresolved += 1;
                        if alert {
                            let msg =
                                held_message(&format!("Scalping exit for {}", p.sl.symbol), &u);
                            self.absence.describe(akey, &msg);
                            p.book.alert_held(msg);
                        }
                    }
                }
            }
            Found::Ambiguous(n) => {
                f.unresolved += 1;
                self.alert_once(
                    p.key,
                    "ambiguous",
                    format!(
                        "Scalping stop for {}: {} orders in the broker's order book match an exit order the broker did not confirm, and nothing tells them apart. It is not sent again and the stop stays; check the order book at the broker.",
                        p.sl.symbol, n
                    ),
                );
            }
        }
        f
    }

    /// Applies what this pass learned about a leg's exit orders to its
    /// attempt, and settles the leg once every order of it has ended.
    async fn apply_pass(&self, p: &LegPass<'_>, gen: Gen, a: &ExitAttempt, touched: &[usize]) {
        // Only for the stop these orders belong to.
        if self.inner.lock().states.get(p.key).map(|l| l.gen.0) != Some(gen.0) {
            return;
        }
        // Fold this pass's facts into the attempt as it stands now: an
        // exit may have added orders since the snapshot (they are kept).
        let (now_attempt, _) = self
            .update_attempt(p.key, p.sl.id, |slot| {
                if let Some(c) = slot.as_mut().filter(|c| c.id == a.id) {
                    // Only the orders this pass learned about: an exit may
                    // have recorded answers on others meanwhile.
                    for &i in touched {
                        if let (Some(cur), Some(folded)) =
                            (c.children.get_mut(i), a.children.get(i))
                        {
                            *cur = folded.clone();
                        }
                    }
                    if !c.working() && c.settled_at.is_none() {
                        c.settled_at = Some(p.book.read_at);
                    }
                }
            })
            .await;
        if now_attempt.is_some_and(|c| c.id == a.id && !c.working()) {
            self.settle(p.key, gen, p.sl, p.mode).await;
        }
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
        for (key, gen, sl, mut a) in pending {
            if !a.working() {
                // Every order ended: the position book may only now show
                // the leg flat.
                self.settle(&key, gen, &sl, mode).await;
                continue;
            }
            let p = LegPass {
                key: &key,
                sl: &sl,
                mode,
                book,
            };
            let mut touched: Vec<usize> = Vec::new();
            for (i, child) in a.children.iter_mut().enumerate() {
                if child.terminal() {
                    continue;
                }
                let akey = format!("{}#{}#{}", key, a.id, i);
                let f = match (child.state, child.order_id.clone()) {
                    (ChildState::Working, Some(id)) => {
                        self.follow_accepted(&p, &akey, &id, child, &mut live).await
                    }
                    _ => self.find_unconfirmed(&p, &akey, child, &mut live).await,
                };
                unresolved += f.unresolved;
                if f.changed {
                    touched.push(i);
                }
            }
            if !touched.is_empty() {
                self.apply_pass(&p, gen, &a, &touched).await;
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
        let (now_attempt, _) = self
            .update_attempt(key, sl.id, |slot| {
                if let Some(a) = slot.as_mut().filter(|a| a.id == attempt_id) {
                    if let Some(c) = a.children.get_mut(idx).filter(|c| !c.terminal()) {
                        c.state = ChildState::Dead;
                        hit = true;
                        if !a.working() {
                            a.settled_at = Some(now);
                        }
                    }
                }
            })
            .await;
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

    fn owned_among(&self, order_ids: &[String]) -> HashSet<String> {
        self.0
            .upgrade()
            .map(|m| m.owned_among(order_ids))
            .unwrap_or_default()
    }

    async fn reconcile(&self, mode: RunMode, book: &BookSnapshot) -> crate::error::Result<usize> {
        Ok(match self.0.upgrade() {
            Some(m) => m.reconcile_book(mode, book).await,
            None => 0,
        })
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
