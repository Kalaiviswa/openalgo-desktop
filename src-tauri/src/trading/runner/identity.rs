//! A run's own orders, and nobody else's (NEW-TR-01, NEW-TR-03).
//!
//! **Destination.** A deployment keeps its id from run to run, and a Pause
//! lets the trader switch brokers. Its rows are therefore bound to where
//! they went (`store::identity`): a run's ledger is the rows its destination
//! owns, and every order, cancel, status read and book read of the run goes
//! through the session connected now only when that is the run's own
//! ([`Runner::session_refusal`]). A run started on broker B never counts,
//! cancels or closes what broker A holds; that stays with A, closeable by a
//! run on A, and the trader is told about it.
//!
//! **Incarnation.** Each start is a new engine whose intents are numbered
//! from 1 again. A frame reaches a page only for a row of that page's own
//! incarnation ([`Runner::frame_to_page`]), so an earlier run's order, filled
//! or cancelled late, never lands on a fresh engine's intent of the same
//! number. Its fill still counts in the deployment's ledger at its
//! destination, which a Stop closes.

use super::inbox::Message;
use super::{side_name, store, RunEntry, RunInfo, Runner};
use crate::services::live_owner::Destination;
use crate::strategy::dispatch::{DispatchResult, RunMode};
use serde_json::Value;
use std::collections::BTreeMap;

/// The rows `destination` owns.
pub(super) fn owned_by(
    destination: &Destination,
    rows: Vec<store::OrderRow>,
) -> Vec<store::OrderRow> {
    rows.into_iter()
        .filter(|o| o.destination().is_some_and(|d| destination.owns(&d)))
        .collect()
}

/// One line for a trader about what a deployment holds somewhere else.
fn elsewhere_line(at: &Destination, h: &store::Holding) -> String {
    let what = format!(
        "{} {} {} {}",
        side_name(h.quantity),
        h.quantity.unsigned_abs(),
        h.symbol,
        h.product
    );
    if at.is_known() {
        format!(
            "This strategy also holds {} through {}, from an earlier run there. It is not managed or closed from here: sign in to {} and Stop it there, or close it at that broker.",
            what,
            at.describe(),
            at.describe()
        )
    } else {
        format!(
            "This strategy also holds {} at your broker, placed before this version recorded which account it went through, so OpenAlgo does not manage or close it. Close it at your broker.",
            what
        )
    }
}

impl Runner {
    /// Why this run must not send, cancel or read through the session
    /// connected now (another broker or account than its own), or `None`.
    pub(super) fn session_refusal(&self, info: &RunInfo) -> Option<String> {
        info.destination
            .refusal(self.services().live_owner().as_ref())
    }

    /// Ask for one of this run's orders to be cancelled, only through its
    /// own session.
    pub(super) async fn cancel_own(&self, info: &RunInfo, orderid: &str) -> DispatchResult {
        if let Some(why) = self.session_refusal(info) {
            return DispatchResult::refused(why);
        }
        self.services().cancel(info.mode, orderid).await
    }

    /// Tell a run's page what became of one of its rows: only a row its own
    /// engine placed. A row of an earlier run of the deployment, or one from
    /// before runs were recorded, is that run's, whose page is gone.
    pub(super) fn frame_to_page(&self, entry: &RunEntry, row: &store::OrderRow, frame: Value) {
        let (Some(intent), Some(run)) = (row.intent_id, row.incarnation) else {
            return;
        };
        if run != entry.info.incarnation {
            tracing::debug!(
                "{}: order row {} belongs to run {}, not to the running one, so its page is not told",
                entry.info.run_id,
                row.id,
                run
            );
            return;
        }
        entry.inbox.push(Message::Frame {
            intent_id: intent,
            frame,
        });
    }

    /// Where a run of `given` would trade now: its own destination while it
    /// runs, else the session connected now on the side it last started on.
    /// While it runs on another session than the one connected now, the
    /// sentence saying so: its book is not read through that one.
    pub fn book_destination(&self, given: &str) -> Result<Destination, String> {
        if let Some(info) = self.status_of(given) {
            return match self.session_refusal(&info) {
                Some(why) => Err(why),
                None => Ok(info.destination),
            };
        }
        Ok(match self.mode_for(given) {
            RunMode::Sandbox => Destination::sandbox(),
            RunMode::Live => Destination::live(self.services().live_owner().unwrap_or_default()),
        })
    }

    /// What the deployment holds on `destination`'s side somewhere other
    /// than `destination`, one line each for a trader (NEW-TR-01). Empty
    /// when its rows cannot be read: whoever needs them says so.
    pub(super) fn held_elsewhere(&self, run_id: &str, destination: &Destination) -> Vec<String> {
        let rows = match self
            .db
            .conn()
            .and_then(|c| store::orders_of(&c, run_id, destination.mode.as_str()))
        {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!("Could not read the orders of {}: {}", run_id, e);
                return Vec::new();
            }
        };
        let mut by_place: BTreeMap<(String, String), (Destination, Vec<store::OrderRow>)> =
            BTreeMap::new();
        for o in rows {
            let Some(at) = o.destination().filter(|d| !destination.owns(d)) else {
                continue;
            };
            let key = (
                at.owner.broker.to_lowercase(),
                at.owner.account.to_lowercase(),
            );
            by_place
                .entry(key)
                .or_insert_with(|| (at, Vec::new()))
                .1
                .push(o);
        }
        by_place
            .into_values()
            .flat_map(|(at, rows)| {
                store::holdings(&rows)
                    .into_iter()
                    .filter(|h| h.quantity != 0)
                    .map(move |h| elsewhere_line(&at, &h))
            })
            .collect()
    }

    /// [`Self::held_elsewhere`] for a deployment by name, as its status
    /// reports it.
    pub fn held_elsewhere_of(&self, given: &str) -> Vec<String> {
        match self.book_destination(given) {
            Ok(at) => self.held_elsewhere(&self.as_run_id(given), &at),
            Err(_) => Vec::new(),
        }
    }

    /// What a run starting now is told about the deployment's earlier
    /// positions: those at its own destination it does not trade but a Stop
    /// closes (ARCH-04 d), and those elsewhere it never touches.
    pub(super) fn start_lines(&self, info: &RunInfo) -> Vec<String> {
        let mut lines = vec![format!(
            "This run trades through {}. Its engine begins holding nothing.",
            info.destination.describe()
        )];
        if let Ok(own) = self.read_own(info) {
            for h in store::holdings(&own).iter().filter(|h| h.quantity != 0) {
                lines.push(format!(
                    "Earlier runs of this strategy left {} {} {} {} here. This run does not trade it; Stop closes it.",
                    side_name(h.quantity),
                    h.quantity.unsigned_abs(),
                    h.symbol,
                    h.product
                ));
            }
        }
        lines.extend(self.held_elsewhere(&info.run_id, &info.destination));
        lines
    }
}
