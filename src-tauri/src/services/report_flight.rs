//! One report computed at a time, shared by every request that arrives
//! while it is computed (single flight), for the report pages a browser
//! keeps refreshing (Latency, Traffic). Each report holds a logs
//! connection for its whole computation, and the logs pool has four, so
//! four refreshes computing at once left none for the traffic, latency,
//! error and order log writers. Shared, they hold one.

use crate::error::Result;
use crate::state::AppState;
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;

#[derive(Default)]
pub struct ReportFlight {
    /// The last report and when it finished; locked while one is computed.
    last: tokio::sync::Mutex<Option<(Instant, Value)>>,
}

impl ReportFlight {
    /// The report `compute` makes, on the blocking pool (off the async
    /// workers). A request that arrives while one is computed waits for it
    /// and answers with it; one that arrives after it computes its own.
    pub async fn share<F>(&self, ctx: &Arc<AppState>, compute: F) -> Result<Value>
    where
        F: FnOnce(&AppState) -> Result<Value> + Send + 'static,
    {
        let asked = Instant::now();
        let mut last = self.last.lock().await;
        if let Some((done, report)) = last.as_ref() {
            if *done >= asked {
                return Ok(report.clone());
            }
        }
        let report = ctx.blocking(compute).await?;
        *last = Some((Instant::now(), report.clone()));
        Ok(report)
    }
}
