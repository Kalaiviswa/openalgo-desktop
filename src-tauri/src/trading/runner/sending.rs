//! The steps of sending one order for a run (`Runner::send`): its row is
//! written before it goes out, the destination's answer is kept (three
//! outcomes, LOG-08), the run's log says what happened, and the page hears
//! what became of its intent.

use super::inbox::Message;
use super::orders::{self, Placement, Target};
use super::{store, RunEntry, RunInfo, Runner, Unsaved};
use crate::strategy::dispatch::{DispatchResult, RunMode};

impl Runner {
    /// Write the order's row before it goes out, so a crash while it is at
    /// the destination still leaves a record to settle.
    pub(super) fn record_sent(
        &self,
        info: &RunInfo,
        target: &Target<'_>,
        p: &Placement,
        internal: bool,
    ) -> Result<i64, String> {
        let conn = self.db.conn().map_err(|e| e.to_string())?;
        store::insert_order(
            &conn,
            &store::NewOrder {
                deployment: info.run_id.clone(),
                mode: info.mode.as_str().into(),
                intent_id: (p.intent_id >= 0).then_some(p.intent_id),
                tag: p.tag.clone(),
                symbol: target.symbol.into(),
                exchange: target.exchange.into(),
                action: p.action.into(),
                quantity: p.quantity,
                pricetype: p.pricetype.into(),
                price: p.price,
                trigger_price: p.trigger_price,
                product: target.product.into(),
                is_exit: internal,
            },
            self.clock.now(),
        )
        .map_err(|e| e.to_string())
    }

    /// Keep the destination's answer: it is what lets this run follow the
    /// order. Unsaved, the row has no id and its fill would never be
    /// counted, so the answer is held on the run and retried, and nothing
    /// new is sent meanwhile.
    pub(super) async fn keep_answer(&self, info: &RunInfo, entry: Option<&RunEntry>, ack: Unsaved) {
        if self.save_ack(&ack).await {
            return;
        }
        self.write_log(
            info,
            &[format!(
                "The answer for order {} could not be saved, so nothing new is sent and Stop waits until it is.",
                ack.orderid.as_deref().unwrap_or("without a number")
            )],
        );
        if let Some(e) = entry {
            e.unsaved.lock().push(ack);
        }
    }

    /// Tell the page what became of its intent.
    pub(super) fn tell_page(
        &self,
        e: &RunEntry,
        p: &Placement,
        status: &'static str,
        result: &DispatchResult,
    ) {
        // The engine's own word for an order sent and not yet acknowledged:
        // the intent stays pending, never rejected.
        let word = if status == store::UNCONFIRMED {
            "placed"
        } else {
            status
        };
        e.inbox.push(Message::Frame {
            intent_id: p.intent_id,
            frame: orders::frame(
                p.intent_id,
                word,
                0,
                None,
                result.broker_order_id.as_deref(),
                result.error.as_deref(),
                self.clock.now().timestamp_millis(),
            ),
        });
    }
}

/// The row's status for the destination's answer. Three outcomes (LOG-08):
/// an answer that is neither an acceptance nor a refusal means the order
/// may be at the destination. Its row is unconfirmed, never rejected: it is
/// not sent again, not counted as flat, and the order book settles it.
pub(super) fn row_status(result: &DispatchResult) -> &'static str {
    if result.ok {
        "working"
    } else if result.uncertain {
        store::UNCONFIRMED
    } else {
        "rejected"
    }
}

/// The run's log line for one order sent.
pub(super) fn sent_line(
    info: &RunInfo,
    target: &Target<'_>,
    p: &Placement,
    result: &DispatchResult,
) -> String {
    if result.ok {
        format!(
            "Sent {} {} {} as {} {} on the {} side. Order {}.",
            p.action,
            p.quantity,
            target.symbol,
            p.pricetype,
            target.product,
            info.mode.as_str(),
            result.broker_order_id.as_deref().unwrap_or("not numbered")
        )
    } else if result.uncertain {
        format!(
            "The {} did not confirm {} {} {} ({}). It may have been placed, so it is not sent again, and nothing new is sent, until the order book settles it.",
            if info.mode == RunMode::Live {
                "broker"
            } else {
                "sandbox"
            },
            p.action,
            p.quantity,
            target.symbol,
            result
                .error
                .as_deref()
                .unwrap_or("no answer came back")
                .trim_end_matches('.')
        )
    } else {
        format!(
            "An order was not sent: {}",
            result.error.as_deref().unwrap_or("it was refused")
        )
    }
}
