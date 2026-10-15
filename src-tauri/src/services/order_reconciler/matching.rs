//! Matching an unconfirmed placement to the broker's book, and judging an
//! absence (sandbox: confirmed reads release it; Live: held for the trader,
//! P0-3).

use super::{
    Absence, BookSnapshot, HeldOrder, ABSENT_READS, ABSENT_SPAN_SECS, LIVE_RELEASE_SECS,
    TIMESTAMP_SLACK_SECS,
};
use crate::strategy::dispatch::{BookOrder, RunMode};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// An unconfirmed placement, as its owner recorded it.
#[derive(Debug, Clone, Copy, Default)]
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
    /// The owner's net position on this instrument just before the order
    /// was sent (signed). A Live absence is released without the trader
    /// only while the broker still shows exactly this (review P1-2);
    /// `None` never releases.
    pub net_before: Option<i64>,
    /// `MARKET`, `LIMIT`, `SL`, `SL-M` as sent (P1-4). A MARKET order may
    /// show as LIMIT and an SL-M as SL (market price protection).
    pub pricetype: Option<&'a str>,
    /// The limit price, compared for LIMIT and SL orders when both sides
    /// carry one.
    pub price: Option<f64>,
    /// The trigger price, compared for SL and SL-M orders.
    pub trigger_price: Option<f64>,
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

/// Find an unconfirmed placement in one book read (P1-4).
///
/// * When the broker reports the client tag of every order
///   (`tags_reported`, its capability, never inferred from the rows) and
///   the placement carried one, only that tag matches.
/// * Otherwise the order's symbol, exchange, side, quantity, product and
///   shape (price type, limit and trigger prices) must match; the row must
///   not already belong to anyone (`bound`) nor carry another order's tag;
///   and its own timestamp must be readable and no more than
///   [`TIMESTAMP_SLACK_SECS`] before the placement. A row whose time
///   cannot be read is never taken: it could be any earlier order.
pub fn find_placed<'a>(
    book: &'a [BookOrder],
    u: &Unconfirmed<'_>,
    bound: &dyn Fn(&str) -> bool,
    tags_reported: bool,
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
            && super::shape::same_shape(u, r)
    };
    let tag = u.client_tag.map(str::trim).filter(|t| !t.is_empty());
    let count = |hits: Vec<&'a BookOrder>| match hits.len() {
        0 => Found::Absent,
        1 => Found::Order(hits[0]),
        n => Found::Ambiguous(n),
    };
    if let (Some(tag), true) = (tag, tags_reported) {
        return count(
            book.iter()
                .filter(|r| r.client_tag.as_deref().map(str::trim) == Some(tag))
                .filter(fields)
                .collect(),
        );
    }
    let earliest = u
        .placed_at
        .map(|t| t - chrono::Duration::seconds(TIMESTAMP_SLACK_SECS));
    count(
        book.iter()
            .filter(fields)
            .filter(|r| !bound(&r.order_id))
            // Another order's tag is never this one's.
            .filter(|r| {
                match r
                    .client_tag
                    .as_deref()
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                {
                    Some(t) => Some(t) == tag,
                    None => true,
                }
            })
            .filter(|r| match book_time(&r.row) {
                Some(t) => earliest.is_none_or(|e| t >= e),
                None => false,
            })
            .collect(),
    )
}

/// One unconfirmed placement the book has not shown.
struct Miss {
    since: DateTime<Utc>,
    reads: u32,
    /// Live: held for the trader, with its description.
    held: Option<HeldOrder>,
}

/// Counts reads that missed an unconfirmed placement and holds Live ones
/// for the trader. Bounded by the placements still unconfirmed: owners
/// prune it every pass.
#[derive(Default)]
pub struct AbsenceTracker {
    misses: Mutex<HashMap<String, Miss>>,
}

impl AbsenceTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a read without the order; true once its absence is confirmed
    /// ([`ABSENT_READS`] reads over at least [`ABSENT_SPAN_SECS`]). This is
    /// not a release decision: use [`AbsenceTracker::judge`].
    pub fn absent(&self, key: &str, now: DateTime<Utc>) -> bool {
        let mut m = self.misses.lock();
        let e = m.entry(key.to_string()).or_insert(Miss {
            since: now,
            reads: 0,
            held: None,
        });
        e.reads += 1;
        e.reads >= ABSENT_READS && (now - e.since).num_seconds() >= ABSENT_SPAN_SECS
    }

    /// Record a read that missed the unconfirmed placement `u` (owner key
    /// `key`, trader-facing id `id`) and say what its owner may do:
    ///
    /// * Sandbox: [`Absence::NotPlaced`] once the absence is confirmed.
    /// * Live: never on absence alone. [`Absence::Hold`] (alert once), and
    ///   [`Absence::NotPlaced`] only after [`LIVE_RELEASE_SECS`], with no
    ///   working look-alike in the order book, no matching trade in the
    ///   trade book, and the net position still exactly what the owner had
    ///   before sending (`u.net_before`). The trade book and positions are
    ///   read only then, not on every pass.
    ///
    /// The owner forgets the key once it settles the order.
    pub async fn judge(
        &self,
        key: &str,
        id: &str,
        mode: RunMode,
        book: &BookSnapshot,
        u: &Unconfirmed<'_>,
    ) -> Absence {
        let now = book.read_at;
        if !self.absent(key, now) {
            return Absence::Wait;
        }
        if mode == RunMode::Sandbox {
            return Absence::NotPlaced;
        }
        let Some(since) = self.misses.lock().get(key).map(|e| e.since) else {
            return Absence::Wait;
        };
        let long = (now - since).num_seconds() >= LIVE_RELEASE_SECS;
        // A working order of the same instrument and side that was not
        // matched (an unreadable time, a changed price) could be this one.
        // A row another order already owns is accounted for.
        let lookalike = book.rows.iter().any(|r| {
            r.symbol.trim().eq_ignore_ascii_case(u.symbol.trim())
                && r.exchange.trim().eq_ignore_ascii_case(u.exchange.trim())
                && r.action.trim().eq_ignore_ascii_case(u.action.trim())
                && !matches!(
                    r.status.trim(),
                    "complete"
                        | "completed"
                        | "filled"
                        | "rejected"
                        | "cancelled"
                        | "canceled"
                        | "expired"
                )
                && !book.owned(&r.order_id)
        });
        if long && !lookalike && u.net_before.is_some() {
            // Read without holding the lock (a network read), and only now.
            let corr = book.corroboration().await;
            let net_now = corr
                .as_ref()
                .and_then(|c| c.net(u.symbol, u.exchange, u.product));
            let traded = corr.as_ref().map(|c| c.trade_seen(u));
            if traded == Some(false) && net_now.is_some() && net_now == u.net_before {
                tracing::warn!(
                    "Unconfirmed {} {} {} on {}: absent for {} s, no matching trade, position as before sending; released as not placed",
                    u.action,
                    u.quantity,
                    u.symbol,
                    u.exchange,
                    (now - since).num_seconds()
                );
                return Absence::NotPlaced;
            }
        }
        let mut m = self.misses.lock();
        let Some(e) = m.get_mut(key) else {
            return Absence::Wait;
        };
        let alert = e.held.is_none();
        if alert {
            e.held = Some(HeldOrder {
                owner: String::new(),
                id: id.to_string(),
                symbol: u.symbol.to_string(),
                exchange: u.exchange.to_string(),
                action: u.action.to_string(),
                quantity: u.quantity,
                since: e.since,
                message: String::new(),
                product: u.product.map(str::to_string),
                client_tag: u.client_tag.map(str::to_string),
                placed_at: u.placed_at,
            });
        }
        Absence::Hold { alert }
    }

    /// Describe a held order (after the owner raised its alert).
    pub fn describe(&self, key: &str, message: &str) {
        if let Some(h) = self
            .misses
            .lock()
            .get_mut(key)
            .and_then(|e| e.held.as_mut())
        {
            h.message = message.to_string();
        }
    }

    /// The orders held for the trader.
    pub fn held(&self) -> Vec<HeldOrder> {
        let mut v: Vec<HeldOrder> = self
            .misses
            .lock()
            .values()
            .filter_map(|e| e.held.clone())
            .collect();
        v.sort_by(|a, b| a.since.cmp(&b.since).then_with(|| a.id.cmp(&b.id)));
        v
    }

    /// Keep only the keys `keep` accepts (an owner pruning entries whose
    /// orders are gone).
    pub fn retain_by(&self, keep: impl Fn(&str) -> bool) {
        self.misses.lock().retain(|k, _| keep(k));
    }

    /// The owner key of the held order with this trader-facing id.
    pub fn held_key(&self, id: &str) -> Option<String> {
        self.misses
            .lock()
            .iter()
            .find(|(_, e)| e.held.as_ref().is_some_and(|h| h.id == id))
            .map(|(k, _)| k.clone())
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
