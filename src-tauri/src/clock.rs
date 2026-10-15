//! Injected clock. Session expiry, OAuth state expiry and anything else that
//! depends on wall time reads it from here so tests are deterministic.

use chrono::{DateTime, FixedOffset, Offset, Utc};
use parking_lot::RwLock;
use std::sync::Arc;

/// India Standard Time is UTC+05:30 all year (no daylight saving), so this
/// one offset converts every exchange time: the app's single copy of it.
pub const IST_OFFSET_SECS: i64 = 5 * 3600 + 30 * 60;

/// India Standard Time as a chrono offset.
pub fn ist_offset() -> FixedOffset {
    FixedOffset::east_opt(IST_OFFSET_SECS as i32).unwrap_or(Utc.fix())
}

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// Settable clock for tests.
#[derive(Debug)]
pub struct ManualClock {
    now: RwLock<DateTime<Utc>>,
}

impl ManualClock {
    pub fn new(now: DateTime<Utc>) -> Arc<Self> {
        Arc::new(Self {
            now: RwLock::new(now),
        })
    }

    pub fn set(&self, now: DateTime<Utc>) {
        *self.now.write() = now;
    }

    pub fn advance(&self, by: chrono::Duration) {
        let mut n = self.now.write();
        *n += by;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *self.now.read()
    }
}
