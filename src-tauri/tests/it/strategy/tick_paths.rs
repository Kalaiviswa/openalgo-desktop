//! Generated price paths (PROD-REL-01). Forty runs, each with its own
//! leg (long or short; stop, target, continuous or stepped trail) and its
//! own combined limits (stop, target, profit lock, trailing lock), get 40 to
//! 90 prices each, interleaved, all while the evaluator is held. Each run
//! must decide what evaluating every price in order through the pure risk
//! core decides: the same first exit, the same stop, the same levels at the
//! end, and one exit order at most.

use super::tick_bursts::{eventually, exit_kinds, option_leg, send_while_held, settled, Burst};
use super::*;
use openalgo_desktop_lib::risk::BreachReason;
use openalgo_desktop_lib::strategy::risk_adapter as ra;
use openalgo_desktop_lib::strategy::state::RunState;
use openalgo_desktop_lib::strategy::tick_feed::PriceSource;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::sync::Arc;

const SEED: u64 = 0x2026_1022;
const RUNS: usize = 40;

/// Every contract a run can hold (the offsets a NIFTY leg supports), less
/// the blocker's (the weekly OTM3 call).
fn contracts() -> Vec<(&'static str, &'static str, String)> {
    let mut offsets = vec!["ATM".to_string()];
    for i in 1..=5 {
        offsets.push(format!("ITM{}", i));
        offsets.push(format!("OTM{}", i));
    }
    let mut v = Vec::new();
    for expiry in ["weekly", "monthly"] {
        for option_type in ["CE", "PE"] {
            for offset in &offsets {
                if (expiry, option_type, offset.as_str()) != ("weekly", "CE", "OTM3") {
                    v.push((expiry, option_type, offset.clone()));
                }
            }
        }
    }
    v
}

/// A leg's own risk: a stop, a target, and no trail, a continuous one or a
/// stepped one, each drawn.
fn leg_risk(rng: &mut StdRng) -> Value {
    let mut risk = json!({});
    if rng.gen_bool(0.8) {
        risk["sl_pts"] = json!(rng.gen_range(5..=20));
    }
    if rng.gen_bool(0.5) {
        risk["target_pts"] = json!(rng.gen_range(10..=60));
    }
    risk["trail"] = match rng.gen_range(0..3) {
        0 => json!({"x": 0, "y": 0}),
        1 => json!({"x": rng.gen_range(2..=10), "y": 0}),
        _ => {
            let x = rng.gen_range(4..=12);
            json!({"x": x, "y": rng.gen_range(1..=x)})
        }
    };
    risk
}

/// The run's combined limits, in rupees (a point is 65 on one lot).
fn run_limits(rng: &mut StdRng) -> Value {
    match rng.gen_range(0..5) {
        0 => json!({}),
        1 => json!({"overall_sl_mtm": 65 * rng.gen_range(5..=25)}),
        2 => json!({"overall_target_mtm": 65 * rng.gen_range(15..=60)}),
        _ => {
            let at = rng.gen_range(5..=25);
            let mode = if rng.gen_bool(0.5) {
                "lock"
            } else {
                "lock_and_trail"
            };
            json!({"lock_profit": {"mode": mode, "if_profit_reaches": 65 * at,
                                   "lock_profit": 65 * rng.gen_range(1..at),
                                   "trail_step": 65 * rng.gen_range(1..=6)}})
        }
    }
}

/// A path from the entry: a drift with noise and sudden moves, so peaks,
/// reversals and new extremes keep coming after the 32nd price.
fn price_path(rng: &mut StdRng) -> Vec<f64> {
    let len = rng.gen_range(40..=90);
    let drift = rng.gen_range(-0.3..0.3);
    let mut price: f64 = 100.0;
    let mut v = vec![price];
    while v.len() < len {
        let step = if rng.gen_bool(0.1) {
            rng.gen_range(-12.0..12.0)
        } else {
            rng.gen_range(-2.0..2.0) + drift
        };
        price = ((price + step).max(5.0) * 20.0).round() / 20.0;
        v.push(price);
    }
    v
}

/// What a run decided, and the levels it ended on.
#[derive(Debug, PartialEq)]
struct Outcome {
    first_exit: Option<String>,
    exits: usize,
    stop: Option<String>,
    leg_stop: Option<f64>,
    leg_target: Option<f64>,
    high: Option<f64>,
    low: Option<f64>,
    last: Option<f64>,
    trail_active: bool,
    peak: f64,
    trough: f64,
    lock_active: bool,
    lock_floor: Option<f64>,
}

/// The levels a run ended on, with what it decided still to fill in.
fn levels(run: &RunState) -> Outcome {
    let leg = run.leg(1).unwrap();
    Outcome {
        first_exit: None,
        exits: 0,
        stop: None,
        leg_stop: leg.effective_sl,
        leg_target: leg.effective_target,
        high: leg.highest_price,
        low: leg.lowest_price,
        last: leg.ltp,
        trail_active: leg.trail_active,
        peak: run.pnl_peak,
        trough: run.pnl_trough,
        lock_active: run.lock_armed,
        lock_floor: run.lock_floor,
    }
}

/// The engine's name for a run-level breach (`stop_reason_for`).
fn stop_reason(r: BreachReason) -> String {
    match r {
        BreachReason::CombinedStop => "overall_sl",
        BreachReason::CombinedTarget => "overall_target",
        BreachReason::LockProfit => "lock_profit",
        other => panic!("not a run-level breach: {:?}", other),
    }
    .to_string()
}

/// What evaluating every price in order through the pure risk core
/// decides: the leg first, then the run, as the engine does per price. A
/// run-level breach on the same price wins (it stops the run), and a later
/// decision places nothing (its leg is already claimed).
fn uncompressed(start: &RunState, strategy: &Value, path: &[f64]) -> Outcome {
    let mut run = start.clone();
    let mut leg_breach: Option<(usize, BreachReason)> = None;
    let mut run_breach: Option<(usize, BreachReason)> = None;
    for (i, ltp) in path.iter().enumerate() {
        let d = ra::evaluate_leg(run.leg_mut(1).unwrap(), *ltp).unwrap();
        leg_breach = leg_breach.or(d.reason.map(|r| (i, r)));
        let a = ra::evaluate_run(&mut run, strategy).unwrap();
        if a.breached {
            run_breach = run_breach.or(a.reason.map(|r| (i, r)));
        }
    }
    let first_exit = match (leg_breach, run_breach) {
        (Some((i, _)), Some((j, r))) if j <= i => Some(format!("exit_{}", stop_reason(r))),
        (None, Some((_, r))) => Some(format!("exit_{}", stop_reason(r))),
        (Some((_, BreachReason::Stop)), _) => Some("exit_sl".to_string()),
        (Some((_, BreachReason::Target)), _) => Some("exit_target".to_string()),
        (Some((_, r)), _) => panic!("not a leg breach: {:?}", r),
        (None, None) => None,
    };
    Outcome {
        exits: usize::from(first_exit.is_some()),
        first_exit,
        stop: run_breach.map(|(_, r)| stop_reason(r)),
        ..levels(&run)
    }
}

/// What the module decided for the run.
fn observed(t: &T, run: i64) -> Outcome {
    let kinds = exit_kinds(t, run);
    Outcome {
        exits: kinds.len(),
        first_exit: kinds.into_iter().next(),
        stop: t.run(run).stop_requested_reason,
        ..levels(&t.m.state.snapshot(run).unwrap())
    }
}

struct Case {
    run: i64,
    symbol: String,
    path: Vec<f64>,
    expected: Outcome,
}

#[tokio::test]
async fn every_generated_path_decides_as_evaluating_every_price_does() {
    let burst = Burst::new(8192);
    let t = t_with(
        ist(2026, 10, 7, 10, 0),
        Some(burst.clone() as Arc<dyn PriceSource>),
    );
    t.m.start().await;
    let mut rng = StdRng::seed_from_u64(SEED);
    let mut cases = Vec::new();
    for (i, (expiry, option_type, offset)) in contracts().into_iter().take(RUNS).enumerate() {
        let side = if rng.gen_bool(0.5) { "B" } else { "S" };
        let leg = option_leg(1, (side, expiry, option_type, &offset), leg_risk(&mut rng));
        let sid = t.make(config(
            &format!("Path {}", i),
            json!([leg]),
            run_limits(&mut rng),
        ));
        let run = t.start_filled(sid, 100.0).await;
        let start = t.m.state.snapshot(run).unwrap();
        let path = price_path(&mut rng);
        cases.push(Case {
            run,
            symbol: start.leg(1).unwrap().symbol.clone(),
            expected: uncompressed(&start, &t.row(sid).to_dict(false), &path),
            path,
        });
    }
    let symbols: std::collections::HashSet<&str> =
        cases.iter().map(|c| c.symbol.as_str()).collect();
    assert_eq!(symbols.len(), RUNS, "one contract per run");

    // The paths interleaved, price by price, as a burst arrives.
    let longest = cases.iter().map(|c| c.path.len()).max().unwrap_or(0);
    let mut sent = Vec::new();
    for k in 0..longest {
        for c in &cases {
            if let Some(p) = c.path.get(k) {
                sent.push((c.symbol.clone(), *p));
            }
        }
    }
    send_while_held(&t, &burst, &sent).await;

    let all_match = || cases.iter().all(|c| observed(&t, c.run) == c.expected);
    eventually(all_match).await;
    settled(&t).await;
    let wrong: Vec<String> = cases
        .iter()
        .filter_map(|c| {
            let got = observed(&t, c.run);
            (got != c.expected).then(|| {
                format!(
                    "run {} ({} prices): expected {:?}\n  got {:?}",
                    c.run,
                    c.path.len(),
                    c.expected,
                    got
                )
            })
        })
        .collect();
    let decided = cases
        .iter()
        .filter(|c| c.expected.first_exit.is_some())
        .count();
    assert!(
        wrong.is_empty(),
        "{} of {} generated paths ({} with an exit) decided differently from evaluating every price:\n{}",
        wrong.len(),
        cases.len(),
        decided,
        wrong.join("\n")
    );
    assert!(decided > RUNS / 4, "the paths exercise exits: {}", decided);
    t.m.shutdown().await;
}
