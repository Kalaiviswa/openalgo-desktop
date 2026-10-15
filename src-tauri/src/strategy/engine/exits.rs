//! Exiting legs at market from the symbol each leg holds, and folding the
//! broker's facts for working entries and exits (web `engine.py`
//! `_exit_legs`).

use super::{ev, StrategyModule, ORDER_UNCONFIRMED, UNFILLED_EXIT};
use crate::strategy::dispatch::{build_order, exit_action, RunMode, EXIT_PRICETYPE};
use crate::strategy::state::ClaimId;
use crate::strategy::store::{NewOrder, StrategyRow};
use serde_json::{json, Value};

impl StrategyModule {
    /// Exit the named legs at market, from the symbol each leg HOLDS.
    pub(crate) async fn exit_legs(
        &self,
        run_id: i64,
        strategy: &StrategyRow,
        leg_ids: &[i64],
        kind: &str,
        mode: RunMode,
        user_id: &str,
    ) -> Vec<Value> {
        // Claimed and classified in ONE hold of the run lock.
        let (live, unfilled) = self.state.claim_legs_for_exit(run_id, leg_ids, kind);
        // A live claim carries the whole leg state: boxed, so the two kinds
        // of target are the same size.
        enum Target {
            Live(Box<crate::strategy::state::ExitClaim>),
            Superseded(crate::strategy::state::SupersededClaim),
        }
        let mut targets: Vec<Target> = live
            .into_iter()
            .map(|c| Target::Live(Box::new(c)))
            .collect();
        for leg_id in leg_ids {
            let position = self
                .state
                .with_run(run_id, |r| {
                    r.leg(*leg_id)
                        .and_then(|l| l.superseded.as_ref().map(|s| s.position.clone()))
                })
                .flatten();
            if let Some(p) = position {
                if let Some(c) = self.state.claim_superseded_exit(run_id, *leg_id, &p) {
                    targets.push(Target::Superseded(c));
                }
            }
        }

        let mut outcomes = Vec::new();
        for target in targets {
            let (lid, position, symbol, exchange, quantity, position_ref, token, owner) =
                match &target {
                    Target::Live(c) => (
                        c.leg.leg_id,
                        c.leg.position.clone(),
                        c.leg.symbol.clone(),
                        c.leg.exchange.clone(),
                        c.leg.qty,
                        c.leg.position_ref.clone(),
                        c.claim_token.clone(),
                        "live",
                    ),
                    Target::Superseded(c) => (
                        c.leg_id,
                        c.position.clone(),
                        c.symbol.clone(),
                        c.exchange.clone(),
                        c.quantity,
                        c.position_ref.clone(),
                        c.claim_token.clone(),
                        "superseded",
                    ),
                };
            let action = match exit_action(&position) {
                Ok(a) => a,
                Err(e) => {
                    tracing::error!("Run {} leg {}: {}", run_id, lid, e);
                    outcomes.push(json!({"leg_id": lid, "ok": false, "error": e,
                        "position_ref": position_ref, "exit_owner": owner}));
                    continue;
                }
            };
            let order = build_order(
                &symbol,
                &exchange,
                action,
                quantity,
                &strategy.product,
                &strategy.name,
                EXIT_PRICETYPE,
            );
            let row = self.store.record_order(
                run_id,
                lid,
                kind,
                &NewOrder {
                    symbol: symbol.clone(),
                    exchange: exchange.clone(),
                    action: action.into(),
                    qty: quantity,
                    product: Some(order.product.clone()),
                    pricetype: EXIT_PRICETYPE.into(),
                    status: "pending".into(),
                    position_ref: position_ref.clone(),
                    broker_order_id: None,
                },
            );
            let row_id = match row {
                Ok(id) => Some(id),
                Err(e) => {
                    // Not sent (P2-6): an exit with no row cannot be found
                    // again if its answer is lost, so it could be sent twice.
                    // The claim is released and the leg stays managed: the
                    // next breach, stop retry or close tries again.
                    tracing::error!("Exit row for run {} leg {} not written: {}", run_id, lid, e);
                    let claim = ClaimId::Token(token.clone());
                    if owner == "superseded" {
                        self.state.release_superseded_exit(run_id, lid, &claim);
                    } else {
                        self.state.release_leg_exit(run_id, lid, &claim);
                    }
                    self.emit(
                        strategy.id,
                        user_id,
                        "leg_exit_rejected",
                        &format!(
                            "Exit for leg {} was not sent: its order could not be recorded. The leg stays managed and the exit is tried again; check the disk and the app's data folder.",
                            lid
                        ),
                        ev(Some(run_id), Some(lid), "critical"),
                    )
                    .await;
                    outcomes.push(json!({"leg_id": lid, "ok": false,
                        "position_ref": position_ref, "exit_owner": owner,
                        "error": "The exit could not be recorded, so it was not sent"}));
                    continue;
                }
            };
            let mut claim = ClaimId::Token(token.clone());
            if let Some(row_id) = row_id {
                let bound = match &target {
                    Target::Superseded(_) => {
                        self.state.bind_superseded_exit(run_id, lid, &token, row_id)
                    }
                    Target::Live(_) => self.state.bind_live_exit(
                        run_id,
                        lid,
                        &token,
                        row_id,
                        position_ref.as_deref(),
                    ),
                };
                if !bound {
                    let reason = if owner == "superseded" {
                        "Outgoing position exit claim changed before dispatch"
                    } else {
                        "Live position exit claim changed before dispatch"
                    };
                    let _ = self
                        .store
                        .update_order(row_id, Some("rejected"), None, Some(reason));
                    if owner == "superseded" {
                        self.state.release_superseded_exit(run_id, lid, &claim);
                    } else {
                        self.state.release_leg_exit(run_id, lid, &claim);
                    }
                    self.emit(
                        strategy.id,
                        user_id,
                        "leg_exit_rejected",
                        &format!(
                            "{} exit claim changed on leg {} before dispatch",
                            if owner == "superseded" {
                                "Superseded"
                            } else {
                                "Live"
                            },
                            lid
                        ),
                        ev(Some(run_id), Some(lid), "critical"),
                    )
                    .await;
                    outcomes.push(json!({"leg_id": lid, "ok": false,
                        "position_ref": position_ref, "exit_owner": owner,
                        "error": format!("The {} position changed before its exit could be placed", owner)}));
                    continue;
                }
                claim = ClaimId::Row(row_id);
            }

            let result = self.gateway.place(mode, &order).await;
            if let Some(row_id) = row_id {
                self.record_acknowledgement(row_id, &result, strategy.id, user_id, run_id, lid)
                    .await;
            }
            if result.uncertain {
                // The exit may be at the broker: the claim stays with its row
                // so no tick, stop retry or close sends a second one. Only the
                // order reconciler releases it (LOG-08).
                if row_id.is_none() {
                    // No row to find it by: say so, and keep the leg claimed.
                    self.emit(
                        strategy.id,
                        user_id,
                        ORDER_UNCONFIRMED,
                        &format!(
                            "Exit {} {} {} for leg {} was sent without an order row, and the broker did not confirm it. Check the order book; the leg is not exited again automatically.",
                            action, quantity, symbol, lid
                        ),
                        ev(Some(run_id), Some(lid), "critical"),
                    )
                    .await;
                }
                outcomes.push(json!({
                    "leg_id": lid,
                    "ok": false,
                    "unconfirmed": true,
                    "error": result.error,
                    "position_ref": position_ref,
                    "exit_owner": owner,
                }));
                continue;
            }
            if !result.ok {
                // Release the claim so a later attempt is not mistaken for a
                // duplicate and the leg skipped for the rest of the session.
                if owner == "superseded" {
                    if self.state.release_superseded_exit(run_id, lid, &claim) {
                        self.report_flip_outgoing_exit_rejected(
                            run_id,
                            lid,
                            "refused",
                            result.broker_order_id.as_deref(),
                        )
                        .await;
                    }
                } else {
                    self.state.release_leg_exit(run_id, lid, &claim);
                }
            }
            self.emit(
                strategy.id,
                user_id,
                if result.ok {
                    "leg_exit_placed"
                } else {
                    "leg_exit_rejected"
                },
                &if result.ok {
                    format!("Exit {} {} {} placed ({})", action, quantity, symbol, kind)
                } else {
                    format!(
                        "Exit rejected on leg {}: {}",
                        lid,
                        result.error.clone().unwrap_or_default()
                    )
                },
                ev(
                    Some(run_id),
                    Some(lid),
                    if result.ok { "info" } else { "critical" },
                ),
            )
            .await;
            if result.ok && row_id.is_some() {
                self.replay_for(result.broker_order_id.as_deref()).await;
            }
            outcomes.push(json!({
                "leg_id": lid,
                "ok": result.ok,
                "error": result.error,
                "position_ref": position_ref,
                "exit_owner": owner,
            }));
        }
        for leg in unfilled {
            outcomes.push(json!({
                "leg_id": leg.leg_id,
                "ok": false,
                "symbol": leg.symbol,
                "broker_order_id": null,
                "position_ref": leg.position_ref,
                "exit_owner": "live",
                "error": UNFILLED_EXIT,
            }));
        }
        outcomes
    }

    pub(crate) async fn report_flip_outgoing_exit_rejected(
        &self,
        run_id: i64,
        leg_id: i64,
        ended: &str,
        broker_order_id: Option<&str>,
    ) {
        let Ok(Some(run)) = self.store.get_run(run_id) else {
            return;
        };
        if run.stopped_at.is_some() {
            return;
        }
        let Ok(Some(strategy)) = self.store.get_strategy_unscoped(run.strategy_id) else {
            return;
        };
        let order = broker_order_id
            .map(|b| format!(" order {}", b))
            .unwrap_or_default();
        self.emit(
            run.strategy_id,
            &strategy.user_id,
            "flip_outgoing_exit_rejected",
            &format!(
                "Outgoing exit{} for leg {} was {}. The old side is still held, remains managed, and can be closed again.",
                order, leg_id, ended
            ),
            ev(Some(run_id), Some(leg_id), "critical"),
        )
        .await;
    }

    /// Cancel each accepted working entry through the run's pipe, then fold
    /// one broker status fact. Cancellation is intent, never proof.
    pub(super) async fn cancel_and_reconcile_working_entries(&self, run_id: i64, mode: RunMode) {
        let working = [
            "pending",
            "open",
            "working",
            "trigger pending",
            "trigger_pending",
        ];
        let rows = self.store.list_orders(run_id).unwrap_or_default();
        for row in rows.iter().filter(|r| {
            r.kind == "entry" && working.contains(&r.status.trim().to_ascii_lowercase().as_str())
        }) {
            let Some(bid) = row.broker_order_id.clone().filter(|b| !b.is_empty()) else {
                continue;
            };
            let c = self.gateway.cancel(mode, &bid).await;
            if !c.ok {
                tracing::warn!(
                    "Working entry {} on run {} could not be cancelled: {}",
                    bid,
                    run_id,
                    c.error.unwrap_or_default()
                );
            }
            let status = self.gateway.order_status(mode, &bid).await;
            if status.ok {
                self.apply_order_snapshot(&bid, &status.order).await;
            }
        }
    }

    /// Fold one broker fact for each accepted exit still awaiting a frame.
    pub(super) async fn reconcile_working_exits(&self, run_id: i64, mode: RunMode) {
        let working = [
            "pending",
            "open",
            "working",
            "trigger pending",
            "trigger_pending",
        ];
        let rows = self.store.list_orders(run_id).unwrap_or_default();
        for row in rows.iter().filter(|r| {
            r.kind != "entry" && working.contains(&r.status.trim().to_ascii_lowercase().as_str())
        }) {
            let Some(bid) = row.broker_order_id.clone().filter(|b| !b.is_empty()) else {
                continue;
            };
            let status = self.gateway.order_status(mode, &bid).await;
            if status.ok {
                self.apply_order_snapshot(&bid, &status.order).await;
            }
        }
    }
}
