//! Sahi's request limits (web `broker/sahi/api/rate_limiter.py`).
//!
//! Sahi gives every endpoint its own budget per user, over a per-second and
//! a per-minute window (docs.sahi.com, Conventions, "Rate limits"). Each
//! category here has its own [`Budget`], so a burst of quotes never delays
//! an order. Every window is paced a little under Sahi's figure, as the web
//! does, to leave room for clock jitter. A 429 pauses the whole category for
//! the Retry-After Sahi names (the Fyers rule, BRL-03).

use crate::brokers::common::outcome;
use crate::brokers::common::ratelimit::{Budget, Denied, Limits, Rolling, MAX_QUEUE_WAIT};
use crate::error::AppError;
use reqwest::Method;
use std::time::Duration;
use tokio::time::Instant;

/// Throttle answers repeated per request (web `MAX_RETRIES`).
pub const MAX_RETRIES: u32 = 3;

/// Back-off when Sahi gives no Retry-After: 1, 2, 4 s (web `BASE_BACKOFF`).
pub const BASE_BACKOFF: Duration = Duration::from_secs(1);

/// One row of Sahi's rate-limit table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// Place, modify and cancel share one budget.
    OrderWrite,
    /// Order book, order detail, trades of an order.
    OrderRead,
    Positions,
    Holdings,
    Funds,
    /// Order and basket margin share one budget.
    Margin,
    Profile,
    Ltp,
    Quotes,
    Candles,
    OptionChain,
    /// The token endpoint on the auth host; its limit is not published.
    Auth,
}

const CATEGORIES: [Category; 12] = [
    Category::OrderWrite,
    Category::OrderRead,
    Category::Positions,
    Category::Holdings,
    Category::Funds,
    Category::Margin,
    Category::Profile,
    Category::Ltp,
    Category::Quotes,
    Category::Candles,
    Category::OptionChain,
    Category::Auth,
];

/// A window of `max` sends per `span`.
const fn window(max: u32, span_ms: u64) -> Rolling {
    Rolling {
        max,
        span: Duration::from_millis(span_ms),
    }
}

impl Category {
    /// The category of an API-host call, from its method and path.
    pub fn of(method: &Method, path: &str) -> Self {
        let path = path.split('?').next().unwrap_or(path);
        match path {
            p if p.starts_with("/v1/orders") && *method != Method::GET => Category::OrderWrite,
            p if p.starts_with("/v1/orders") => Category::OrderRead,
            p if p.starts_with("/v1/positions") => Category::Positions,
            p if p.starts_with("/v1/holdings") => Category::Holdings,
            p if p.starts_with("/v1/funds") => Category::Funds,
            p if p.starts_with("/v1/margins") => Category::Margin,
            p if p.starts_with("/v1/profile") => Category::Profile,
            p if p.starts_with("/v1/marketdata/ltp") => Category::Ltp,
            p if p.starts_with("/v1/marketdata/quotes") => Category::Quotes,
            p if p.starts_with("/v1/marketdata/candles") => Category::Candles,
            p if p.starts_with("/v1/marketdata/optionchain") => Category::OptionChain,
            _ => Category::Auth,
        }
    }

    /// The web's `LIMITS` row: a one-per-second endpoint is paced at one
    /// per 1.05 s, the others one under Sahi's figure.
    pub fn limits(self) -> Limits {
        let windows = match self {
            Category::OrderWrite => vec![window(9, 1_000), window(240, 60_000)],
            Category::OrderRead | Category::Positions | Category::Holdings | Category::Margin => {
                vec![window(4, 1_000), window(240, 60_000)]
            }
            Category::Funds | Category::Profile | Category::Ltp | Category::Quotes => {
                vec![window(1, 1_050)]
            }
            Category::Candles => vec![window(2, 1_050), window(240, 60_000)],
            Category::OptionChain => vec![window(2, 1_050), window(115, 60_000)],
            Category::Auth => vec![window(1, 2_000)],
        };
        Limits {
            windows,
            per_day: None,
        }
    }
}

/// One logical request: a deadline for all its waiting and the throttle
/// retries it may still spend (one budget per operation, however many
/// sends it takes).
#[derive(Debug, Clone)]
pub struct Op {
    pub deadline: Instant,
    pub retries_left: u32,
}

impl Op {
    pub fn single() -> Self {
        Self::within(MAX_QUEUE_WAIT)
    }

    pub fn within(total: Duration) -> Self {
        Self {
            deadline: Instant::now() + total,
            retries_left: MAX_RETRIES,
        }
    }
}

/// The budgets of one Sahi account.
pub struct SahiLimiter {
    budgets: Vec<(Category, Budget)>,
}

impl std::fmt::Debug for SahiLimiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SahiLimiter").finish_non_exhaustive()
    }
}

impl Default for SahiLimiter {
    fn default() -> Self {
        Self::with(|c| c.limits())
    }
}

impl SahiLimiter {
    /// Budgets with other limits (tests run a fake at full speed).
    pub fn with(limits: impl Fn(Category) -> Limits) -> Self {
        Self {
            budgets: CATEGORIES
                .iter()
                .map(|c| (*c, Budget::new(limits(*c))))
                .collect(),
        }
    }

    fn budget(&self, c: Category) -> Option<&Budget> {
        self.budgets.iter().find(|(k, _)| *k == c).map(|(_, b)| b)
    }

    /// Claim a send of category `c` within `op`'s deadline, or refuse it
    /// before anything is sent.
    pub async fn acquire(&self, c: Category, op: &Op) -> Result<(), AppError> {
        let Some(budget) = self.budget(c) else {
            return Ok(());
        };
        match budget.acquire(op.deadline).await {
            Ok(()) => Ok(()),
            Err(d) => {
                tracing::warn!("Sahi {:?} request refused before sending ({:?})", c, d);
                Err(outcome::busy(denied_message(d)))
            }
        }
    }

    /// Sahi answered with a throttle: hold every request of `c` until
    /// `until`, so other callers do not keep sending meanwhile.
    pub fn cool_down(&self, c: Category, until: Instant) {
        if let Some(b) = self.budget(c) {
            b.cool_down(until);
        }
    }
}

fn denied_message(d: Denied) -> String {
    match d {
        Denied::Wait(w) => format!(
            "Sahi is limiting requests right now and this one would have waited {} seconds for its turn, so it was not sent. Try again in a few seconds.",
            w.as_secs().max(1)
        ),
        Denied::Daily(_) => crate::brokers::common::ratelimit::BUSY_MESSAGE.to_string(),
    }
}
