//! A page that sends a batch of intents again (NEW-TR-04).
//!
//! A page's request can reach the runner while its answer never reaches the
//! page: the connection dropped, the reply could not be read. That is not a
//! refusal, so the page keeps such a batch pending and sends it again,
//! unchanged, until it hears back (`src/runner/driver.ts`). Here an intent
//! that was answered already gets the same answer again, before any check
//! that might refuse it now, and is never placed twice: one that became an
//! order row is answered from that row (`(incarnation, intent_id)` is the
//! durable key, `store::identity`), one refused or cancelled without a row
//! from the answer kept on the run. A frame supersedes the one queued before
//! it for the same intent, so answering a placed intent "rejected" on a
//! repeat would lose the very order this protects.
//!
//! A batch naming another incarnation (a page of an earlier run, a request
//! replayed after a restart) places nothing: each intent is answered from
//! its record, or as not sent. Records are read only within the page's own
//! deployment: naming another deployment's run number reads none of it.

use super::inbox::Message;
use super::{orders, store, RunEntry, Runner};
use serde_json::{json, Value};

/// The most intents a run remembers the answer of without a row. A page
/// sends again only the batch it is waiting on, so this is ample; an older
/// answer forgotten is re-decided, never placed twice when it has a row.
const MAX_ANSWERED: usize = 1024;

/// What an intent of a run that is not running is told when it left no
/// row: nothing was sent for it.
const OTHER_RUN: &str = "this run of the strategy has ended, so it was not sent";

/// What a batch is told when its intents' records cannot be read: whether
/// they were sent is unknown, so the page keeps them pending and asks again.
const RECORDS_UNREADABLE: &str =
    "This strategy's orders could not be read just now, so nothing new was sent. The page asks again.";

/// An intent's frame from the row it became. An order with no definite
/// answer yet, or held for approval in the Action Center with no broker
/// order (LOG-04), is still pending to the engine, never rejected.
fn row_frame(row: &store::OrderRow, now_ms: i64) -> Value {
    let status = row.status.as_str();
    let held = row.orderid.is_none() && row.pending_order_id.is_some();
    let word = if status == store::UNCONFIRMED || status == "placed" || held {
        "placed"
    } else {
        status
    };
    orders::frame(
        row.intent_id.unwrap_or(-1),
        word,
        row.filled_quantity,
        row.average_price,
        row.orderid.as_deref(),
        row.message.as_deref().filter(|_| word == "rejected"),
        now_ms,
    )
}

fn intent_id(intent: &Value) -> Option<i64> {
    intent
        .get("intentId")
        .and_then(Value::as_i64)
        .filter(|id| *id >= 0)
}

impl Runner {
    /// Keep the answer an intent got without leaving a row: a refusal's
    /// frame, or `None` for a cancel done.
    pub(super) fn remember(&self, entry: &RunEntry, intent_id: i64, answer: Option<Value>) {
        if intent_id < 0 {
            return;
        }
        let mut kept = entry.answered.lock();
        kept.insert(intent_id, answer);
        while kept.len() > MAX_ANSWERED {
            kept.pop_first();
        }
    }

    /// Answer again each intent of the batch that was answered before, and
    /// answer the intents that are new. An error when the records cannot be
    /// read: nothing of the batch is sent then.
    pub(super) fn answer_repeats(
        &self,
        entry: &RunEntry,
        intents: &[Value],
    ) -> Result<Vec<Value>, String> {
        let conn = self.db.conn().map_err(|e| {
            tracing::error!("Could not read the orders of {}: {}", entry.info.run_id, e);
            RECORDS_UNREADABLE.to_string()
        })?;
        let now_ms = self.clock.now().timestamp_millis();
        let mut fresh = Vec::new();
        let mut repeated = Vec::new();
        for intent in intents {
            let Some(id) = intent_id(intent) else {
                fresh.push(intent.clone());
                continue;
            };
            let row = store::order_of_intent(&conn, &entry.info.run_id, entry.info.incarnation, id)
                .map_err(|e| {
                    tracing::error!("Could not read the orders of {}: {}", entry.info.run_id, e);
                    RECORDS_UNREADABLE.to_string()
                })?;
            if let Some(row) = row {
                self.frame_to_page(entry, &row, row_frame(&row, now_ms));
                repeated.push(id.to_string());
                continue;
            }
            let earlier = entry.answered.lock().get(&id).cloned();
            match earlier {
                Some(answer) => {
                    if let Some(frame) = answer {
                        entry.inbox.push(Message::Frame {
                            intent_id: id,
                            frame,
                        });
                    }
                    repeated.push(id.to_string());
                }
                None => fresh.push(intent.clone()),
            }
        }
        if !repeated.is_empty() {
            self.write_log(
                &entry.info,
                &[format!(
                    "The strategy page asked again about intent {}, already handled; it was answered from its record and nothing was sent again.",
                    repeated.join(", ")
                )],
            );
        }
        Ok(fresh)
    }

    /// A batch of a run that is not this one: nothing is placed, and each
    /// intent is answered from its record.
    pub(super) fn answer_other_run(
        &self,
        entry: &RunEntry,
        incarnation: i64,
        intents: &[Value],
    ) -> Value {
        let now_ms = self.clock.now().timestamp_millis();
        let conn = match self.db.conn() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Could not read the orders of {}: {}", entry.info.run_id, e);
                return json!({"status": "error", "message": RECORDS_UNREADABLE});
            }
        };
        let mut outcomes = Vec::with_capacity(intents.len());
        for intent in intents {
            let id = intent_id(intent).unwrap_or(-1);
            // Scoped to this page's deployment: another deployment's run
            // number finds nothing and is answered "not sent".
            let row = match store::order_of_intent(&conn, &entry.info.run_id, incarnation, id) {
                Ok(row) => row,
                Err(e) => {
                    tracing::error!("Could not read the orders of {}: {}", entry.info.run_id, e);
                    return json!({"status": "error", "message": RECORDS_UNREADABLE});
                }
            };
            outcomes.push(match row {
                Some(row) => row_frame(&row, now_ms),
                None => orders::frame(id, "rejected", 0, None, None, Some(OTHER_RUN), now_ms),
            });
        }
        self.write_log(
            &entry.info,
            &[format!(
                "A strategy page of another run of this strategy asked about {} intent(s); each was answered from its record and nothing was sent.",
                intents.len()
            )],
        );
        json!({"status": "success", "sent": 0, "stale": true, "outcomes": outcomes})
    }
}
