//! Risk decisions waiting for the strategy evaluator to act on them.
//!
//! The feed's ticks are read by one task and acted on by another
//! (`tick_feed::start`). The reader decides every price of every held
//! symbol as it arrives, in the run's lock (`StrategyModule::decide_tick`),
//! so every ratchet (a leg's high and low, its trailing stop, the run's peak
//! and profit lock) and every breach is judged on the whole path, in the
//! order the prices came. Only acting waits for the evaluator: an exit to
//! place, a stop, an event to record, a page to update. While it is busy, a
//! run's later decisions are combined with those waiting
//! (`Decision::merge`), so what waits is bounded by the runs and their legs,
//! never by the prices.
//!
//! Prices used to wait instead, each symbol's folded past 32 to its first,
//! lowest, highest and latest, and that lost decisions (PROD-REL-01): a peak
//! that raised a trailing stop and the dip that then hit it were folded
//! away by a later, larger peak. No shorter path decides alike for every
//! rule: a stepped trail anchored on the live stop moves on every
//! evaluation.
//!
//! The web queues ticks in one bounded FIFO and sheds when it is full
//! (`services/strategy_module/tick_feed.py`, `TICK_QUEUE_MAX`), so a burst
//! there can drop the deciding tick itself; deciding on arrival is a
//! deliberate desktop departure.

use crate::strategy::store::EventFields;
use crate::strategy::tick_context::TickContext;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;

/// One notification a decision raises: its kind, message and fields.
pub(crate) type Event = (&'static str, String, EventFields);

/// Notifications of a level that keeps moving: of those waiting, only the
/// latest is recorded.
const PROGRESS: &[&str] = &["leg_trail_advanced", "lock_profit_floor_advanced"];

/// An exit risk decided for a leg, and the position it was decided on.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LegExit {
    pub(crate) leg_id: i64,
    pub(crate) kind: &'static str,
    /// The leg's position when it breached. A leg re-entered since holds
    /// another position, which this exit must not close.
    pub(crate) position_ref: Option<String>,
}

/// What one price decided for one run (in the run's lock) and, while it
/// waits, every later decision combined into it.
#[derive(Debug, Default)]
pub(crate) struct Decision {
    pub(crate) leg_exits: Vec<LegExit>,
    /// A strategy-level breach: the run stops.
    pub(crate) stop_reason: Option<&'static str>,
    pub(crate) events: Vec<Event>,
}

impl Decision {
    /// Combine a later decision with this one, as acting on each in turn
    /// would: a waiting stop closes every leg, so no action after it is
    /// kept; a price that stops the run exits no leg of its own (the stop
    /// does); a position keeps its first exit (its claim refuses a second).
    /// A notification is kept once per kind and leg, a moving level's
    /// latest replacing the one before.
    fn merge(&mut self, later: Decision) {
        for e in later.events {
            self.note(e);
        }
        if self.stop_reason.is_some() {
            return;
        }
        if later.stop_reason.is_some() {
            self.stop_reason = later.stop_reason;
            return;
        }
        for x in later.leg_exits {
            let waiting = self
                .leg_exits
                .iter()
                .any(|w| w.leg_id == x.leg_id && w.position_ref == x.position_ref);
            if !waiting {
                self.leg_exits.push(x);
            }
        }
    }

    fn note(&mut self, e: Event) {
        let same = self
            .events
            .iter()
            .position(|w| w.0 == e.0 && w.2.leg_id == e.2.leg_id);
        match same {
            None => self.events.push(e),
            Some(i) if PROGRESS.contains(&e.0) => {
                self.events.remove(i);
                self.events.push(e);
            }
            Some(_) => {}
        }
    }
}

/// A run's decisions not acted on yet.
pub(crate) struct RunWork {
    pub(crate) ctx: Arc<TickContext>,
    pub(crate) decided: Decision,
    /// When the run's first waiting decision came: runs are acted on in the
    /// order they decided.
    arrived: u64,
}

/// The decisions waiting, and the runs the evaluator has taken and not yet
/// acted on.
#[derive(Default)]
struct Waiting {
    runs: HashMap<i64, RunWork>,
    arrivals: u64,
    acting: u64,
}

/// The decisions waiting for the evaluator, its wake-up, and what the
/// strategy tick path read, combined and missed (health).
#[derive(Default)]
pub(crate) struct Pending {
    waiting: Mutex<Waiting>,
    /// Woken on every decision left; the evaluator acts on all waiting.
    pub(super) wake: Arc<Notify>,
    /// Prices of held symbols read since start.
    read: AtomicU64,
    /// Decisions combined with ones still waiting for the evaluator (each
    /// was decided on its own price; only its follow-up was combined).
    combined: AtomicU64,
    /// Ticks the reader missed because the market-data channel overran it:
    /// lost to the strategy module.
    lagged: AtomicU64,
    /// `lagged` as of the last health alert.
    alerted: AtomicU64,
}

impl Pending {
    pub(super) fn note_read(&self) {
        self.read.fetch_add(1, Ordering::Relaxed);
    }

    /// Leave one price's decision for a run to the evaluator and wake it.
    pub(crate) fn leave(&self, run_id: i64, ctx: Arc<TickContext>, decision: Decision) {
        {
            let mut guard = self.waiting.lock();
            let waiting = &mut *guard;
            if let Some(work) = waiting.runs.get_mut(&run_id) {
                work.decided.merge(decision);
                self.combined.fetch_add(1, Ordering::Relaxed);
            } else {
                let mut decided = Decision::default();
                decided.merge(decision);
                waiting.arrivals += 1;
                let arrived = waiting.arrivals;
                waiting.runs.insert(
                    run_id,
                    RunWork {
                        ctx,
                        decided,
                        arrived,
                    },
                );
            }
        }
        self.wake.notify_one();
    }

    /// Everything waiting, run by run in the order they first decided. A
    /// run taken counts as waiting until `acted`.
    pub(crate) fn take(&self) -> Vec<(i64, RunWork)> {
        let mut runs: Vec<(i64, RunWork)> = {
            let mut waiting = self.waiting.lock();
            waiting.acting += waiting.runs.len() as u64;
            waiting.runs.drain().collect()
        };
        runs.sort_by_key(|(_, work)| work.arrived);
        runs
    }

    /// One run taken has been acted on.
    pub(crate) fn acted(&self) {
        let mut waiting = self.waiting.lock();
        waiting.acting = waiting.acting.saturating_sub(1);
    }

    /// Drop every decision waiting (shutdown).
    pub(super) fn clear(&self) {
        self.waiting.lock().runs.clear();
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

    /// The counts for health: `warn` once any tick was missed. The read and
    /// combined counts keep the keys they had when prices waited
    /// (`ticks_queued`, `ticks_folded`); the Health page names them.
    pub(super) fn health(&self) -> Value {
        let lagged = self.lagged.load(Ordering::Relaxed);
        let runs_waiting = {
            let waiting = self.waiting.lock();
            waiting.runs.len() as u64 + waiting.acting
        };
        json!({
            "status": if lagged == 0 { "pass" } else { "warn" },
            "ticks_queued": self.read.load(Ordering::Relaxed),
            "ticks_folded": self.combined.load(Ordering::Relaxed),
            "ticks_missed": lagged,
            "runs_waiting": runs_waiting,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exit(leg_id: i64, kind: &'static str) -> LegExit {
        LegExit {
            leg_id,
            kind,
            position_ref: Some(format!("position-{}", leg_id)),
        }
    }

    fn event(kind: &'static str, leg_id: Option<i64>, message: &str) -> Event {
        let fields = EventFields {
            leg_id,
            ..Default::default()
        };
        (kind, message.to_string(), fields)
    }

    fn exits(d: &Decision) -> Vec<(i64, &'static str)> {
        d.leg_exits.iter().map(|x| (x.leg_id, x.kind)).collect()
    }

    fn combined(decisions: Vec<Decision>) -> Decision {
        let mut d = Decision::default();
        for later in decisions {
            d.merge(later);
        }
        d
    }

    #[test]
    fn a_leg_keeps_its_first_exit_and_nothing_acts_after_a_stop() {
        let d = combined(vec![
            Decision {
                leg_exits: vec![exit(1, "exit_sl")],
                ..Default::default()
            },
            Decision {
                leg_exits: vec![exit(1, "exit_target"), exit(2, "exit_target")],
                ..Default::default()
            },
            Decision {
                stop_reason: Some("overall_sl"),
                ..Default::default()
            },
            Decision {
                leg_exits: vec![exit(3, "exit_sl")],
                stop_reason: Some("lock_profit"),
                ..Default::default()
            },
        ]);
        assert_eq!(exits(&d), vec![(1, "exit_sl"), (2, "exit_target")]);
        assert_eq!(d.stop_reason, Some("overall_sl"), "the first stop");
    }

    /// Acting on one price alone, a stop wins and the price's own leg
    /// exits are not placed (the stop exits every leg).
    #[test]
    fn a_price_that_stops_the_run_exits_no_leg_of_its_own() {
        let d = combined(vec![Decision {
            leg_exits: vec![exit(1, "exit_sl")],
            stop_reason: Some("lock_profit"),
            events: vec![event("leg_sl_hit", Some(1), "hit")],
        }]);
        assert!(d.leg_exits.is_empty());
        assert_eq!(d.stop_reason, Some("lock_profit"));
        assert_eq!(d.events.len(), 1, "its events are still recorded");
    }

    /// A leg re-entered while its first exit waits is another position: its
    /// own exit is kept beside the first, which is dropped when acted on.
    #[test]
    fn a_re_entered_position_keeps_its_own_exit() {
        let mut again = exit(1, "exit_sl");
        again.position_ref = Some("position-1b".into());
        let d = combined(vec![
            Decision {
                leg_exits: vec![exit(1, "exit_sl")],
                ..Default::default()
            },
            Decision {
                leg_exits: vec![again.clone()],
                ..Default::default()
            },
        ]);
        assert_eq!(d.leg_exits, vec![exit(1, "exit_sl"), again]);
    }

    #[test]
    fn a_notification_is_kept_once_and_a_moving_level_keeps_its_latest() {
        let d = combined(vec![
            Decision {
                events: vec![event("leg_trail_advanced", Some(1), "to 105")],
                ..Default::default()
            },
            Decision {
                leg_exits: vec![exit(1, "exit_sl")],
                events: vec![event("leg_sl_hit", Some(1), "at 104")],
                ..Default::default()
            },
            Decision {
                events: vec![
                    event("leg_sl_hit", Some(1), "at 103"),
                    event("leg_trail_advanced", Some(1), "to 145"),
                    event("leg_trail_advanced", Some(2), "to 98"),
                ],
                ..Default::default()
            },
            Decision {
                stop_reason: Some("overall_sl"),
                events: vec![event("overall_sl_hit", None, "down")],
                ..Default::default()
            },
            Decision {
                events: vec![event("lock_profit_triggered", None, "after the stop")],
                ..Default::default()
            },
        ]);
        let seen: Vec<(&str, Option<i64>, &str)> = d
            .events
            .iter()
            .map(|(k, m, f)| (*k, f.leg_id, m.as_str()))
            .collect();
        assert_eq!(
            seen,
            vec![
                ("leg_sl_hit", Some(1), "at 104"),
                ("leg_trail_advanced", Some(1), "to 145"),
                ("leg_trail_advanced", Some(2), "to 98"),
                ("overall_sl_hit", None, "down"),
                ("lock_profit_triggered", None, "after the stop"),
            ]
        );
    }

    #[test]
    fn counts_survive_and_a_miss_alerts_once() {
        let p = Pending::default();
        p.note_read();
        p.note_read();
        assert!(p.take().is_empty());
        p.note_lagged(3);
        assert_eq!(p.missed_since_alert(), Some(3));
        assert_eq!(p.missed_since_alert(), None, "once per growth");
        let h = p.health();
        assert_eq!(h["ticks_queued"], 2);
        assert_eq!(h["ticks_missed"], 3);
        assert_eq!(h["runs_waiting"], 0);
        assert_eq!(h["status"], "warn");
    }

    mod props {
        use super::*;
        use proptest::prelude::*;

        const LEGS: [i64; 3] = [1, 2, 3];

        /// What acting places: a position is exited once (its claim), a
        /// stop exits every leg not yet exited, and the first stop's reason
        /// is the one kept.
        #[derive(Debug, Default, PartialEq)]
        struct Placed {
            exits: Vec<(i64, &'static str)>,
            stop: Option<&'static str>,
        }

        impl Placed {
            fn exit(&mut self, leg_id: i64, kind: &'static str) {
                if self.exits.iter().all(|(l, _)| *l != leg_id) {
                    self.exits.push((leg_id, kind));
                }
            }

            fn stop(&mut self, reason: &'static str) {
                self.stop.get_or_insert(reason);
                for leg in LEGS {
                    self.exit(leg, "stop");
                }
            }

            /// The old path: each price acted on alone, a stop winning
            /// over the price's own exits (`act_on_risk`).
            fn act_alone(&mut self, d: &Decision) {
                match d.stop_reason {
                    Some(reason) => self.stop(reason),
                    None => d.leg_exits.iter().for_each(|x| self.exit(x.leg_id, x.kind)),
                }
            }

            /// The evaluator on combined decisions (`act_on_run`): the leg
            /// exits waiting, then the stop.
            fn act_combined(&mut self, d: &Decision) {
                d.leg_exits.iter().for_each(|x| self.exit(x.leg_id, x.kind));
                if let Some(reason) = d.stop_reason {
                    self.stop(reason);
                }
            }
        }

        fn decision() -> impl Strategy<Value = Decision> {
            let leg_exit = (
                prop::sample::select(LEGS.to_vec()),
                prop::sample::select(vec!["exit_sl", "exit_target"]),
            );
            (
                prop::collection::vec(leg_exit, 0..3),
                prop::option::weighted(
                    0.15,
                    prop::sample::select(vec!["overall_sl", "lock_profit"]),
                ),
            )
                .prop_map(|(exits, stop_reason)| Decision {
                    leg_exits: exits.into_iter().map(|(l, k)| exit(l, k)).collect(),
                    stop_reason,
                    events: vec![],
                })
        }

        proptest! {
            /// However the evaluator's turns cut a run of decisions,
            /// acting on each turn's combined decisions places exactly
            /// what acting on every decision in turn places.
            #[test]
            fn combining_decisions_places_what_acting_on_each_does(
                decisions in prop::collection::vec(decision(), 1..40),
                cuts in prop::collection::vec(any::<bool>(), 40),
            ) {
                let mut alone = Placed::default();
                for d in &decisions {
                    alone.act_alone(d);
                }
                let mut turns = Placed::default();
                let mut waiting = Decision::default();
                for (i, d) in decisions.into_iter().enumerate() {
                    waiting.merge(d);
                    if cuts[i] {
                        turns.act_combined(&std::mem::take(&mut waiting));
                    }
                }
                turns.act_combined(&waiting);
                prop_assert_eq!(turns, alone);
            }
        }
    }
}
