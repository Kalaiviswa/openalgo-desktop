//! What a broker feed's readers missed (NEW-REL-05). Each reader of the
//! feed's event channel (`TICK_CHANNEL_CAP` events) that falls behind skips
//! ahead (`RecvError::Lagged`) and says how many it skipped; the feed keeps
//! the count per reader, and the counts since its current connection began,
//! for health and Run diagnostics. Before this the skips were log lines
//! only, most of them at debug level.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// A reader of a broker feed's events that can fall behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reader {
    /// The strategy module's price reader (it also folds what it holds).
    Strategy,
    /// The scalping risk monitor.
    Scalping,
    /// The sandbox's tick forwarder (it polls quotes after a skip).
    Sandbox,
    /// The relay to the market data feed for apps (port 8765).
    FeedClients,
    /// The order update relay to the event bus (a skip there may have lost
    /// an order fact; the reconciler repairs it from the order book).
    OrderRelay,
}

impl Reader {
    pub const ALL: [Reader; 5] = [
        Reader::Strategy,
        Reader::Scalping,
        Reader::Sandbox,
        Reader::FeedClients,
        Reader::OrderRelay,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Reader::Strategy => "strategy",
            Reader::Scalping => "scalping",
            Reader::Sandbox => "sandbox",
            Reader::FeedClients => "feed_clients",
            Reader::OrderRelay => "order_relay",
        }
    }
}

/// A feed's counts since its current (or last) connection began.
#[derive(Debug, Clone, PartialEq)]
pub struct SinceConnect {
    /// How long ago the connection began; `None` before the first.
    pub connected_for: Option<Duration>,
    /// Ticks, depth and order updates received.
    pub received: u64,
    /// What each reader skipped, every reader in `Reader::ALL` order.
    pub skipped: Vec<(Reader, u64)>,
}

impl SinceConnect {
    pub fn skipped_total(&self) -> u64 {
        self.skipped.iter().map(|(_, n)| n).sum()
    }
}

/// The counters behind `SinceConnect`. Times are milliseconds after the
/// feed's epoch, plus one (0: never).
#[derive(Debug, Default)]
pub(super) struct Losses {
    skipped: [AtomicU64; 5],
    skipped_at_connect: [AtomicU64; 5],
    received_at_connect: AtomicU64,
    connected_at: AtomicU64,
}

fn slot(r: Reader) -> usize {
    Reader::ALL.iter().position(|x| *x == r).unwrap_or(0)
}

impl Losses {
    pub(super) fn note(&self, reader: Reader, n: u64) {
        self.skipped[slot(reader)].fetch_add(n, Ordering::Relaxed);
    }

    /// A connection began: what has been counted so far is before it.
    pub(super) fn connected(&self, received: u64, now_ms: u64) {
        for (base, now) in self.skipped_at_connect.iter().zip(&self.skipped) {
            base.store(now.load(Ordering::Relaxed), Ordering::Relaxed);
        }
        self.received_at_connect.store(received, Ordering::Relaxed);
        self.connected_at
            .store(now_ms.saturating_add(1), Ordering::Relaxed);
    }

    pub(super) fn since_connect(&self, received: u64, now_ms: u64) -> SinceConnect {
        let connected_for = self
            .connected_at
            .load(Ordering::Relaxed)
            .checked_sub(1)
            .map(|at| Duration::from_millis(now_ms.saturating_sub(at)));
        let skipped = Reader::ALL
            .iter()
            .map(|r| {
                let i = slot(*r);
                let n = self.skipped[i].load(Ordering::Relaxed);
                (
                    *r,
                    n.saturating_sub(self.skipped_at_connect[i].load(Ordering::Relaxed)),
                )
            })
            .collect();
        SinceConnect {
            connected_for,
            received: received.saturating_sub(self.received_at_connect.load(Ordering::Relaxed)),
            skipped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_start_again_at_each_connection() {
        let l = Losses::default();
        l.note(Reader::Scalping, 5);
        l.connected(100, 1_000);
        l.note(Reader::Scalping, 3);
        l.note(Reader::FeedClients, 2);
        let s = l.since_connect(160, 4_000);
        assert_eq!(s.connected_for, Some(Duration::from_secs(3)));
        assert_eq!(s.received, 60);
        assert_eq!(s.skipped_total(), 5);
        assert!(s.skipped.contains(&(Reader::Scalping, 3)));
        assert!(s.skipped.contains(&(Reader::FeedClients, 2)));
        l.connected(200, 5_000);
        let s = l.since_connect(210, 5_500);
        assert_eq!((s.received, s.skipped_total()), (10, 0));
        assert_eq!(s.connected_for, Some(Duration::from_millis(500)));
        assert_eq!(Losses::default().since_connect(0, 9).connected_for, None);
    }
}
