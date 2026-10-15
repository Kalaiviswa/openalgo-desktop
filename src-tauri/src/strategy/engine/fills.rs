//! Recording fills against the exact position they belong to, and the
//! run's realized P&L (web `engine.py` `apply_fill`).

use super::{exit_fill_quantities, FillOpts, StrategyModule};
use crate::strategy::risk_adapter;
use futures_util::future::BoxFuture;

impl StrategyModule {
    /// Record a fill against the exact position it belongs to. Returns
    /// whether the run went flat.
    pub fn apply_fill<'a>(
        &'a self,
        run_id: i64,
        leg_id: i64,
        avg_price: Option<f64>,
        is_entry: bool,
        opts: FillOpts,
    ) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            self.apply_fill_inner(run_id, leg_id, avg_price, is_entry, opts)
                .await
        })
    }

    async fn apply_fill_inner(
        &self,
        run_id: i64,
        leg_id: i64,
        avg_price: Option<f64>,
        is_entry: bool,
        opts: FillOpts,
    ) -> bool {
        let mut warnings: Vec<String> = Vec::new();
        enum Outcome {
            NoState,
            Ignored,
            Applied {
                went_flat: bool,
                entry_applied: bool,
                strategy_id: i64,
            },
        }
        let price = avg_price.filter(|p| p.is_finite() && *p > 0.0);
        let outcome = self
            .state
            .with_run(run_id, |run| {
                let strategy_id = run.strategy_id;
                let Some(leg) = run.leg_mut(leg_id) else {
                    return Outcome::Ignored;
                };
                let mut entry_applied = false;
                let settles_superseded = !is_entry
                    && leg.superseded.as_ref().is_some_and(|sup| {
                        (opts.position_ref.is_some() && sup.position_ref == opts.position_ref)
                            || (opts.position_ref.is_none()
                                && opts.order_row_id.is_some()
                                && sup.exit_order_id == opts.order_row_id)
                            || (opts.position_ref.is_none()
                                && opts.order_row_id.is_none()
                                && leg.exit_order_id.is_none())
                    });
                if settles_superseded {
                    // Settle the outgoing position from its own entry and
                    // size; the live position is untouched.
                    let mut realized_add = 0.0;
                    let mut clear = false;
                    if let Some(sup) = leg.superseded.as_mut() {
                        let entry = sup.entry_avg;
                        let (applied, remaining) = exit_fill_quantities(opts.filled_qty, sup.qty);
                        let sign = if sup.position == "B" { 1.0 } else { -1.0 };
                        if entry > 0.0 {
                            if let Some(p) = price {
                                realized_add = (p - entry) * applied as f64 * sign;
                            }
                        }
                        let owns =
                            opts.order_row_id.is_none() || sup.exit_order_id == opts.order_row_id;
                        let release = opts.order_terminal && owns;
                        if remaining > 0 || !release {
                            sup.qty = remaining;
                            if release {
                                sup.exit_order_id = None;
                                sup.exit_claim_token = None;
                                sup.exit_kind = None;
                            }
                        } else {
                            clear = true;
                        }
                    }
                    if clear {
                        leg.superseded = None;
                    }
                    leg.realized_pnl += realized_add;
                } else {
                    if opts.position_ref.is_some() && leg.position_ref != opts.position_ref {
                        warnings.push(format!(
                            "Ignoring a fill for position {:?} on leg {}: the live position is {:?}",
                            opts.position_ref, leg_id, leg.position_ref
                        ));
                        return Outcome::Ignored;
                    }
                    // A fill naming an order this leg is not waiting on
                    // belongs to an incarnation already replaced.
                    if let Some(row) = opts.order_row_id {
                        let expected = if is_entry {
                            leg.entry_order_id
                        } else {
                            leg.exit_order_id
                        };
                        if let Some(exp) = expected {
                            if exp != row && !opts.allow_prior_order_correction {
                                warnings.push(format!(
                                    "Ignoring a fill for order {} on leg {}: the leg is waiting on {}",
                                    row, leg_id, exp
                                ));
                                return Outcome::Ignored;
                            }
                        }
                        if !is_entry
                            && leg.exit_kind.is_none()
                            && leg.exit_order_id.is_none()
                            && !opts.allow_prior_order_correction
                        {
                            warnings.push(format!(
                                "Ignoring exit fill for order {} on leg {}: it has no exit in flight",
                                row, leg_id
                            ));
                            return Outcome::Ignored;
                        }
                    }
                    if is_entry {
                        match price {
                            Some(p) => leg.entry_avg = p,
                            None => warnings.push(format!(
                                "Leg {} on run {} filled without a usable average price; managing its quantity with valuation unavailable",
                                leg_id, run_id
                            )),
                        }
                        let managed = opts.cumulative_filled_qty.or(opts.filled_qty);
                        if let Some(q) = managed {
                            if q != leg.qty {
                                warnings.push(format!(
                                    "Leg {} on run {} filled {} of {}; managing the filled size",
                                    leg_id, run_id, q, leg.qty
                                ));
                                leg.qty = q;
                            }
                        }
                        if let Some(c) = opts.cumulative_filled_qty {
                            leg.entry_filled_qty = c;
                        }
                        leg.entry_status = if opts.order_terminal {
                            "complete".into()
                        } else {
                            "open".into()
                        };
                        leg.status = "open".into();
                        entry_applied = opts.order_terminal;
                    } else {
                        if let Some(p) = price {
                            leg.exit_avg = Some(p);
                        }
                        let entry = leg.entry_avg;
                        let (applied, remaining) = exit_fill_quantities(opts.filled_qty, leg.qty);
                        let sign = if leg.position == "B" { 1.0 } else { -1.0 };
                        if applied > 0 {
                            match price {
                                Some(p) if entry > 0.0 => {
                                    leg.realized_pnl += (p - entry) * applied as f64 * sign;
                                }
                                _ => warnings.push(format!(
                                    "Leg {} on run {} exited without complete fill pricing; booking no realized P&L for that quantity",
                                    leg_id, run_id
                                )),
                            }
                        }
                        let owns = opts.order_row_id.is_none() || leg.exit_order_id == opts.order_row_id;
                        let release = opts.order_terminal && owns;
                        if remaining > 0 {
                            leg.qty = remaining;
                            leg.status = "open".into();
                        } else {
                            leg.qty = 0;
                            leg.status = "closed".into();
                            leg.mtm = 0.0;
                        }
                        if release {
                            leg.exit_order_id = None;
                            leg.exit_claim_token = None;
                            leg.exit_kind = None;
                        }
                    }
                }
                // Recompute the run totals while the lock is held.
                if let Ok((realized, unrealized)) = risk_adapter::run_pnl(run) {
                    run.pnl_realized = realized;
                    run.pnl_unrealized = unrealized;
                    run.pnl_total = realized + unrealized;
                    if run.pnl_total > run.pnl_peak {
                        run.pnl_peak = run.pnl_total;
                    }
                    if run.pnl_total < run.pnl_trough {
                        run.pnl_trough = run.pnl_total;
                    }
                }
                Outcome::Applied {
                    went_flat: !run.requires_management(),
                    entry_applied,
                    strategy_id,
                }
            })
            .unwrap_or(Outcome::NoState);
        for w in warnings {
            tracing::warn!("{}", w);
        }

        let (went_flat, entry_applied, strategy_id) = match outcome {
            Outcome::NoState => {
                // A late terminal update after another worker finalised:
                // reconcile from the durable rows instead.
                if !is_entry {
                    self.reconcile_run_pnl(run_id);
                }
                return false;
            }
            Outcome::Ignored => return false,
            Outcome::Applied {
                went_flat,
                entry_applied,
                strategy_id,
            } => (went_flat, entry_applied, strategy_id),
        };

        if entry_applied {
            // The entry may have filled after a stop reported it unfilled.
            self.reconcile_pending_stop(run_id).await;
            return false;
        }
        if !went_flat {
            return false;
        }
        let Ok(Some(run_row)) = self.store.get_run(run_id) else {
            return true;
        };
        if run_row.stopped_at.is_some() {
            return true;
        }
        let strategy = self.store.get_strategy_unscoped(strategy_id).ok().flatten();
        let user_id = strategy
            .as_ref()
            .map(|s| s.user_id.clone())
            .unwrap_or_default();
        let kind = strategy.as_ref().map(|s| s.strategy_kind.clone());
        // A signal run is a trading day: a leg exiting does not end it.
        if run_row.stop_requested_reason.is_none() && kind.as_deref() == Some("signal") {
            return true;
        }
        let (reason, message) = match &run_row.stop_requested_reason {
            Some(r) => (r.clone(), format!("Run stopped ({})", r)),
            None => ("manual".to_string(), "All legs closed".to_string()),
        };
        self.finalise(run_id, strategy_id, &user_id, &reason, &message)
            .await;
        true
    }

    /// Recompute provable realized P&L from durable owner facts (FIFO per
    /// exact position reference). Leaves the row alone when ownership is
    /// ambiguous or no priced round trip exists.
    pub fn reconcile_run_pnl(&self, run_id: i64) -> Option<f64> {
        let orders = self.store.list_orders(run_id).ok()?;
        let mut owners: std::collections::BTreeMap<
            (i64, Option<String>),
            Vec<&crate::strategy::store::OrderRow>,
        > = Default::default();
        for o in &orders {
            owners
                .entry((o.leg_id, o.position_ref.clone()))
                .or_default()
                .push(o);
        }
        let mut realized = 0.0;
        let mut settled = 0;
        for facts in owners.values() {
            let mut lots: Vec<(String, f64, i64)> = Vec::new();
            for o in facts {
                let status = o.status.to_ascii_lowercase();
                let qty = match status.as_str() {
                    "complete" => o.filled_qty.filter(|q| *q > 0).unwrap_or(o.qty),
                    "cancelled" | "rejected" => o.filled_qty.unwrap_or(0),
                    _ => 0,
                };
                if qty <= 0 {
                    continue;
                }
                let Some(price) = o.avg_fill_price.filter(|p| *p > 0.0) else {
                    lots.clear();
                    break;
                };
                if o.kind == "entry" {
                    lots.push((o.action.to_ascii_uppercase(), price, qty));
                    continue;
                }
                let mut left = qty;
                let mut matched = false;
                for lot in lots.iter_mut().filter(|l| l.2 > 0) {
                    let expected = if lot.0 == "BUY" { "SELL" } else { "BUY" };
                    if !o.action.eq_ignore_ascii_case(expected) {
                        return None;
                    }
                    let applied = left.min(lot.2);
                    let sign = if lot.0 == "BUY" { 1.0 } else { -1.0 };
                    realized += (price - lot.1) * applied as f64 * sign;
                    lot.2 -= applied;
                    left -= applied;
                    matched = true;
                    if left <= 0 {
                        break;
                    }
                }
                if matched {
                    settled += 1;
                }
            }
        }
        if settled == 0 {
            return None;
        }
        let realized = (realized * 100.0).round() / 100.0;
        let _ = self.store.execute_raw(&format!(
            "UPDATE sm_strategy_run SET pnl_realized = {} WHERE id = {}",
            realized, run_id
        ));
        Some(realized)
    }
}
