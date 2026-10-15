//! What proves a leg flat (the position book and the leg's own exit
//! orders, read fail-closed), and the exit attempt: every order sent to
//! flatten a leg, since the first.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What a position book says about one leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegPosition {
    /// A row for the leg with this net quantity (0: positively flat).
    Net(i64),
    /// No row for the leg: an empty or lagging book (right after an entry),
    /// or a broker that drops closed rows. Never read as flat on its own.
    Missing,
    /// A row whose quantity cannot be read.
    Unreadable,
}

/// One leg in a position book body.
pub fn leg_position(book: &Value, symbol: &str, exchange: &str, product: &str) -> LegPosition {
    let rows = book
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for p in rows {
        let field = |k: &str| {
            p.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_ascii_uppercase()
        };
        if p.get("symbol").and_then(Value::as_str) == Some(symbol)
            && field("exchange") == exchange.to_ascii_uppercase()
            && field("product") == product.to_ascii_uppercase()
        {
            let q = match p.get("quantity") {
                Some(Value::Number(n)) => n.as_f64(),
                Some(Value::String(s)) => s.trim().parse::<f64>().ok(),
                _ => None,
            };
            return match q.filter(|q| q.is_finite()) {
                Some(q) => LegPosition::Net(q as i64),
                None => LegPosition::Unreadable,
            };
        }
    }
    LegPosition::Missing
}

/// The net quantity of one leg in a position book body (0 when the book has
/// no readable row; never use it to decide a leg is flat).
pub fn net_quantity(book: &Value, symbol: &str, exchange: &str, product: &str) -> i64 {
    match leg_position(book, symbol, exchange, product) {
        LegPosition::Net(n) => n,
        _ => 0,
    }
}

/// What the position book and a leg's exit orders together prove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flatness {
    /// The leg is flat: it may be cleared.
    Flat,
    /// Open with this signed net quantity.
    Open(i64),
    /// Nothing proves it either way: it stays protected.
    Unknown,
}

/// Whether a leg is flat (the monitor's fail-closed rule):
///
/// * a row at quantity 0 is flat, unless every exit order of the leg ended
///   with nothing filled (then the zero is not the exit's doing: a lagging
///   or stale row is never taken for it);
/// * a missing row is flat only when the leg's own exit orders filled the
///   whole position they were sent for;
/// * an unreadable row is never flat.
pub fn flatness(pos: LegPosition, attempt: Option<&ExitAttempt>) -> Flatness {
    let none_filled =
        attempt.is_some_and(|a| !a.children.is_empty() && !a.working() && a.filled() == 0);
    match pos {
        LegPosition::Net(0) if none_filled => Flatness::Unknown,
        LegPosition::Net(0) => Flatness::Flat,
        LegPosition::Net(n) => Flatness::Open(n),
        LegPosition::Missing => match attempt {
            Some(a) if a.net0 != 0 && !a.working() && a.filled() >= a.net0.abs() => Flatness::Flat,
            _ => Flatness::Unknown,
        },
        LegPosition::Unreadable => Flatness::Unknown,
    }
}

/// Extra position-book reads before a leg the book does not show is left
/// protected and the trader alerted.
pub const POSITION_REREADS: u32 = 2;
/// Spacing of those reads.
pub const POSITION_REREAD_MS: u64 = 300;

/// Where one exit order stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildState {
    /// Acknowledged by the broker, not yet ended.
    Working,
    /// Sent with no definite answer: it may be at the broker (LOG-08).
    Unconfirmed,
    /// Completely filled.
    Filled,
    /// Rejected or cancelled (possibly after a partial fill), or confirmed
    /// never placed.
    Dead,
}

/// One exit order of an attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExitChild {
    pub order_id: Option<String>,
    #[serde(default)]
    pub client_tag: Option<String>,
    /// `BUY` / `SELL`.
    pub action: String,
    pub qty: i64,
    #[serde(default)]
    pub filled: i64,
    pub state: ChildState,
    pub placed_at: DateTime<Utc>,
    /// The leg's signed net position just before this order was sent: a
    /// Live absence is released without the trader only while the broker
    /// still shows exactly this (review P1-2).
    #[serde(default)]
    pub net_before: Option<i64>,
}

impl ExitChild {
    pub fn terminal(&self) -> bool {
        matches!(self.state, ChildState::Filled | ChildState::Dead)
    }

    /// Quantity that may still fill.
    pub fn open_qty(&self) -> i64 {
        if self.terminal() {
            0
        } else {
            (self.qty - self.filled).max(0)
        }
    }
}

/// The exit of one leg: every order sent to flatten it, since the first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExitAttempt {
    pub id: String,
    /// The signed net position when the first exit went out.
    pub net0: i64,
    pub children: Vec<ExitChild>,
    /// When every child had ended.
    #[serde(default)]
    pub settled_at: Option<DateTime<Utc>>,
}

impl ExitAttempt {
    /// Some child may still fill.
    pub fn working(&self) -> bool {
        self.children.iter().any(|c| !c.terminal())
    }

    pub fn filled(&self) -> i64 {
        self.children.iter().map(|c| c.filled.max(0)).sum()
    }

    pub fn open(&self) -> i64 {
        self.children.iter().map(ExitChild::open_qty).sum()
    }

    /// What may still be sent: the position the attempt started from, less
    /// what filled and what is still working.
    pub fn unexited(&self) -> i64 {
        (self.net0.abs() - self.filled() - self.open()).max(0)
    }

    pub(super) fn to_json(&self) -> Option<String> {
        serde_json::to_string(self).ok()
    }
}

/// What one exit attempt came to.
#[derive(Debug, Clone, PartialEq)]
pub enum ExitOutcome {
    /// The leg is flat and nothing of it is working: cleared.
    Flat,
    /// Exit orders went out; the leg stays protected until they end and the
    /// position is flat.
    Placed { orders: usize, unconfirmed: usize },
    /// Earlier exit orders still cover the position (or the position book
    /// has not caught up with their fills): nothing more is sent now.
    Waiting,
    /// Refused: the leg stays managed. Trader-facing reason.
    Refused(String),
    /// The stop was edited or replaced while this exit was being decided:
    /// nothing was sent, the next tick decides on the new stop.
    Stale,
}
