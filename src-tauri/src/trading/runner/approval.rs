//! A Live run's orders that Semi-Auto order mode held for the trader's
//! approval in the Action Center (LOG-04).
//!
//! The placement's answer was the pending order's id, not a broker order,
//! so its row is open with no number and the run cannot poll it. On every
//! refresh the run reads the Action Center's own record of that order and
//! follows it:
//!
//! * **Placed** (approved, and the broker gave it a number): the row is
//!   bound to that order, so the run polls it, counts its fills and cancels
//!   it on Stop like any order it sent itself.
//! * **Refused** (rejected by the trader, or approved and refused by the
//!   broker): nothing was sent, so the row is rejected and the page told.
//! * **Unknown** (approved and sent with no definite answer, an acceptance
//!   with no number, or the record is gone): the order may be at the broker,
//!   so the row is unconfirmed and the order book settles it (LOG-08).
//! * **Waiting**: still pending, or approved and not sent yet.
//!
//! The web records an empty order id for such an order and never follows
//! it; following it is a desktop addition. The record is read, not pushed,
//! so a restart or a missed refresh loses nothing.

use super::inbox::Message;
use super::{orders, store, RunEntry, Runner};
use crate::db::sqlite::action_center::{self, PendingOrderRow, SUBMITTING};

/// What became of an order held for approval, from the Action Center's record.
#[derive(Debug, Clone, PartialEq)]
enum Approval {
    Waiting,
    Placed(String),
    Refused(String),
    Unknown(&'static str),
}

const SENT_WITHOUT_ANSWER: &str = "Approved in the Action Center and sent, but the broker gave no definite answer. It may have been placed, so it is not sent again; the order book settles it.";
const RECORD_GONE: &str = "Its Action Center record is gone, so whether it was placed is not known; the order book settles it.";

/// Read the Action Center's record of a held order.
fn approval_of(record: Option<&PendingOrderRow>) -> Approval {
    let Some(p) = record else {
        return Approval::Unknown(RECORD_GONE);
    };
    match p.status.as_str() {
        "pending" => Approval::Waiting,
        "rejected" => Approval::Refused(match p.rejected_reason.as_deref().map(str::trim) {
            Some(why) if !why.is_empty() => {
                format!("You rejected it in the Action Center: {}", why)
            }
            _ => "You rejected it in the Action Center".into(),
        }),
        "approved" => {
            let id = p
                .broker_order_id
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty());
            match (id, p.broker_status.as_deref()) {
                // Approved, not claimed for sending yet.
                (None, None) => Approval::Waiting,
                (None, Some("rejected")) => Approval::Refused(
                    "Approved in the Action Center, but the broker refused it, so it was not placed"
                        .into(),
                ),
                (Some(id), Some(status)) if status != SUBMITTING => {
                    Approval::Placed(id.to_string())
                }
                _ => Approval::Unknown(SENT_WITHOUT_ANSWER),
            }
        }
        _ => Approval::Unknown(RECORD_GONE),
    }
}

impl Runner {
    /// Follow the run's open rows held in the Action Center (see the module
    /// doc). Answers the open rows as they stand afterwards: a bound row has
    /// its number, a refused one is gone. A record that cannot be read
    /// leaves its row as it was, held, for the next refresh.
    pub(super) fn follow_approvals(
        &self,
        entry: &RunEntry,
        open: Vec<store::OrderRow>,
    ) -> Vec<store::OrderRow> {
        let held = |o: &store::OrderRow| o.orderid.is_none() && o.pending_order_id.is_some();
        if !open.iter().any(held) {
            return open;
        }
        let conn = match self.db.conn() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Could not read the Action Center orders of a run: {}", e);
                return open;
            }
        };
        let mut kept = Vec::with_capacity(open.len());
        for mut o in open {
            let Some(pending) = o.pending_order_id.filter(|_| held(&o)) else {
                kept.push(o);
                continue;
            };
            let record = match action_center::get(&conn, pending) {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("Could not read Action Center order {}: {}", pending, e);
                    kept.push(o);
                    continue;
                }
            };
            match approval_of(record.as_ref()) {
                Approval::Waiting => kept.push(o),
                Approval::Placed(orderid) => {
                    if self.bind_held(entry, &conn, &o, &orderid) {
                        o.orderid = Some(orderid);
                        o.status = "working".into();
                    }
                    kept.push(o);
                }
                Approval::Refused(why) => {
                    if !self.refuse_held(entry, &conn, &o, &why) {
                        kept.push(o);
                    }
                }
                Approval::Unknown(why) => {
                    self.unconfirm_held(entry, &conn, &mut o, why);
                    kept.push(o);
                }
            }
        }
        kept
    }

    /// Bind a held row to the order its approval placed; the run follows it
    /// from here like any order it sent.
    fn bind_held(
        &self,
        entry: &RunEntry,
        conn: &rusqlite::Connection,
        o: &store::OrderRow,
        orderid: &str,
    ) -> bool {
        match store::bind_approved(conn, o.id, orderid, self.clock.now()) {
            Ok(true) => {
                self.write_log(
                    &entry.info,
                    &[format!(
                        "{} {} {} was approved in the Action Center and placed as order {}. This run follows it.",
                        o.action, o.quantity, o.symbol, orderid
                    )],
                );
                self.frame_for(entry, o, "working", Some(orderid), None);
                true
            }
            Ok(false) => false,
            Err(e) => {
                tracing::warn!("Could not bind OpenScript order row {}: {}", o.id, e);
                false
            }
        }
    }

    /// A held order that was never placed: its row is rejected and the page
    /// told, so the script trades on. False when the row could not be
    /// written (it stays held).
    fn refuse_held(
        &self,
        entry: &RunEntry,
        conn: &rusqlite::Connection,
        o: &store::OrderRow,
        why: &str,
    ) -> bool {
        match store::refuse_held(conn, o.id, why, self.clock.now()) {
            Ok(changed) => {
                if changed {
                    self.write_log(
                        &entry.info,
                        &[format!(
                            "{} {} {} was not placed. {}.",
                            o.action, o.quantity, o.symbol, why
                        )],
                    );
                    self.frame_for(entry, o, "rejected", None, Some(why));
                }
                true
            }
            Err(e) => {
                tracing::warn!("Could not settle OpenScript order row {}: {}", o.id, e);
                false
            }
        }
    }

    /// A held order that may be at the broker with no number: unconfirmed,
    /// for the order book to settle (LOG-08). Nothing new is sent meanwhile.
    fn unconfirm_held(
        &self,
        entry: &RunEntry,
        conn: &rusqlite::Connection,
        o: &mut store::OrderRow,
        why: &str,
    ) {
        if o.status == store::UNCONFIRMED {
            return;
        }
        match store::unconfirmed(conn, o.id, None, Some(why), self.clock.now()) {
            Ok(()) => {
                o.status = store::UNCONFIRMED.into();
                self.write_log(
                    &entry.info,
                    &[format!("{} {} {}: {}", o.action, o.quantity, o.symbol, why)],
                );
                self.services().request_reconcile();
            }
            Err(e) => tracing::warn!("Could not mark OpenScript order row {}: {}", o.id, e),
        }
    }

    /// Tell the page what became of a held order's intent.
    fn frame_for(
        &self,
        entry: &RunEntry,
        o: &store::OrderRow,
        status: &str,
        orderid: Option<&str>,
        text: Option<&str>,
    ) {
        let Some(intent) = o.intent_id else {
            return;
        };
        entry.inbox.push(Message::Frame {
            intent_id: intent,
            frame: orders::frame(
                intent,
                status,
                0,
                None,
                orderid,
                text,
                self.clock.now().timestamp_millis(),
            ),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(status: &str, id: Option<&str>, broker_status: Option<&str>) -> PendingOrderRow {
        PendingOrderRow {
            id: 7,
            user_id: "u".into(),
            api_type: "placeorder".into(),
            order_data: "{}".into(),
            created_at: String::new(),
            created_at_ist: None,
            status: status.into(),
            approved_at: None,
            approved_at_ist: None,
            approved_by: None,
            rejected_at_ist: None,
            rejected_by: None,
            rejected_reason: None,
            broker_order_id: id.map(str::to_string),
            broker_status: broker_status.map(str::to_string),
            force_live: true,
        }
    }

    /// Every Action Center state a held order can be in, and what the run
    /// makes of it. Only a definite refusal releases the row; anything that
    /// may have reached the broker is unconfirmed (LOG-08).
    #[test]
    fn an_approval_is_followed_by_what_its_record_says() {
        let a = |r: PendingOrderRow| approval_of(Some(&r));
        assert_eq!(a(record("pending", None, None)), Approval::Waiting);
        assert_eq!(a(record("approved", None, None)), Approval::Waiting);
        assert_eq!(
            a(record("approved", Some("OID1"), Some("open"))),
            Approval::Placed("OID1".into())
        );
        assert_eq!(
            a(record("approved", Some("OID1"), Some("complete"))),
            Approval::Placed("OID1".into())
        );
        let mut rejected = record("rejected", None, None);
        rejected.rejected_reason = Some("too late".into());
        assert_eq!(
            a(rejected),
            Approval::Refused("You rejected it in the Action Center: too late".into())
        );
        assert!(matches!(
            a(record("approved", None, Some("rejected"))),
            Approval::Refused(_)
        ));
        for unknown in [
            record("approved", None, Some(SUBMITTING)),
            record("approved", None, Some("open")),
            record("approved", Some("OID1"), Some(SUBMITTING)),
            record("something else", None, None),
        ] {
            assert!(matches!(a(unknown), Approval::Unknown(_)));
        }
        assert_eq!(approval_of(None), Approval::Unknown(RECORD_GONE));
    }
}
