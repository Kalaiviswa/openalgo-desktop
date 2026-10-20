//! Ticks and updates the app's consumers skipped because they fell behind.
//! The app logs each skip (the risk owners at debug level) but keeps no
//! count of most of them, so a benchmark counts them from the log: a layer
//! on the process's tracing subscriber adds up the number in every
//! "lagged by N" / "skipped N" line, per consumer.

use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};
use tracing::field::{Field, Visit};
use tracing_subscriber::filter::Targets;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::Layer;

/// Skipped counts by consumer.
#[derive(Default)]
pub struct Skipped(Mutex<BTreeMap<&'static str, u64>>);

impl Skipped {
    pub fn get(&self, consumer: &str) -> u64 {
        self.0.lock().get(consumer).copied().unwrap_or(0)
    }

    /// `consumer:n,...`, or `none`.
    pub fn summary(&self) -> String {
        let m = self.0.lock();
        if m.is_empty() {
            return "none".into();
        }
        m.iter()
            .map(|(k, v)| format!("{}:{}", k, v))
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// The logging modules of the consumers that skip, and their labels.
const CONSUMERS: [(&str, &str); 6] = [
    (
        "openalgo_desktop_lib::strategy::tick_feed",
        "strategy_ticks",
    ),
    ("openalgo_desktop_lib::scalping::monitor", "scalping_ticks"),
    (
        "openalgo_desktop_lib::services::sandbox_feed",
        "sandbox_ticks",
    ),
    ("openalgo_desktop_lib::feed::bridge", "feed_bridge"),
    ("openalgo_desktop_lib::feed::server", "feed_server"),
    (
        "openalgo_desktop_lib::services::broker_runtime",
        "order_relay",
    ),
];

struct Message(String);

impl Visit for Message {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{:?}", value);
        }
    }
}

/// The count in "lagged by N" or "skipped N".
fn count_in(message: &str) -> Option<u64> {
    let rest = message
        .split_once("lagged by ")
        .or_else(|| message.split_once("skipped "))?
        .1;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

struct Count(Arc<Skipped>);

impl<S: tracing::Subscriber> Layer<S> for Count {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        let target = event.metadata().target();
        let Some((_, label)) = CONSUMERS.iter().find(|(t, _)| target.starts_with(t)) else {
            return;
        };
        let mut m = Message(String::new());
        event.record(&mut m);
        if let Some(n) = count_in(&m.0) {
            *self.0 .0.lock().entry(label).or_insert(0) += n;
        }
    }
}

/// Start counting (once per process; the benchmarks each run in their own).
pub fn install() -> Arc<Skipped> {
    static SKIPPED: OnceLock<Arc<Skipped>> = OnceLock::new();
    SKIPPED
        .get_or_init(|| {
            let skipped = Arc::new(Skipped::default());
            let filter = CONSUMERS.iter().fold(Targets::new(), |f, (t, _)| {
                f.with_target(*t, tracing::Level::DEBUG)
            });
            let subscriber =
                tracing_subscriber::registry().with(Count(skipped.clone()).with_filter(filter));
            let _ = tracing::subscriber::set_global_default(subscriber);
            skipped
        })
        .clone()
}

#[test]
fn counts_are_read_from_both_wordings() {
    assert_eq!(count_in("Strategy tick consumer lagged by 120"), Some(120));
    assert_eq!(
        count_in("Market data relay fell behind the broker feed; skipped 7 events (9 total)"),
        Some(7)
    );
    assert_eq!(count_in("nothing here"), None);
}
