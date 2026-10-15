//! The webhook pipeline (web `webhook_bridge.py`): token, kill switch,
//! address, payload, action, mode, the live gate, the dedupe and cooling
//! windows, then dispatch, each refusal audited.

use super::{
    digest_matches, ip_allowed, looks_like_token, parse_payload, redact, Audit, StrategyModule,
    WebhookOutcome, COOLING_OFF, DEDUPE_WINDOW, MAX_TRACKED_KEYS,
};
use crate::strategy::store::{StrategyRow, RUN_MODES};
use serde_json::Value;

impl StrategyModule {
    /// Run one inbound alert through the pipeline and act on it, or refuse
    /// it. Never fails: an unexpected error is `rejected_engine_error`.
    pub async fn handle_webhook(
        &self,
        token: &str,
        body: &[u8],
        ip: Option<&str>,
        ua: Option<&str>,
    ) -> WebhookOutcome {
        let audit = Audit {
            module: self,
            ip,
            ua,
        };
        // 1. Token: shape first (no lookup for a non-token), then the digest.
        if !looks_like_token(token) {
            return self.unknown_token_outcome(ip, ua);
        }
        let strategy = match self.store.get_strategy_by_webhook_token(token) {
            // The row was found through the digest index; confirm the digest
            // in constant time so the comparison leaks nothing about it.
            Ok(Some(s)) if digest_matches(token, &s.webhook_token_hash) => s,
            Ok(_) => return self.unknown_token_outcome(ip, ua),
            Err(e) => {
                tracing::error!("Could not resolve a webhook token: {}", e);
                return self.unknown_token_outcome(ip, ua);
            }
        };
        let sid = strategy.id;
        let with = |mut o: WebhookOutcome, id: Option<i64>| {
            o.strategy_id = Some(sid);
            o.webhook_event_id = id;
            o
        };

        // 2. Kill switch, ahead of everything a caller controls.
        if strategy.webhook_locked {
            let id = audit.write(
                "rejected_locked",
                Some(sid),
                None,
                None,
                None,
                Some("The webhook kill switch is engaged"),
            );
            return with(
                WebhookOutcome::new(
                    "rejected_locked",
                    "This strategy's webhook is locked",
                    false,
                ),
                id,
            );
        }
        // 3. Address, before the body is parsed.
        if !ip_allowed(ip, &strategy.webhook_ip_allowlist) {
            let id = audit.write(
                "rejected_ip",
                Some(sid),
                None,
                None,
                None,
                Some("The caller address is outside the allowlist"),
            );
            // A failed authentication on this webhook, counted per webhook
            // whatever the source address, so rotating addresses locks it.
            if self.webhook.record_webhook_failure(sid) {
                self.lock_webhook_after_failures(&strategy).await;
            }
            return with(
                WebhookOutcome::new(
                    "rejected_ip",
                    "This address is not allowed to trigger this strategy",
                    false,
                ),
                id,
            );
        }
        // 4. Payload.
        let payload = match parse_payload(body) {
            Ok(p) => p,
            Err(e) => {
                let id = audit.write("rejected_payload", Some(sid), None, None, None, Some(&e));
                return with(WebhookOutcome::new("rejected_payload", e, false), id);
            }
        };
        let safe = redact(&Value::Object(payload.clone()), token, 0);

        // 5. Action, by kind.
        let allowed = crate::strategy::signals::actions_for(&strategy.strategy_kind);
        let raw_action = payload.get("action");
        let action = raw_action
            .and_then(Value::as_str)
            .map(|a| a.trim().to_ascii_lowercase());
        let Some(action) = action.filter(|a| allowed.contains(&a.as_str())) else {
            let shown = match raw_action {
                Some(Value::String(s)) => Some(s.trim().to_ascii_lowercase()),
                Some(Value::Null) | None => None,
                Some(other) => Some(other.to_string()),
            };
            let id = audit.write(
                "rejected_invalid_action",
                Some(sid),
                shown.as_deref(),
                None,
                Some(&safe),
                Some("Unrecognised action"),
            );
            return with(
                WebhookOutcome::new(
                    "rejected_invalid_action",
                    format!("'action' must be one of {}", allowed.join(", ")),
                    false,
                ),
                id,
            );
        };

        // 5b. Signal mode branches out before the batch-only stages.
        if crate::strategy::signals::SIGNAL_ACTIONS.contains(&action.as_str()) {
            let leg_id = payload.get("leg_id");
            let symbol = payload.get("symbol").and_then(Value::as_str);
            let exchange = payload.get("exchange").and_then(Value::as_str);
            let r = self
                .handle_signal(&strategy, &action, leg_id, symbol, exchange)
                .await;
            if !r.ok {
                let err = r
                    .error
                    .clone()
                    .unwrap_or_else(|| "The signal was refused".into());
                let id = audit.write(
                    "rejected_invalid_action",
                    Some(sid),
                    Some(&action),
                    None,
                    Some(&safe),
                    Some(&err),
                );
                return with(
                    WebhookOutcome::new("rejected_invalid_action", err, false),
                    id,
                );
            }
            let id = audit.write("ok", Some(sid), Some(&action), None, Some(&safe), None);
            let message = match &r.note {
                Some(n) => format!("Signal accepted ({})", n),
                None => "Signal accepted".into(),
            };
            let mut o = with(WebhookOutcome::new("ok", message, true), id);
            o.run_id = r.run_id;
            return o;
        }

        // 6. Mode, required by start, ignored by stop.
        let mut mode: Option<String> = None;
        if action == "start" {
            let raw = payload.get("mode");
            let m = raw
                .and_then(Value::as_str)
                .map(|m| m.trim().to_ascii_lowercase());
            match m.filter(|m| RUN_MODES.contains(&m.as_str())) {
                Some(m) => mode = Some(m),
                None => {
                    let shown = match raw {
                        Some(Value::String(s)) => Some(s.trim().to_ascii_lowercase()),
                        Some(Value::Null) | None => None,
                        Some(o) => Some(o.to_string()),
                    };
                    let id = audit.write(
                        "rejected_invalid_action",
                        Some(sid),
                        Some(&action),
                        shown.as_deref(),
                        Some(&safe),
                        Some("Unrecognised mode"),
                    );
                    return with(
                        WebhookOutcome::new(
                            "rejected_invalid_action",
                            format!(
                                "'mode' must be one of {} when starting",
                                RUN_MODES.join(", ")
                            ),
                            false,
                        ),
                        id,
                    );
                }
            }
            // 7. The live gate.
            if mode.as_deref() == Some("live") && !strategy.live_enabled {
                let id = audit.write(
                    "rejected_live_disabled",
                    Some(sid),
                    Some(&action),
                    mode.as_deref(),
                    Some(&safe),
                    Some("Live trading is not enabled for this strategy"),
                );
                return with(
                    WebhookOutcome::new(
                        "rejected_live_disabled",
                        "Live trading is not enabled for this strategy",
                        false,
                    ),
                    id,
                );
            }
        }

        // 8 and 9: both windows read and the claim written under one lock.
        let key = (sid, action.clone(), mode.clone());
        let now = self.webhook.now();
        let refusal = {
            let mut dedupe = self.webhook.dedupe.lock();
            dedupe.retain(|_, at| now.saturating_duration_since(*at) < DEDUPE_WINDOW);
            if dedupe.len() >= MAX_TRACKED_KEYS {
                dedupe.clear();
            }
            let cooling = if action == "start" {
                self.webhook.cooling_remaining(sid, now)
            } else {
                0
            };
            if dedupe.contains_key(&key) {
                Some(("rejected_dedupe", 0))
            } else if cooling > 0 {
                Some(("rejected_cooling_off", cooling))
            } else {
                dedupe.insert(key.clone(), now);
                None
            }
        };
        match refusal {
            Some(("rejected_dedupe", _)) => {
                let id = audit.write(
                    "rejected_dedupe",
                    Some(sid),
                    Some(&action),
                    mode.as_deref(),
                    Some(&safe),
                    Some(&format!(
                        "Duplicate signal inside {}s",
                        DEDUPE_WINDOW.as_secs()
                    )),
                );
                return with(
                    WebhookOutcome::new(
                        "rejected_dedupe",
                        format!(
                            "Duplicate signal ignored, already handled within {}s",
                            DEDUPE_WINDOW.as_secs()
                        ),
                        true,
                    ),
                    id,
                );
            }
            Some((_, remaining)) => {
                let id = audit.write(
                    "rejected_cooling_off",
                    Some(sid),
                    Some(&action),
                    mode.as_deref(),
                    Some(&safe),
                    Some(&format!(
                        "Stopped within the last {}s",
                        COOLING_OFF.as_secs()
                    )),
                );
                return with(
                    WebhookOutcome::new(
                        "rejected_cooling_off",
                        format!(
                            "This strategy stopped recently, try again in {}s",
                            remaining
                        ),
                        false,
                    ),
                    id,
                );
            }
            None => {}
        }

        // 10. Dispatch. The accepted row first, so the run can point at it.
        let event_id = audit.write(
            "ok",
            Some(sid),
            Some(&action),
            mode.as_deref(),
            Some(&safe),
            None,
        );
        let was_live = strategy.status != "stopped";
        let (ok, run_id, error, stop_pending, exits) = if action == "start" {
            let r = self
                .start_run(
                    sid,
                    &strategy.user_id,
                    mode.as_deref().unwrap_or("sandbox"),
                    "webhook",
                    event_id,
                )
                .await;
            (r.ok, r.run_id, r.error, None, vec![])
        } else {
            self.bridge_stop(&strategy).await
        };
        if !ok {
            // Release the claim so the sender's retry is not swallowed.
            self.webhook.dedupe.lock().remove(&key);
            let err = error.unwrap_or_else(|| "The engine could not act on the signal".into());
            let id = audit.write(
                "rejected_engine_error",
                Some(sid),
                Some(&action),
                mode.as_deref(),
                Some(&safe),
                Some(&err),
            );
            let mut o = with(WebhookOutcome::new("rejected_engine_error", err, false), id);
            o.run_id = run_id;
            o.stop_pending = stop_pending;
            o.exits = exits;
            return o;
        }
        if action == "stop" && was_live && stop_pending != Some(true) {
            self.webhook.note_run_stopped(sid, now);
        }
        let mut o = with(
            WebhookOutcome::new("ok", format!("Strategy {} accepted", action), true),
            event_id,
        );
        o.run_id = run_id;
        o.stop_pending = stop_pending;
        o.exits = exits;
        o
    }

    /// Web `webhook_bridge.stop_run`: a stop applies to the current run, and
    /// stopping an already-flat strategy is a success.
    async fn bridge_stop(
        &self,
        strategy: &StrategyRow,
    ) -> (bool, Option<i64>, Option<String>, Option<bool>, Vec<Value>) {
        let current = self
            .store
            .get_strategy_unscoped(strategy.id)
            .ok()
            .flatten()
            .and_then(|s| s.current_run_id);
        let Some(run_id) = current else {
            return (true, None, None, None, vec![]);
        };
        let r = self.stop_run(run_id, &strategy.user_id, "manual").await;
        (r.ok, Some(run_id), r.error, Some(r.stop_pending), r.exits)
    }
}
