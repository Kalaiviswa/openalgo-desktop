//! Following a run's orders after they are sent: the destination's answer
//! to each placement saved to its row (retried, and held on the run while it
//! cannot be, LOG-04), and every open order read back so the page hears what
//! became of it.

use super::{identity, orders, store, RunEntry, Runner, Unsaved};
use crate::strategy::dispatch::{OrderStatusResult, RunMode};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// How many times a placement's answer is written before it is kept on the
/// run to be written later.
const ACK_ATTEMPTS: u32 = 3;

impl Runner {
    fn try_save(&self, ack: &Unsaved) -> crate::error::Result<()> {
        let conn = self.db.conn()?;
        if let Some(pending) = ack.pending_order_id {
            return store::held_for_approval(&conn, ack.row_id, pending, self.clock.now());
        }
        if ack.status == store::UNCONFIRMED {
            return store::unconfirmed(
                &conn,
                ack.row_id,
                ack.client_tag.as_deref(),
                ack.message.as_deref(),
                self.clock.now(),
            );
        }
        store::placed(
            &conn,
            ack.row_id,
            ack.orderid.as_deref(),
            ack.status,
            ack.message.as_deref(),
            self.clock.now(),
        )
    }

    /// Save a placement's answer, trying a few times. False when it could
    /// not be saved.
    pub(super) async fn save_ack(&self, ack: &Unsaved) -> bool {
        for attempt in 1..=ACK_ATTEMPTS {
            match self.try_save(ack) {
                Ok(()) => return true,
                Err(e) => {
                    tracing::error!(
                        "Could not save the answer for OpenScript order row {} (attempt {}): {}",
                        ack.row_id,
                        attempt,
                        e
                    );
                    if attempt < ACK_ATTEMPTS {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
        }
        false
    }

    /// Save the answers a run could not save before. What still cannot be
    /// saved stays on the run.
    pub(super) fn flush_unsaved(&self, entry: &RunEntry) {
        let pending = std::mem::take(&mut *entry.unsaved.lock());
        if pending.is_empty() {
            return;
        }
        let mut left = Vec::new();
        for ack in pending {
            match self.try_save(&ack) {
                Ok(()) => self.write_log(
                    &entry.info,
                    &[format!(
                        "The answer for order {} is saved now.",
                        ack.orderid.as_deref().unwrap_or("without a number")
                    )],
                ),
                Err(e) => {
                    tracing::warn!(
                        "Still cannot save OpenScript order row {}: {}",
                        ack.row_id,
                        e
                    );
                    left.push(ack);
                }
            }
        }
        entry.unsaved.lock().extend(left);
    }

    /// Read back every open order of a run and tell the page what changed.
    pub(super) async fn refresh_orders(&self, entry: &RunEntry) {
        self.refresh_orders_until(entry, None).await;
    }

    /// One order's status, or `None` when it could not be read before
    /// `deadline`.
    async fn status_until(
        &self,
        mode: RunMode,
        oid: &str,
        deadline: Option<Instant>,
    ) -> Option<OrderStatusResult> {
        let services = self.services();
        let read = services.order_status(mode, oid);
        let Some(d) = deadline else {
            return Some(read.await);
        };
        tokio::time::timeout(d.saturating_duration_since(Instant::now()), read)
            .await
            .ok()
    }

    /// [`Self::refresh_orders`], making no read past `deadline` (TIME-03): a
    /// Stop's close budget bounds the whole close, not one pass of serial
    /// status reads. The rows left unread stay as they were, which the close
    /// reads as still working.
    pub(super) async fn refresh_orders_until(&self, entry: &RunEntry, deadline: Option<Instant>) {
        self.flush_unsaved(entry);
        let read = self
            .db
            .conn()
            .and_then(|c| store::open_orders(&c, &entry.info.run_id, entry.info.mode.as_str()))
            .map(|rows| identity::owned_by(&entry.info.destination, rows));
        let open = match read {
            Ok(rows) => rows,
            Err(e) => {
                // Whoever needs the rows reads them again and says so.
                tracing::warn!(
                    "Could not read the open orders of {}: {}",
                    entry.info.run_id,
                    e
                );
                return;
            }
        };
        // The Action Center's record is the app's own, read whatever session
        // is connected.
        let open = self.follow_approvals(entry, open);
        entry.unresolved.store(
            open.iter().filter(|o| o.orderid.is_none()).count(),
            Ordering::SeqCst,
        );
        // Another broker or account is connected: nothing of this run is read
        // through it (NEW-TR-01). Its orders are followed again once its own
        // session is back.
        if let Some(why) = self.session_refusal(&entry.info) {
            tracing::debug!("{}: orders not read: {}", entry.info.run_id, why);
            return;
        }
        for o in open {
            let Some(oid) = o.orderid.clone() else {
                continue;
            };
            let Some(read) = self.status_until(entry.info.mode, &oid, deadline).await else {
                tracing::info!(
                    "The close of {} ran out of time while reading its orders",
                    entry.info.run_id
                );
                return;
            };
            if !read.ok {
                continue;
            }
            if let Some(now) = orders::progress(&read.order, o.quantity) {
                self.fold_progress(entry, &o, &oid, &now);
            }
        }
    }

    /// Record what the destination now says about one open order, and tell
    /// the page of the run that placed it.
    fn fold_progress(
        &self,
        entry: &RunEntry,
        o: &store::OrderRow,
        oid: &str,
        now: &orders::Progress,
    ) {
        if now.status == o.status
            && now.filled == o.filled_quantity
            && (now.average_price.is_none() || now.average_price == o.average_price)
        {
            return;
        }
        if let Ok(conn) = self.db.conn() {
            let _ = store::progressed(
                &conn,
                o.id,
                now.status,
                now.filled,
                now.average_price,
                self.clock.now(),
            );
        }
        if now.status == "filled" {
            self.write_log(
                &entry.info,
                &[format!(
                    "Order {} filled: {} {} at {}.",
                    oid,
                    o.action,
                    now.filled,
                    now.average_price
                        .map(|p| format!("{:.2}", p))
                        .unwrap_or_else(|| "an unreported price".into())
                )],
            );
        }
        if let Some(intent) = o.intent_id {
            let frame = orders::frame(
                intent,
                now.status,
                now.filled,
                now.average_price.or(o.average_price),
                Some(oid),
                None,
                self.clock.now().timestamp_millis(),
            );
            self.frame_to_page(entry, o, frame);
        }
    }
}
