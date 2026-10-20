//! OpenScript orders against one order-book read: the order reconciler's
//! runner owner (LOG-08).
//!
//! An unconfirmed row is a placement the destination neither accepted nor
//! refused: the order may be there. Its run sends nothing new and a Stop
//! does not go ahead while it is (see `Runner::page_intents` and
//! `Runner::settle`). Here it is looked up in the book ([`BookSnapshot::find`]:
//! by the client tag it was sent with where the broker reports tags, else by
//! its fields and shape among the orders nobody owns):
//!
//! * **Found**, the row is bound to that order and the run's own polling
//!   follows it from there; its page is told.
//! * **Absent**, [`AbsenceTracker::judge`] decides. In Sandbox a confirmed
//!   absence means it was not placed: the row is rejected and the page told,
//!   so the run trades again. In Live absence alone never releases it (a
//!   broker's book can lag or drop an order, and a wrong release lets a
//!   second one out): it is held, the trader is alerted once on the Health
//!   page, and it is released when the trader marks it not placed there
//!   ([`Runner::confirm_not_placed`]). The runner does not read the broker's
//!   position before each order, so it gives no `net_before` and a Live row
//!   is never released without the trader.
//! * **Several candidates** are never guessed between; the trader is told.
//!
//! Every per-row map is pruned to the rows still unconfirmed.
//!
//! **Only rows of the session the book was read through** (NEW-TR-01): a
//! Live row placed through another broker or account is neither searched
//! for, judged absent nor bound here; it waits for its own session. A Live
//! row from before owners were recorded is the exception: the broker switch
//! guard kept it on the connected broker, and a book that finds it adopts
//! it. A frame reaches a page only for its own run's row (NEW-TR-03).
//!
//! [`AbsenceTracker::judge`]: crate::services::order_reconciler::AbsenceTracker::judge

use super::{orders, store, RunEntry, Runner};
use crate::services::live_owner::{Destination, LiveOwner};
use crate::services::order_reconciler::{
    held_message, Absence, BookSnapshot, Found, HeldOrder, OrderOwner, Unconfirmed,
};
use crate::strategy::dispatch::{BookOrder, RunMode};
use std::collections::HashSet;
use std::sync::{Arc, Weak};

/// What a sandbox row reads once the book has settled it as never placed.
pub const NOT_PLACED: &str =
    "The sandbox has no record of this order after repeated checks, so it was not placed";

/// What a live row reads once the order book has settled it as never placed.
pub const NOT_PLACED_LIVE: &str =
    "Your broker has no record of this order after repeated checks, so it was not placed";

/// What a row reads once the trader has marked it as not placed.
pub const MARKED_NOT_PLACED: &str =
    "You marked this order as not placed after checking the broker's order book";

/// When the row was written, as the matcher reads it.
fn placed_at(o: &store::OrderRow) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(&o.placed_at)
        .ok()
        .map(|t| t.with_timezone(&chrono::Utc))
}

/// An unconfirmed row as the order book is searched for it.
fn unconfirmed_of(o: &store::OrderRow) -> Unconfirmed<'_> {
    Unconfirmed {
        symbol: &o.symbol,
        exchange: &o.exchange,
        action: &o.action,
        quantity: o.quantity,
        product: Some(o.product.as_str()),
        client_tag: o.client_tag.as_deref(),
        placed_at: placed_at(o),
        // Not known: a Live absence is released by the trader only.
        net_before: None,
        pricetype: Some(o.pricetype.as_str()),
        price: o.price,
        trigger_price: o.trigger_price,
    }
}

/// Whether a book read through `session` (Live) may settle row `o`: a row
/// of that session, or a Live row whose owner was never recorded.
fn settles_here(mode: RunMode, session: Option<&LiveOwner>, o: &store::OrderRow) -> bool {
    let Some(at) = o.destination() else {
        return false;
    };
    match (mode, session) {
        (RunMode::Sandbox, _) => at.mode == RunMode::Sandbox,
        // A row from before owners were recorded may be adopted by the book
        // that finds it (`adopt_found`); nothing else of another session.
        (RunMode::Live, Some(owner)) => {
            let here = Destination::live(owner.clone());
            here.owns(&at) || (here.is_known() && at.mode == RunMode::Live && o.broker.is_empty())
        }
        (RunMode::Live, None) => false,
    }
}

impl Runner {
    /// The unconfirmed rows of one side that a book read through `session`
    /// (Live) may settle.
    fn unconfirmed_here(
        &self,
        mode: RunMode,
        session: Option<&LiveOwner>,
    ) -> crate::error::Result<Vec<store::OrderRow>> {
        let conn = self.db.conn()?;
        let rows = store::unconfirmed_rows(&conn, mode.as_str())?;
        Ok(rows
            .into_iter()
            .filter(|o| settles_here(mode, session, o))
            .collect())
    }

    /// Whether one side has an unconfirmed row the session connected now
    /// may settle.
    pub fn has_unconfirmed(&self, mode: RunMode) -> bool {
        let session = match mode {
            RunMode::Live => self.services().live_owner(),
            RunMode::Sandbox => None,
        };
        match self.unconfirmed_here(mode, session.as_ref()) {
            Ok(rows) => !rows.is_empty(),
            Err(e) => {
                // Unknown: read the book rather than leave a row unsettled.
                tracing::warn!("Could not check for unconfirmed OpenScript orders: {}", e);
                true
            }
        }
    }

    /// Whether a destination order id already belongs to one of the
    /// runner's rows. Unknown counts as owned, so no other owner binds it.
    pub fn owns_order(&self, orderid: &str) -> bool {
        self.db
            .conn()
            .and_then(|c| store::owns_orderid(&c, orderid))
            .unwrap_or(true)
    }

    fn running_entry(&self, deployment: &str, mode: &str) -> Option<Arc<RunEntry>> {
        self.reg
            .lock()
            .runs
            .get(deployment)
            .filter(|e| e.info.mode.as_str() == mode)
            .cloned()
    }

    /// The run a row belongs to, as the trader knows it.
    fn run_name(&self, o: &store::OrderRow) -> String {
        match self.running_entry(&o.deployment, &o.mode) {
            Some(e) => format!("OpenScript {} ({})", e.info.script, o.deployment),
            None => format!("OpenScript {}", o.deployment),
        }
    }

    /// Fold one book read for every unconfirmed row of `mode`. Returns how
    /// many stay unresolved.
    pub async fn reconcile_book(&self, mode: RunMode, book: &BookSnapshot) -> usize {
        // Judged against the session the book was read through, not the one
        // connected by the time it is folded.
        let rows = match self.unconfirmed_here(mode, book.session.as_ref()) {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!(
                    "Unconfirmed OpenScript orders could not be read to reconcile: {}",
                    e
                );
                return 0;
            }
        };
        let scope = format!("{}:", mode.as_str());
        let mut live: HashSet<String> = HashSet::new();
        let mut claimed: HashSet<String> = HashSet::new();
        let mut unresolved = 0;
        for o in &rows {
            let key = format!("{}{}", scope, o.id);
            live.insert(key.clone());
            let u = unconfirmed_of(o);
            let found = {
                let bound = |id: &str| claimed.contains(id) || book.owned(id);
                book.find(&u, &bound)
            };
            match found {
                Found::Order(b) => {
                    self.absence.forget(&key);
                    self.ambiguous.lock().remove(&key);
                    claimed.insert(b.order_id.clone());
                    self.bind_unconfirmed(book.session.as_ref(), o, b);
                }
                Found::Absent => {
                    let id = o.id.to_string();
                    match self.absence.judge(&key, &id, mode, book, &u).await {
                        Absence::Wait => unresolved += 1,
                        Absence::NotPlaced => {
                            self.absence.forget(&key);
                            let why = if mode == RunMode::Sandbox {
                                NOT_PLACED
                            } else {
                                NOT_PLACED_LIVE
                            };
                            self.settle_not_placed(o, why);
                        }
                        Absence::Hold { alert } => {
                            // Live: never released on absence alone (P0-3).
                            unresolved += 1;
                            if alert {
                                let msg = held_message(&self.run_name(o), &u);
                                self.absence.describe(&key, &msg);
                                book.alert_held(msg.clone());
                                if let Some(e) = self.running_entry(&o.deployment, &o.mode) {
                                    self.write_log(&e.info, &[msg]);
                                }
                            }
                        }
                    }
                }
                Found::Ambiguous(n) => {
                    unresolved += 1;
                    if self.ambiguous.lock().insert(key.clone()) {
                        self.tell(
                            o,
                            &format!(
                                "The unconfirmed {} {} {} of this strategy matches {} orders in the order book and nothing tells them apart, so none is taken for it. Check your order book. Until it is settled this strategy sends nothing new and Stop does not go ahead.",
                                o.action, o.quantity, o.symbol, n
                            ),
                        );
                    }
                }
            }
        }
        // Bounded by the rows still unconfirmed on this side.
        self.absence.retain_scope(&scope, &live);
        self.ambiguous
            .lock()
            .retain(|k| !k.starts_with(&scope) || live.contains(k));
        unresolved
    }

    fn bind_unconfirmed(&self, session: Option<&LiveOwner>, o: &store::OrderRow, b: &BookOrder) {
        let now = self.clock.now();
        let conn = match self.db.conn() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Could not bind OpenScript order row {}: {}", o.id, e);
                return;
            }
        };
        match store::bind(&conn, o.id, &b.order_id, now) {
            Ok(true) => self.adopt_found(&conn, session, o),
            Ok(false) => return,
            Err(e) => {
                tracing::error!(
                    "OpenScript order row {} could not be bound to order {}: {}",
                    o.id,
                    b.order_id,
                    e
                );
                return;
            }
        }
        let progress = orders::progress(&b.row, o.quantity);
        if let Some(p) = &progress {
            if let Err(e) = store::progressed(&conn, o.id, p.status, p.filled, p.average_price, now)
            {
                tracing::warn!("Could not record the fill of order {}: {}", b.order_id, e);
            }
        }
        drop(conn);
        let line = format!(
            "The unconfirmed {} {} {} was found in the order book as order {}; it is followed from here.",
            o.action, o.quantity, o.symbol, b.order_id
        );
        let Some(e) = self.running_entry(&o.deployment, &o.mode) else {
            tracing::info!("{} ({})", line, o.deployment);
            return;
        };
        self.write_log(&e.info, &[line]);
        if let Some(intent) = o.intent_id {
            let (status, filled, average) = progress
                .map(|p| (p.status, p.filled, p.average_price))
                .unwrap_or(("working", 0, None));
            let frame = orders::frame(
                intent,
                status,
                filled,
                average,
                Some(&b.order_id),
                None,
                now.timestamp_millis(),
            );
            self.frame_to_page(&e, o, frame);
        }
    }

    /// A Live row whose owner was never recorded, found in the book read
    /// through `session`: it was that session's, and is recorded so.
    fn adopt_found(
        &self,
        conn: &rusqlite::Connection,
        session: Option<&LiveOwner>,
        o: &store::OrderRow,
    ) {
        let Some(owner) = session.filter(|_| o.mode == "live" && o.broker.is_empty()) else {
            return;
        };
        if let Err(e) = store::adopt(conn, o.id, owner, self.clock.now()) {
            tracing::warn!(
                "Could not record the owner of OpenScript order row {}: {}",
                o.id,
                e
            );
        }
    }

    /// Settle a row as never placed and tell its page. False when it could
    /// not be saved, or was no longer unconfirmed.
    fn settle_not_placed(&self, o: &store::OrderRow, why: &str) -> bool {
        let now = self.clock.now();
        match self
            .db
            .conn()
            .and_then(|c| store::release_unconfirmed(&c, o.id, why, now))
        {
            Ok(true) => {}
            Ok(false) => return false,
            Err(e) => {
                tracing::error!("OpenScript order row {} could not be settled: {}", o.id, e);
                return false;
            }
        }
        self.tell(
            o,
            &format!(
                "The unconfirmed {} {} {} was not placed: {}. {}",
                o.action,
                o.quantity,
                o.symbol,
                why.trim_end_matches('.'),
                if o.is_exit {
                    "Press Stop again to close what this strategy holds."
                } else {
                    "This strategy can send orders again."
                }
            ),
        );
        if let Some(e) = self.running_entry(&o.deployment, &o.mode) {
            if let Some(intent) = o.intent_id {
                let frame = orders::frame(
                    intent,
                    "rejected",
                    0,
                    None,
                    None,
                    Some(why),
                    now.timestamp_millis(),
                );
                self.frame_to_page(&e, o, frame);
            }
        }
        true
    }

    /// Write a line to the run's log, or the app log when it is not running.
    fn tell(&self, o: &store::OrderRow, line: &str) {
        match self.running_entry(&o.deployment, &o.mode) {
            Some(e) => self.write_log(&e.info, &[line.to_string()]),
            None => tracing::warn!("OpenScript {}: {}", o.deployment, line),
        }
    }

    /// Live orders held for the trader (P0-3).
    pub fn held_orders(&self) -> Vec<HeldOrder> {
        self.absence.held()
    }

    /// The trader checked the broker's books: the held order `id` (the
    /// order row) was not placed. Its run trades again and Stop goes ahead.
    /// Run under the reconciler's pass lock, after the reconciler read the
    /// order book and trade book once more.
    pub async fn confirm_not_placed(&self, id: &str) -> Result<bool, String> {
        let Some(key) = self.absence.held_key(id) else {
            return Ok(false);
        };
        let Ok(row_id) = id.parse::<i64>() else {
            return Ok(false);
        };
        let row = match self.db.conn().and_then(|c| store::order_by_id(&c, row_id)) {
            Ok(row) => row,
            Err(e) => {
                tracing::error!("Held OpenScript order {} could not be read: {}", id, e);
                return Err("The order could not be read. Try again.".into());
            }
        };
        let Some(o) = row.filter(|o| o.status == store::UNCONFIRMED && o.orderid.is_none()) else {
            // Found or settled since the page was loaded.
            self.absence.forget(&key);
            return Ok(false);
        };
        let settled = self.settle_not_placed(&o, MARKED_NOT_PLACED);
        if !settled {
            return match self.db.conn().and_then(|c| store::order_by_id(&c, row_id)) {
                Ok(Some(now)) if now.status == store::UNCONFIRMED => Err(
                    "The order could not be marked just now. Nothing was changed. Try again."
                        .into(),
                ),
                _ => {
                    self.absence.forget(&key);
                    Ok(false)
                }
            };
        }
        self.absence.forget(&key);
        Ok(true)
    }
}

/// The OpenScript runner as an owner of orders.
pub struct RunnerOrders(pub Weak<Runner>);

#[async_trait::async_trait]
impl OrderOwner for RunnerOrders {
    fn name(&self) -> &'static str {
        "openscript"
    }

    fn has_open_orders(&self, mode: RunMode) -> bool {
        self.0.upgrade().is_some_and(|r| r.has_unconfirmed(mode))
    }

    fn owns_order(&self, order_id: &str) -> bool {
        self.0.upgrade().is_some_and(|r| r.owns_order(order_id))
    }

    async fn reconcile(&self, mode: RunMode, book: &BookSnapshot) -> usize {
        match self.0.upgrade() {
            Some(r) => r.reconcile_book(mode, book).await,
            None => 0,
        }
    }

    fn held_orders(&self) -> Vec<HeldOrder> {
        self.0
            .upgrade()
            .map(|r| r.held_orders())
            .unwrap_or_default()
    }

    async fn confirm_not_placed(&self, id: &str) -> Result<bool, String> {
        match self.0.upgrade() {
            Some(r) => r.confirm_not_placed(id).await,
            None => Ok(false),
        }
    }
}
