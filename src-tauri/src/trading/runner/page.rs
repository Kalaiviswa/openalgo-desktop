//! What the run's page asks of the runner: its spec, bars and inbox, log
//! lines, the end of a run, and the intents the strategy decided (each
//! checked against the run and sent as an order).

use super::inbox::Message;
use super::orders::{Planned, Target};
use super::{orders, store};
use super::{Refusal, RunEntry, Runner, MAX_LOG_LINES};
use chrono::Duration as ChronoDuration;
use chrono_tz::Asia::Kolkata;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

impl Runner {
    fn entry_for(&self, run_id: &str, token: &str) -> Option<Arc<RunEntry>> {
        let entry = self.reg.lock().runs.get(run_id).cloned()?;
        if !crate::server::form::tokens_match(token, &entry.token) {
            return None;
        }
        *entry.last_seen.lock() = Instant::now();
        Some(entry)
    }

    /// Whether this token belongs to this running run.
    pub fn authenticate(&self, run_id: &str, token: &str) -> bool {
        self.entry_for(run_id, token).is_some()
    }

    /// What the page needs to load the program.
    pub fn page_spec(&self, run_id: &str, token: &str) -> Option<Result<Value, Refusal>> {
        let entry = self.entry_for(run_id, token)?;
        let info = &entry.info;
        let Some(program) = self.scripts.program(&info.script) else {
            return Some(Err(Refusal::new(
                409,
                format!("{} has no compiled program any more.", info.script),
            )));
        };
        let program = String::from_utf8_lossy(&program).into_owned();
        let inputs = self
            .db
            .conn()
            .ok()
            .and_then(|c| store::read(&c, run_id).ok().flatten())
            .map(|d| Value::Object(d.inputs))
            .unwrap_or_else(|| json!({}));
        let today = self.clock.now().with_timezone(&Kolkata).date_naive();
        let facts = self
            .services()
            .instrument(&info.symbol, &info.exchange, today);
        Some(Ok(json!({
            "status": "success",
            "run": info.answer(),
            "program": program,
            "inputs": inputs,
            "facts": facts,
            "product": orders::product_for(&info.product),
            "lotSize": self.services().lot_size(&info.symbol, &info.exchange),
            "barPollMs": self.options.read().bar_poll.as_millis() as u64,
        })))
    }

    /// The history a run is replayed over; sets the anchor new bars follow.
    pub async fn page_bars(&self, run_id: &str, token: &str) -> Option<Result<Value, Refusal>> {
        let entry = self.entry_for(run_id, token)?;
        let info = &entry.info;
        let today = self.clock.now().with_timezone(&Kolkata).date_naive();
        let days = self.options.read().history_days.max(1);
        let start = today - ChronoDuration::days(days);
        let got = self
            .services()
            .history(&info.symbol, &info.exchange, &info.interval, start, today)
            .await;
        Some(match got {
            Ok(bars) => {
                if let Some(last) = bars.last() {
                    *entry.anchor.lock() = Some(last.time);
                    *entry.last_bar.lock() = Some(last.json());
                }
                Ok(json!({
                    "status": "success",
                    "data": bars.iter().map(|b| b.json()).collect::<Vec<_>>(),
                }))
            }
            Err(why) => Err(Refusal::new(
                502,
                if why.is_empty() {
                    "History could not be read just now.".to_string()
                } else {
                    why
                },
            )),
        })
    }

    /// The page's long poll.
    pub async fn page_inbox(&self, run_id: &str, token: &str, after: u64) -> Option<Value> {
        let entry = self.entry_for(run_id, token)?;
        let wait = self.options.read().inbox_wait;
        let (messages, closed) = entry.inbox.next(after, wait).await;
        *entry.last_seen.lock() = Instant::now();
        Some(json!({"status": "success", "messages": messages, "stop": closed}))
    }

    /// Lines the page writes into its run's log.
    pub fn page_log(&self, run_id: &str, token: &str, lines: &[String]) -> bool {
        let Some(entry) = self.entry_for(run_id, token) else {
            return false;
        };
        let kept: Vec<String> = lines
            .iter()
            .take(MAX_LOG_LINES)
            .map(|l| l.chars().take(2000).collect())
            .collect();
        self.write_log(&entry.info, &kept);
        true
    }

    /// The page says its script stopped.
    pub fn page_ended(&self, run_id: &str, token: &str, message: &str) -> bool {
        if self.entry_for(run_id, token).is_none() {
            return false;
        }
        let why: String = message.chars().take(500).collect();
        self.end_run(
            run_id,
            &format!(
                "Stopped: {}",
                if why.is_empty() {
                    "the script ended"
                } else {
                    &why
                }
            ),
        );
        true
    }

    /// Intents from a confirmed bar. Each answers with a frame in the inbox.
    pub async fn page_intents(
        &self,
        run_id: &str,
        token: &str,
        intents: &[Value],
    ) -> Option<Value> {
        let entry = self.entry_for(run_id, token)?;
        let info = entry.info.clone();
        let now_ms = || self.clock.now().timestamp_millis();
        let reject = |id: i64, text: &str| {
            entry.inbox.push(Message::Frame {
                intent_id: id,
                frame: orders::frame(id, "rejected", 0, None, None, Some(text), now_ms()),
            });
        };
        let _held = entry.order_lock.lock().await;
        if entry.halted.load(Ordering::SeqCst) {
            for i in intents {
                reject(
                    i.get("intentId").and_then(Value::as_i64).unwrap_or(-1),
                    "this run is stopping, so nothing further is sent",
                );
            }
            return Some(json!({"status": "success", "sent": 0, "halted": true}));
        }
        // An earlier order's answer not yet saved: its fill cannot be
        // counted, so a new order would be sized and sent blind.
        self.flush_unsaved(&entry);
        if !entry.unsaved.lock().is_empty() {
            for i in intents {
                reject(
                    i.get("intentId").and_then(Value::as_i64).unwrap_or(-1),
                    "an earlier order of this run could not be recorded yet, so nothing new is sent",
                );
            }
            return Some(json!({"status": "success", "sent": 0, "unsaved": true}));
        }
        // An order the destination did not confirm may be there: a new
        // order would be sized without it, so nothing is sent until the
        // order book settles it (LOG-08). A ledger that cannot be read is
        // treated the same way.
        let own = match self.read_own(&info) {
            Ok(rows) => rows,
            Err(_) => {
                for i in intents {
                    reject(
                        i.get("intentId").and_then(Value::as_i64).unwrap_or(-1),
                        "this run's own orders could not be read, so nothing new is sent",
                    );
                }
                return Some(json!({"status": "success", "sent": 0, "unconfirmed": true}));
            }
        };
        if own.iter().any(|o| {
            o.orderid.is_none() && (o.status == store::UNCONFIRMED || o.status == "placed")
        }) {
            for i in intents {
                reject(
                    i.get("intentId").and_then(Value::as_i64).unwrap_or(-1),
                    "an earlier order of this run was not confirmed, so nothing new is sent until the order book settles it",
                );
            }
            return Some(json!({"status": "success", "sent": 0, "unconfirmed": true}));
        }
        let services = self.services();
        let lot = services.lot_size(&info.symbol, &info.exchange);
        let planned: Vec<Planned> = intents.iter().map(|i| orders::plan(i, lot)).collect();
        if let Some(Planned::Unroutable { kind, .. }) = planned
            .iter()
            .find(|p| matches!(p, Planned::Unroutable { .. }))
        {
            for i in intents {
                reject(
                    i.get("intentId").and_then(Value::as_i64).unwrap_or(-1),
                    "this runner cannot send that order as one piece",
                );
            }
            let why = format!(
                "{} asked for an order this runner cannot send as one piece ({}). Nothing was sent for this bar and the run has stopped.",
                info.script, kind
            );
            drop(_held);
            self.end_run(run_id, &why);
            return Some(json!({"status": "success", "sent": 0, "stopped": true}));
        }
        let product = orders::product_for(&info.product);
        let target = Target {
            strategy: &info.run_id,
            symbol: &info.symbol,
            exchange: &info.exchange,
            product: &product,
        };
        // Order facts on this side may be missing after lost order updates
        // (ARCH-01): no new exposure until the order book has been read
        // again. An order that only reduces what this run holds still goes,
        // as a strategy's exits do; Stop is never held by this.
        let paused = services.entry_refusal(info.mode);
        let mut room = orders::ExitRoom::of(&own, &info.symbol, &info.exchange, &product);
        let mut sent = 0;
        for p in planned {
            match p {
                Planned::Place(placement) => {
                    if let Some(why) = paused.as_deref() {
                        if !room.take(placement.action, placement.quantity) {
                            self.write_log(
                                &info,
                                &[format!(
                                    "{} {} {} was not sent: {}",
                                    placement.action, placement.quantity, info.symbol, why
                                )],
                            );
                            reject(placement.intent_id, why);
                            continue;
                        }
                    }
                    match self
                        .send(&info, &target, &placement, Some(&entry), false)
                        .await
                    {
                        Ok((_, r)) => sent += usize::from(r.ok),
                        Err(e) => {
                            tracing::error!("Could not record an OpenScript order: {}", e);
                            reject(placement.intent_id, "the order could not be recorded");
                        }
                    }
                }
                Planned::Cancel { tag, .. } => {
                    let rows = match self.read_own(&info) {
                        Ok(rows) => rows,
                        Err(why) => {
                            self.write_log(
                                &info,
                                &[format!(
                                    "The orders tagged {} were not cancelled: {}",
                                    tag, why
                                )],
                            );
                            Vec::new()
                        }
                    };
                    for o in rows
                        .into_iter()
                        .filter(|o| o.tag == tag && !store::TERMINAL.contains(&o.status.as_str()))
                    {
                        if let Some(oid) = o.orderid.as_deref() {
                            let r = services.cancel(info.mode, oid).await;
                            self.write_log(
                                &info,
                                &[if r.ok {
                                    format!("Asked for order {} to be cancelled.", oid)
                                } else {
                                    format!(
                                        "Order {} could not be cancelled: {}",
                                        oid,
                                        r.error.unwrap_or_default()
                                    )
                                }],
                            );
                        }
                    }
                }
                Planned::Refuse { intent_id, reason } => {
                    self.write_log(&info, &[format!("An order was not sent: {}", reason)]);
                    reject(intent_id, &reason);
                }
                Planned::Unroutable { .. } => {}
            }
        }
        Some(json!({"status": "success", "sent": sent}))
    }
}
