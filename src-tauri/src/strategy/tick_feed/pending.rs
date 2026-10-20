//! Prices waiting for the strategy evaluator, kept per symbol so that a
//! burst never loses a deciding price.
//!
//! The feed's ticks are read by one task and evaluated by another
//! (`tick_feed::start`). While the evaluator is busy (an exit being placed,
//! a database write), the reader keeps every price of every held symbol in
//! that symbol's path, in arrival order. A path that grows past
//! [`PATH_MAX`] is folded to its first price, its lowest and highest in the
//! order they came, and its latest: a spike through a stop, a target or a
//! trailing stop is still evaluated, and the trailing move before a dip is
//! still seen before the dip. Only prices in between are folded, and they
//! are counted.
//!
//! The web queues ticks in one bounded FIFO and sheds when it is full
//! (`services/strategy_module/tick_feed.py`, `TICK_QUEUE_MAX`), so a burst
//! there can drop the deciding tick itself; folding per symbol is a
//! deliberate desktop departure.

use super::Key;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;

/// Prices one symbol keeps before its path is folded.
const PATH_MAX: usize = 32;

/// One symbol's prices since the evaluator last took them.
#[derive(Debug, Default, Clone, PartialEq)]
struct PricePath {
    points: Vec<f64>,
}

impl PricePath {
    /// Add a price; answers how many prices folding dropped (0 unless the
    /// path was full).
    fn push(&mut self, price: f64) -> u64 {
        self.points.push(price);
        if self.points.len() <= PATH_MAX {
            return 0;
        }
        let before = self.points.len();
        self.points = fold(&self.points);
        (before - self.points.len()) as u64
    }

    #[cfg(test)]
    fn points(&self) -> &[f64] {
        &self.points
    }

    fn into_points(self) -> Vec<f64> {
        self.points
    }
}

/// The first price, the lowest and highest of those after it in the order
/// they came, and the latest.
fn fold(points: &[f64]) -> Vec<f64> {
    let (Some(&first), Some(&last)) = (points.first(), points.last()) else {
        return points.to_vec();
    };
    if points.len() <= 2 {
        return points.to_vec();
    }
    let middle = &points[1..points.len() - 1];
    let mut low = 0;
    let mut high = 0;
    for (i, p) in middle.iter().enumerate() {
        if *p < middle[low] {
            low = i;
        }
        if *p > middle[high] {
            high = i;
        }
    }
    let mut kept = vec![first];
    let (a, b) = if low <= high {
        (low, high)
    } else {
        (high, low)
    };
    kept.push(middle[a]);
    if b != a {
        kept.push(middle[b]);
    }
    kept.push(last);
    kept
}

/// The paths waiting for the evaluator, its wake-up, and what the strategy
/// tick path missed or folded (health).
#[derive(Default)]
pub(super) struct Pending {
    paths: Mutex<HashMap<Key, PricePath>>,
    /// Woken on every queued price; the evaluator takes everything waiting.
    pub(super) wake: Arc<Notify>,
    /// Prices queued for evaluation since start.
    queued: AtomicU64,
    /// Prices folded into their symbol's extremes while the evaluator was
    /// busy (not lost: the extremes and the latest are evaluated).
    folded: AtomicU64,
    /// Ticks the reader missed because the market-data channel overran it:
    /// lost to the strategy module.
    lagged: AtomicU64,
    /// `lagged` as of the last health alert.
    alerted: AtomicU64,
}

impl Pending {
    /// Queue one price of a held symbol and wake the evaluator.
    pub(super) fn queue(&self, key: Key, price: f64) {
        let dropped = self.paths.lock().entry(key).or_default().push(price);
        self.queued.fetch_add(1, Ordering::Relaxed);
        if dropped > 0 {
            self.folded.fetch_add(dropped, Ordering::Relaxed);
        }
        self.wake.notify_one();
    }

    /// Everything waiting, each symbol's prices in the order they came.
    pub(super) fn take(&self) -> Vec<(Key, Vec<f64>)> {
        std::mem::take(&mut *self.paths.lock())
            .into_iter()
            .map(|(k, p)| (k, p.into_points()))
            .collect()
    }

    /// Forget the prices of symbols no run holds any more.
    pub(super) fn retain(&self, held: impl Fn(&Key) -> bool) {
        self.paths.lock().retain(|k, _| held(k));
    }

    pub(super) fn note_lagged(&self, n: u64) {
        self.lagged.fetch_add(n, Ordering::Relaxed);
        tracing::warn!(
            "The strategy tick reader missed {} prices: the market-data channel overran it",
            n
        );
    }

    /// The missed count when it grew since the last call (one alert per
    /// growth).
    pub(super) fn missed_since_alert(&self) -> Option<u64> {
        let missed = self.lagged.load(Ordering::Relaxed);
        (self.alerted.fetch_max(missed, Ordering::Relaxed) < missed).then_some(missed)
    }

    /// The counts for health: `warn` once any tick was missed.
    pub(super) fn health(&self) -> Value {
        let lagged = self.lagged.load(Ordering::Relaxed);
        json!({
            "status": if lagged == 0 { "pass" } else { "warn" },
            "ticks_queued": self.queued.load(Ordering::Relaxed),
            "ticks_folded": self.folded.load(Ordering::Relaxed),
            "ticks_missed": lagged,
            "symbols_waiting": self.paths.lock().len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_path_keeps_every_price_in_order() {
        let mut p = PricePath::default();
        for x in [100.0, 101.0, 99.5, 100.2] {
            assert_eq!(p.push(x), 0);
        }
        assert_eq!(p.points(), &[100.0, 101.0, 99.5, 100.2]);
    }

    /// A full path keeps its first price, its extremes in the order they
    /// came and the latest: the spike and the dip after it survive.
    #[test]
    fn a_full_path_keeps_its_extremes_in_order() {
        let mut p = PricePath::default();
        p.push(100.0);
        for i in 0..10 {
            p.push(100.0 + f64::from(i) * 0.01);
        }
        p.push(150.0); // the spike
        for _ in 0..10 {
            p.push(100.5);
        }
        p.push(135.0);
        p.push(80.0); // the dip, after the spike
        let mut dropped = 0;
        for _ in 0..20 {
            dropped += p.push(100.1);
        }
        assert!(dropped > 0, "folded");
        let pts = p.points();
        assert_eq!(pts[0], 100.0);
        let spike = pts.iter().position(|x| *x == 150.0).expect("spike kept");
        let dip = pts.iter().position(|x| *x == 80.0).expect("dip kept");
        assert!(spike < dip, "{:?}", pts);
        assert_eq!(*pts.last().unwrap(), 100.1);
        assert!(pts.len() <= PATH_MAX);
        // The dip before the spike is kept in that order too.
        assert_eq!(fold(&[1.0, 0.5, 9.0, 2.0]), vec![1.0, 0.5, 9.0, 2.0]);
        assert_eq!(fold(&[1.0, 9.0, 3.0, 0.5, 2.0]), vec![1.0, 9.0, 0.5, 2.0]);
    }

    #[test]
    fn take_empties_the_paths_and_counts_survive() {
        let p = Pending::default();
        let k = ("SBIN".to_string(), "NSE".to_string());
        p.queue(k.clone(), 1.0);
        p.queue(k.clone(), 2.0);
        assert_eq!(p.take(), vec![(k, vec![1.0, 2.0])]);
        assert!(p.take().is_empty());
        p.note_lagged(3);
        assert_eq!(p.missed_since_alert(), Some(3));
        assert_eq!(p.missed_since_alert(), None, "once per growth");
        let h = p.health();
        assert_eq!(h["ticks_queued"], 2);
        assert_eq!(h["ticks_missed"], 3);
        assert_eq!(h["status"], "warn");
    }
}
