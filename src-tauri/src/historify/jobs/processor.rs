//! The job processor: one task per running job downloads its items in
//! turn, paced and pausable, and claims the job's terminal status under
//! its status lock.

use super::{jitter, until_cancel, Ctl, Finished, Interrupted, JobEngine, SAVE_FAILED};
use crate::historify::db::{self, Item};
use crate::historify::interval;
use crate::historify::time::{ist_day, parse_date};
use chrono::{Duration as CDuration, NaiveDate, NaiveDateTime};
use serde_json::{json, Value};
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::sync::{watch, Mutex as AsyncMutex, OwnedSemaphorePermit};

impl JobEngine {
    pub(super) async fn process(
        self,
        job_id: String,
        gen: u64,
        rx: watch::Receiver<Ctl>,
        st: Arc<AsyncMutex<()>>,
    ) {
        let mut fin = Finished {
            job_id: job_id.clone(),
            config: Value::Null,
            status: "failed".into(),
            total: 0,
            completed: 0,
            failed: 0,
        };
        let r = self.run(&job_id, gen, rx, &st, &mut fin).await;
        if let Err(e) = r {
            tracing::error!("Historify job {} stopped: {}", job_id, e);
            fin.status = "failed".into();
            self.write_status(
                &job_id,
                "failed",
                Some("The download stopped unexpectedly. Retry the job.".into()),
            )
            .await;
        }
        self.release(&job_id, gen);
        if fin
            .config
            .get("schedule_id")
            .and_then(Value::as_str)
            .is_some()
        {
            let hook = self.inner.hook.read().as_ref().and_then(Weak::upgrade);
            if let Some(h) = hook {
                h.finished(fin).await;
            }
        }
    }

    fn emit_paused(&self, job_id: &str, current: i64, total: i64) {
        self.inner.notify.notify(
            "historify_job_paused",
            json!({"job_id": job_id, "current": current, "total": total, "status": "paused"}),
        );
    }

    /// Wait out a pause (or return the cancel). `Ok(())` when running.
    async fn wait_running(
        &self,
        job_id: &str,
        rx: &mut watch::Receiver<Ctl>,
        current: i64,
        total: i64,
    ) -> Result<(), Interrupted> {
        loop {
            let c = *rx.borrow_and_update();
            match c {
                Ctl::Run => return Ok(()),
                Ctl::Cancel => return Err(Interrupted::Cancelled),
                Ctl::Pause => {
                    self.emit_paused(job_id, current, total);
                    tokio::select! {
                        r = rx.changed() => if r.is_err() { return Err(Interrupted::Cancelled) },
                        _ = tokio::time::sleep(self.inner.cfg.pause_heartbeat) => {}
                    }
                }
            }
        }
    }

    /// Waits for a download slot and marks the job running: the slot and
    /// the job. `None` when the job ends first: cancelled while queued or
    /// before it ran, or not found.
    async fn begin(
        &self,
        job_id: &str,
        rx: &watch::Receiver<Ctl>,
        st: &Arc<AsyncMutex<()>>,
        fin: &mut Finished,
    ) -> crate::error::Result<Option<(OwnedSemaphorePermit, Value)>> {
        // Queue for a download slot; a cancel while queued ends the job.
        let permit = tokio::select! {
            p = self.inner.permits.clone().acquire_owned() => match p {
                Ok(p) => p,
                Err(_) => return Ok(None),
            },
            _ = until_cancel(rx.clone()) => {
                fin.status = "cancelled".into();
                return Ok(None);
            }
        };

        let id = job_id.to_string();
        let Some(job) = self.inner.db.run(move |c| db::job(c, &id)).await? else {
            tracing::error!("Historify job {} not found", job_id);
            return Ok(None);
        };
        fin.config = job
            .get("config")
            .and_then(Value::as_str)
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .unwrap_or(Value::Null);
        {
            let _g = st.lock().await;
            if *rx.borrow() == Ctl::Cancel {
                fin.status = "cancelled".into();
                return Ok(None);
            }
            self.write_status(job_id, "running", None).await;
        }
        Ok(Some((permit, job)))
    }

    /// Sets an item's status, with no records or error.
    async fn mark_item(
        &self,
        iid: i64,
        status: &'static str,
        now: NaiveDateTime,
    ) -> crate::error::Result<()> {
        self.inner
            .db
            .write(move |c| db::set_item_status(c, iid, status, 0, None, now))
            .await
    }

    /// Tells the page which symbol the job has reached.
    fn notify_progress(&self, job_id: &str, processed: i64, total: i64, symbol: &str) {
        self.inner.notify.notify(
            "historify_progress",
            json!({
                "job_id": job_id,
                "current": processed,
                "total": total,
                "symbol": symbol,
                "percent": ((processed as f64 / total as f64) * 1000.0).round() / 10.0,
            }),
        );
    }

    /// Stores an item's outcome and, unless it was skipped, the job's
    /// counts.
    async fn record_item(
        &self,
        job_id: &str,
        iid: i64,
        (status, records, error): (&'static str, i64, Option<String>),
        (completed, failed): (i64, i64),
    ) -> crate::error::Result<()> {
        let (id, now) = (job_id.to_string(), self.now());
        let progress = status != "skipped";
        self.inner
            .db
            .write(move |c| {
                db::set_item_status(c, iid, status, records, error.as_deref(), now)?;
                if progress {
                    db::set_job_progress(c, &id, completed, failed)?;
                }
                Ok(())
            })
            .await
    }

    /// The pause after a symbol, and the longer cooldown after each batch.
    async fn pause_after_item(&self, rx: &watch::Receiver<Ctl>, job_id: &str, done_this_run: i64) {
        let cfg = &self.inner.cfg;
        self.pause_between(rx, jitter(cfg.delay_min, cfg.delay_max))
            .await;
        if cfg.batch_size > 0 && done_this_run > 0 && done_this_run % cfg.batch_size as i64 == 0 {
            let d = jitter(cfg.batch_cooldown_min, cfg.batch_cooldown_max);
            tracing::info!(
                "Historify job {}: cooldown after {} symbols, waiting {:.1}s",
                job_id,
                done_this_run,
                d.as_secs_f64()
            );
            self.pause_between(rx, d).await;
        }
    }

    /// Claims the terminal state under the status lock, honouring a pause
    /// that landed after the last item.
    async fn finish(
        &self,
        job_id: &str,
        gen: u64,
        rx: &mut watch::Receiver<Ctl>,
        st: &Arc<AsyncMutex<()>>,
        fin: &mut Finished,
        (completed, failed, processed, total): (i64, i64, i64, i64),
    ) {
        loop {
            let g = st.lock().await;
            let c = *rx.borrow_and_update();
            match c {
                Ctl::Cancel => {
                    fin.status = "cancelled".into();
                    return;
                }
                Ctl::Pause => {
                    drop(g);
                    self.emit_paused(job_id, processed, total);
                    tokio::select! {
                        r = rx.changed() => if r.is_err() {
                            fin.status = "cancelled".into();
                            return;
                        },
                        _ = tokio::time::sleep(self.inner.cfg.pause_heartbeat) => {}
                    }
                }
                Ctl::Run => {
                    let status = if failed == 0 {
                        "completed"
                    } else {
                        "completed_with_errors"
                    };
                    self.write_status(job_id, status, None).await;
                    self.release(job_id, gen);
                    drop(g);
                    fin.status = status.into();
                    self.inner.notify.notify(
                        "historify_job_complete",
                        json!({
                            "job_id": job_id,
                            "completed": completed,
                            "failed": failed,
                            "total": total,
                            "status": status,
                        }),
                    );
                    tracing::info!(
                        "Historify job {} completed: {} success, {} failed",
                        job_id,
                        completed,
                        failed
                    );
                    return;
                }
            }
        }
    }

    async fn run(
        &self,
        job_id: &str,
        gen: u64,
        mut rx: watch::Receiver<Ctl>,
        st: &Arc<AsyncMutex<()>>,
        fin: &mut Finished,
    ) -> crate::error::Result<()> {
        let Some((_permit, job)) = self.begin(job_id, &rx, st, fin).await? else {
            return Ok(());
        };

        let id = job_id.to_string();
        let items = self.inner.db.run(move |c| db::items(c, &id)).await?;
        if items.is_empty() {
            self.write_status(job_id, "failed", Some("No items to process".into()))
                .await;
            return Ok(());
        }
        let (pending, total, already_completed, already_failed) = split_items(items);
        let (mut completed, mut failed) = (already_completed, already_failed);
        let mut processed = already_completed + already_failed;
        fin.total = total;
        fin.completed = completed;
        fin.failed = failed;

        let (interval, start, end, incremental) = job_range(&job, &fin.config);

        for item in pending {
            if self
                .wait_running(job_id, &mut rx, processed, total)
                .await
                .is_err()
            {
                tracing::info!("Historify job {} cancelled", job_id);
                fin.status = "cancelled".into();
                return Ok(());
            }

            self.mark_item(item.id, "downloading", self.now()).await?;
            processed += 1;
            self.notify_progress(job_id, processed, total, &item.symbol);

            let outcome = self
                .download_item(&rx, &item, &interval, start, end, incremental)
                .await;
            let (status, records, error) = match outcome {
                Err(Interrupted::Cancelled) => {
                    // Not finished: leave it for a retry or resume.
                    self.mark_item(item.id, "pending", Default::default())
                        .await?;
                    fin.status = "cancelled".into();
                    return Ok(());
                }
                Ok(ItemOutcome::Skipped(msg)) => ("skipped", 0, Some(msg)),
                Ok(ItemOutcome::Done(Ok(n))) => {
                    completed += 1;
                    ("success", n, None)
                }
                Ok(ItemOutcome::Done(Err((n, msg)))) => {
                    failed += 1;
                    ("error", n, Some(msg))
                }
            };
            let progress = status != "skipped";
            self.record_item(
                job_id,
                item.id,
                (status, records, error),
                (completed, failed),
            )
            .await?;
            fin.completed = completed;
            fin.failed = failed;
            if !progress {
                tracing::info!(
                    "Skipping {} - data already covers requested range",
                    item.symbol
                );
                continue;
            }

            let done_this_run = processed - already_completed - already_failed;
            self.pause_after_item(&rx, job_id, done_this_run).await;
        }

        self.finish(
            job_id,
            gen,
            &mut rx,
            st,
            fin,
            (completed, failed, processed, total),
        )
        .await;
        Ok(())
    }

    /// Sleep `d`, ending early on a cancel.
    async fn pause_between(&self, rx: &watch::Receiver<Ctl>, d: Duration) {
        if d.is_zero() {
            return;
        }
        tokio::select! {
            _ = tokio::time::sleep(d) => {}
            _ = until_cancel(rx.clone()) => {}
        }
    }

    /// One broker download into the store: `Ok(records)` or the reason.
    async fn download_range(
        &self,
        rx: &watch::Receiver<Ctl>,
        symbol: &str,
        exchange: &str,
        interval: &str,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<Result<i64, String>, Interrupted> {
        if !interval::is_storage(interval) {
            return Ok(Err(format!(
                "Only {} intervals can be downloaded. Other timeframes ({}) are computed from 1m data.",
                interval::sorted(interval::STORAGE_INTERVALS).join(", "),
                interval::sorted(interval::COMPUTED_INTERVALS).join(", ")
            )));
        }
        let fetch = async {
            self.inner.pacer.acquire_unbounded().await;
            self.inner
                .source
                .fetch(symbol, exchange, interval, start, end)
                .await
        };
        let candles = tokio::select! {
            r = fetch => r,
            _ = until_cancel(rx.clone()) => return Err(Interrupted::Cancelled),
        };
        let candles = match candles {
            Ok(c) => c,
            Err(m) => return Ok(Err(m)),
        };
        if candles.is_empty() {
            return Ok(Ok(0));
        }
        let (s, e, i, now) = (
            symbol.to_string(),
            exchange.to_string(),
            interval.to_string(),
            self.now(),
        );
        match self
            .inner
            .db
            .write(move |c| db::upsert_bars(c, &s, &e, &i, &candles, now))
            .await
        {
            Ok(n) => Ok(Ok(n as i64)),
            Err(err) => {
                tracing::error!(
                    "Saving {}:{} {} candles failed: {}",
                    exchange,
                    symbol,
                    interval,
                    err
                );
                Ok(Err(SAVE_FAILED.to_string()))
            }
        }
    }

    /// Web item processing, including the incremental before/after ranges.
    async fn download_item(
        &self,
        rx: &watch::Receiver<Ctl>,
        item: &Item,
        interval: &str,
        start: Option<NaiveDate>,
        end: Option<NaiveDate>,
        incremental: bool,
    ) -> Result<ItemOutcome, Interrupted> {
        let (Some(start), Some(end)) = (start, end) else {
            return Ok(ItemOutcome::Done(Err((
                0,
                "This download has no valid date range. Create it again with a start and end date."
                    .into(),
            ))));
        };
        let (sym, exch) = (item.symbol.as_str(), item.exchange.as_str());
        if incremental {
            let (s, e, i) = (sym.to_string(), exch.to_string(), interval.to_string());
            let range = self
                .inner
                .db
                .run(move |c| db::data_range(c, &s, &e, &i))
                .await
                .unwrap_or_else(|err| {
                    tracing::error!(
                        "Reading the stored range of {}:{} failed: {}",
                        exch,
                        sym,
                        err
                    );
                    None
                });
            if let Some((Some(first_ts), Some(last_ts), _)) =
                range.filter(|r| r.0.is_some_and(|v| v != 0) && r.1.is_some_and(|v| v != 0))
            {
                let (first, last) = (ist_day(first_ts), ist_day(last_ts));
                let minute = interval == "1m";
                let need_before = start < first;
                let need_after = if minute { end >= last } else { end > last };
                if !need_before && !need_after {
                    return Ok(ItemOutcome::Skipped(
                        "Data already covers requested range".into(),
                    ));
                }
                let mut records = 0i64;
                let mut error: Option<String> = None;
                if need_before {
                    let before_end = if minute {
                        first
                    } else {
                        first - CDuration::days(1)
                    };
                    if start <= before_end {
                        match self
                            .download_range(rx, sym, exch, interval, start, before_end)
                            .await?
                        {
                            Ok(n) => records += n,
                            Err(m) => error = Some(m),
                        }
                    }
                }
                if need_after && error.is_none() {
                    let after_start = if minute {
                        last
                    } else {
                        last + CDuration::days(1)
                    };
                    if after_start <= end {
                        match self
                            .download_range(rx, sym, exch, interval, after_start, end)
                            .await?
                        {
                            Ok(n) => records += n,
                            Err(m) => error = Some(m),
                        }
                    }
                }
                return Ok(ItemOutcome::Done(match error {
                    Some(m) => Err((records, m)),
                    None => Ok(records),
                }));
            }
        }
        Ok(ItemOutcome::Done(
            self.download_range(rx, sym, exch, interval, start, end)
                .await?
                .map_err(|m| (0, m)),
        ))
    }

    /// Web `download_data` for one symbol, outside any job (`/api/download`).
    pub async fn download_now(
        &self,
        symbol: &str,
        exchange: &str,
        interval: &str,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<i64, String> {
        let (_tx, rx) = watch::channel(Ctl::Run);
        match self
            .download_range(&rx, symbol, exchange, interval, start, end)
            .await
        {
            Ok(r) => r,
            Err(Interrupted::Cancelled) => Err("The download was stopped.".into()),
        }
    }
}

/// The items still to download (pending, or cut off while downloading),
/// with the total and the counts already done and already failed.
fn split_items(items: Vec<Item>) -> (Vec<Item>, i64, i64, i64) {
    let total = items.len() as i64;
    let already_completed = items.iter().filter(|i| i.status == "success").count() as i64;
    let already_failed = items.iter().filter(|i| i.status == "error").count() as i64;
    let pending: Vec<Item> = items
        .into_iter()
        .filter(|i| i.status == "pending" || i.status == "downloading")
        .collect();
    (pending, total, already_completed, already_failed)
}

/// What a job downloads: its interval (daily when unset), its date range,
/// and whether it is incremental.
fn job_range(job: &Value, config: &Value) -> (String, Option<NaiveDate>, Option<NaiveDate>, bool) {
    let incremental = config
        .get("incremental")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let interval = job["interval"].as_str().unwrap_or("D").to_string();
    let start = job["start_date"].as_str().and_then(parse_date);
    let end = job["end_date"].as_str().and_then(parse_date);
    (interval, start, end, incremental)
}

enum ItemOutcome {
    Skipped(String),
    Done(Result<i64, (i64, String)>),
}
