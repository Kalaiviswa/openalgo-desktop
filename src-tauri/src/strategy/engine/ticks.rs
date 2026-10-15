//! The tick decision path (web `engine.py` `process_tick`): evaluate every
//! run holding an instrument against one price and dispatch what risk
//! decides.

use super::{ev, exit_kind_for, stop_reason_for, StrategyModule};
use crate::risk::BreachReason;
use crate::strategy::dispatch::RunMode;
use crate::strategy::risk_adapter;
use crate::strategy::state::RunState;
use crate::strategy::store::{EventFields, StrategyRow};
use serde_json::{json, Value};

/// One event to send once the run's lock is released.
type Event = (&'static str, String, EventFields);

/// Each open leg on this instrument against the price (in the run's
/// lock): the legs to exit and the events to send. A stop-driven exit
/// trails the other open legs to entry when the strategy asks.
fn evaluate_legs(
    run: &mut RunState,
    run_id: i64,
    (symbol, exchange, ltp): (&str, &str, f64),
    trail_sl_to_entry: bool,
) -> (Vec<(i64, &'static str)>, Vec<Event>) {
    let mut leg_exits: Vec<(i64, &'static str)> = Vec::new();
    let mut events: Vec<Event> = Vec::new();
    let ids: Vec<i64> = run
        .legs
        .values()
        .filter(|l| l.status == "open" && l.symbol == symbol && l.exchange == exchange)
        .map(|l| l.leg_id)
        .collect();
    for id in ids {
        let Some(leg) = run.leg_mut(id) else { continue };
        let d = match risk_adapter::evaluate_leg(leg, ltp) {
            Ok(d) => d,
            Err(e) => {
                tracing::error!("Run {} leg {} not evaluated: {}", run_id, id, e);
                continue;
            }
        };
        if d.trail_armed && d.stop_moved {
            events.push((
                "leg_trail_advanced",
                format!(
                    "Trailing stop on leg {} moved to {}",
                    id,
                    d.stop_price
                        .map(crate::risk::format_price)
                        .unwrap_or_default()
                ),
                ev(Some(run_id), Some(id), "info"),
            ));
        }
        if let Some(reason) = d.reason {
            if let Some(kind) = exit_kind_for(reason) {
                leg_exits.push((id, kind));
                events.push((
                    if reason == BreachReason::Stop {
                        "leg_sl_hit"
                    } else {
                        "leg_target_hit"
                    },
                    d.detail.clone(),
                    ev(Some(run_id), Some(id), "warn"),
                ));
            }
        }
    }
    if trail_sl_to_entry {
        if let Some((id, _)) = leg_exits.iter().find(|(_, k)| *k == "exit_sl") {
            let moved = risk_adapter::trail_open_legs_to_entry(run, *id);
            if !moved.is_empty() {
                events.push((
                    "trail_to_entry_activated",
                    format!(
                        "Stop on leg {} moved {} other legs to entry",
                        id,
                        moved.len()
                    ),
                    ev(Some(run_id), None, "warn"),
                ));
            }
        }
    }
    (leg_exits, events)
}

/// The strategy-level breach this mark makes, if any (in the run's lock):
/// the lock profit's progress is announced, the daily loss limit is a
/// limit on the session (the realized P&L banked earlier today plus this
/// run), then the overall stop, target and lock profit, each with the
/// breach's payload.
fn run_breach(
    run: &mut RunState,
    (strategy, strategy_dict): (&StrategyRow, &Value),
    (limit, banked): (Option<f64>, Option<f64>),
    (run_id, symbol, exchange, ltp): (i64, &str, &str, f64),
    events: &mut Vec<Event>,
) -> Option<&'static str> {
    let aggregate = match risk_adapter::evaluate_run(run, strategy_dict) {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("Run {} not evaluated: {}", run_id, e);
            return None;
        }
    };
    if aggregate.lock_armed_now {
        events.push((
            "lock_profit_armed",
            format!(
                "Lock profit now active with a floor of {}",
                aggregate
                    .lock_floor
                    .map(crate::risk::format_price)
                    .unwrap_or_default()
            ),
            ev(Some(run_id), None, "info"),
        ));
    } else if aggregate.lock_floor_raised {
        events.push((
            "lock_profit_floor_advanced",
            format!(
                "Lock profit floor advanced to {}",
                aggregate
                    .lock_floor
                    .map(crate::risk::format_price)
                    .unwrap_or_default()
            ),
            ev(Some(run_id), None, "info"),
        ));
    }
    // The daily loss limit is a limit on the session, not the run.
    if let (Some(limit), Some(banked)) = (limit, banked) {
        let day = banked + run.pnl_total;
        if day <= -limit {
            events.push((
                "overall_sl_hit",
                format!(
                    "Daily loss limit reached: the session is down {:.2} against a limit of {:.2}",
                    day.abs(),
                    limit
                ),
                ev(Some(run_id), None, "critical"),
            ));
            return Some("daily_loss_limit");
        }
    }
    if let Some(reason) = aggregate.reason.filter(|_| aggregate.breached) {
        if let Some(sr) = stop_reason_for(reason) {
            let threshold = match sr {
                "overall_sl" => -strategy.overall_sl_mtm.unwrap_or(0.0).abs(),
                "overall_target" => strategy.overall_target_mtm.unwrap_or(0.0),
                _ => aggregate.lock_floor.unwrap_or(0.0),
            };
            let legs: Vec<Value> = run
                .legs
                .values()
                .filter(|l| l.status == "open" || l.realized_pnl != 0.0)
                .map(|l| {
                    json!({
                        "symbol": l.symbol, "exchange": l.exchange, "ltp": l.ltp,
                        "mtm": (l.mtm * 100.0).round() / 100.0,
                        "tick_source": l.tick_source, "qty": l.qty, "position": l.position,
                    })
                })
                .collect();
            let kind = match sr {
                "overall_sl" => "overall_sl_hit",
                "overall_target" => "overall_target_hit",
                _ => "lock_profit_triggered",
            };
            events.push((
                kind,
                aggregate.detail.clone(),
                EventFields {
                    run_id: Some(run_id),
                    leg_id: None,
                    severity: Some("warn"),
                    payload: Some(json!({
                        "trigger_total": (aggregate.total_pnl * 100.0).round() / 100.0,
                        "reason": sr,
                        "threshold": (threshold * 100.0).round() / 100.0,
                        "triggering_tick": {"symbol": symbol, "exchange": exchange, "ltp": ltp},
                        "legs": legs,
                    })),
                },
            ));
            return Some(sr);
        }
    }
    None
}

impl StrategyModule {
    /// Evaluate every run holding this instrument against one price.
    pub async fn process_tick(&self, symbol: &str, exchange: &str, ltp: f64) {
        for run_id in self.state.active_run_ids() {
            let holds = self
                .state
                .with_run(run_id, |r| {
                    r.legs
                        .values()
                        .any(|l| l.status == "open" && l.symbol == symbol && l.exchange == exchange)
                })
                .unwrap_or(false);
            if holds {
                self.process_tick_for_run(run_id, symbol, exchange, ltp)
                    .await;
            }
        }
    }

    async fn process_tick_for_run(&self, run_id: i64, symbol: &str, exchange: &str, ltp: f64) {
        let Ok(Some(run_row)) = self.store.get_run(run_id) else {
            return;
        };
        if run_row.stopped_at.is_some() {
            return;
        }
        let strategy_id = run_row.strategy_id;
        let mode = self.mode_of(&run_row.mode);
        let Ok(Some(strategy)) = self.store.get_strategy_unscoped(strategy_id) else {
            return;
        };
        let strategy_dict = strategy.to_dict(false);
        let user_id = strategy.user_id.clone();

        // Read before the lock, never inside it.
        let (limit, banked) = self.session_loss(&strategy, run_id);

        // In-memory arithmetic only.
        let evaluated = self.state.with_run(run_id, |run| {
            let (leg_exits, mut events) = evaluate_legs(
                run,
                run_id,
                (symbol, exchange, ltp),
                strategy.trail_sl_to_entry,
            );
            let stop_reason = run_breach(
                run,
                (&strategy, &strategy_dict),
                (limit, banked),
                (run_id, symbol, exchange, ltp),
                &mut events,
            );
            (leg_exits, stop_reason, events)
        });
        let Some((leg_exits, stop_reason, events)) = evaluated else {
            return;
        };

        // Lock released: everything below reaches the database or broker.
        self.broadcast
            .push_delta(self.state.snapshot(run_id), false)
            .await;
        for (kind, message, fields) in events {
            self.emit(strategy_id, &user_id, kind, &message, fields)
                .await;
        }
        self.act_on_risk(run_id, (&strategy, mode), stop_reason, leg_exits)
            .await;
    }

    /// The strategy's daily loss limit and, when it has one, the realized
    /// P&L banked earlier this session by its other runs (read before the
    /// run's lock, never inside it).
    fn session_loss(&self, strategy: &StrategyRow, run_id: i64) -> (Option<f64>, Option<f64>) {
        let strategy_id = strategy.id;
        let limit = strategy
            .daily_loss_limit_inr
            .map(f64::abs)
            .filter(|l| *l > 0.0);
        let banked = match limit {
            Some(_) => {
                let since = crate::strategy::session::session_started_at(
                    self.clock.now(),
                    self.session.0,
                    self.session.1,
                );
                Some(
                    self.store
                        .realized_pnl_since(strategy_id, since, Some(run_id))
                        .unwrap_or(0.0),
                )
            }
            None => None,
        };
        (limit, banked)
    }

    /// What risk decided, once the lock is released: a strategy-level
    /// breach stops the run; leg exits are placed, unless there is no
    /// broker session (recorded once per episode, never pretended).
    async fn act_on_risk(
        &self,
        run_id: i64,
        (strategy, mode): (&StrategyRow, RunMode),
        stop_reason: Option<&'static str>,
        leg_exits: Vec<(i64, &'static str)>,
    ) {
        let strategy_id = strategy.id;
        let user_id = strategy.user_id.clone();
        if let Some(reason) = stop_reason {
            // A strategy-level breach closes everything.
            let r = self.stop_run(run_id, &user_id, reason).await;
            if r.ok {
                self.note_actionable_again(strategy_id, &user_id, run_id)
                    .await;
            }
            return;
        }
        if leg_exits.is_empty() {
            return;
        }
        if self.gateway.authorised(mode).is_err() {
            // The 3 AM window: record once per episode, never pretend to exit.
            if self.claim_unactionable(run_id) {
                tracing::warn!(
                    "Run {} has risk to act on but no broker session; positions left open",
                    run_id
                );
                self.emit(
                    strategy_id,
                    &user_id,
                    "leg_exit_rejected",
                    "Risk triggered but there is no broker session, so nothing could be exited. Positions are still open. Log in to your broker to restore the session.",
                    ev(Some(run_id), None, "critical"),
                )
                .await;
            }
            return;
        }
        self.note_actionable_again(strategy_id, &user_id, run_id)
            .await;
        for (leg_id, kind) in leg_exits {
            self.exit_legs(run_id, strategy, &[leg_id], kind, mode, &user_id)
                .await;
        }
    }
}
