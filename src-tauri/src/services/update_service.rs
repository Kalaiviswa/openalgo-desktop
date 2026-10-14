//! In-app updates, notify only (maintainer decision 2026-10-15).
//!
//! | Step | Started by | What happens |
//! | --- | --- | --- |
//! | Check | every app start (release builds), "Check for Updates" | asks the update address for the newest version; nothing is downloaded |
//! | Notify | the banner and the update dialog | the version, its release notes and an Update button |
//! | Update | the trader's click (`POST /api/update/install`, CSRF) | names what a restart interrupts and asks first; then downloads the update and verifies its signature |
//! | Restart | the desktop window (`restart_to_update`, Tauri) | asks again if new activity appeared since; installs and restarts |
//!
//! Nothing downloads before the trader clicks Update, and OpenAlgo never
//! restarts without the trader's click in the desktop window. A copy that
//! cannot replace itself (a `.deb` or `.rpm` install, a build from source)
//! is told about the update and given the release page instead.
//!
//! The service owns the decisions (which version is newer, what a restart
//! interrupts, what the trader confirmed). The update plugin behind
//! [`UpdateBackend`] fetches the update information, downloads, checks the
//! signature against the public key built into the app, and installs
//! (`crate::updater`).

use crate::events::subscribers::UiEmitter;
use crate::state::AppState;
use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cmp::Ordering;
use std::sync::{Arc, Weak};
use std::time::Duration;

/// Where every release is published. The release page of one version is
/// `<RELEASES_URL>/tag/v<version>`.
pub const RELEASES_URL: &str = "https://github.com/marketcalls/openalgo-desktop/releases";

/// Socket.IO event carrying [`UpdateStatus`] whenever it changes.
pub const STATUS_EVENT: &str = "update_status";

/// How long a check may take before it counts as failed.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(30);

/// How long reading the broker's order book may take while deciding what a
/// restart interrupts.
const ORDER_BOOK_TIMEOUT: Duration = Duration::from_secs(10);

/// How often download progress is pushed to the window.
const PROGRESS_EVERY: Duration = Duration::from_millis(500);

/// Whether `offered` is an update for a copy running `current`.
///
/// Versions compare by semantic-version precedence (build metadata is
/// ignored), so `1.1.0-beta.2 < 1.1.0-beta.10 < 1.1.0`. A release build
/// never moves onto a pre-release; a pre-release build follows newer
/// pre-releases and the final release.
pub fn is_newer(current: &semver::Version, offered: &semver::Version) -> bool {
    offered.cmp_precedence(current) == Ordering::Greater
        && (offered.pre.is_empty() || !current.pre.is_empty())
}

/// How this copy was installed, which decides whether it can update itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Package {
    /// macOS app (from the `.dmg`): the update replaces the app.
    MacApp,
    /// Windows NSIS installer, per-user: the update runs the new installer.
    WindowsNsis,
    /// Linux AppImage: the update replaces the AppImage file.
    LinuxAppImage,
    /// Linux `.deb`: the package manager owns the files.
    LinuxDeb,
    /// Linux `.rpm`: the package manager owns the files.
    LinuxRpm,
    /// Built from source and run from the build folder.
    Development,
    /// Not known.
    Unknown,
}

impl Package {
    /// Whether this copy can download and install an update itself.
    pub fn self_updates(self) -> bool {
        matches!(
            self,
            Package::MacApp | Package::WindowsNsis | Package::LinuxAppImage
        )
    }

    /// Why this copy cannot update itself, in a trader's words.
    pub fn manual_reason(self) -> Option<&'static str> {
        match self {
            Package::MacApp | Package::WindowsNsis | Package::LinuxAppImage => None,
            Package::LinuxDeb => Some(
                "This copy was installed from a .deb package, which OpenAlgo cannot update by itself. Download the new .deb from the release page and install it over this one. Your data and settings are kept.",
            ),
            Package::LinuxRpm => Some(
                "This copy was installed from an .rpm package, which OpenAlgo cannot update by itself. Download the new version from the release page and install it over this one. Your data and settings are kept.",
            ),
            Package::Development => Some(
                "This copy of OpenAlgo was built from source, so it does not update itself. Build the new version, or install it from the release page.",
            ),
            Package::Unknown => Some(
                "OpenAlgo cannot tell how this copy was installed, so it does not update itself. Download the new version from the release page and install it over this one. Your data and settings are kept.",
            ),
        }
    }
}

/// A newer version the update address offers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Offer {
    pub version: String,
    /// Release notes, as published with the release.
    pub notes: Option<String>,
    /// When the release was published (RFC 3339), if the address says.
    pub published_at: Option<String>,
    /// The release page of this version.
    pub release_url: String,
}

impl Offer {
    pub fn new(version: &str, notes: Option<String>, published_at: Option<String>) -> Self {
        Self {
            version: version.to_string(),
            notes: notes.filter(|n| !n.trim().is_empty()),
            published_at,
            release_url: format!("{}/tag/v{}", RELEASES_URL, version),
        }
    }
}

/// Why a check, download or install did not work. The text is the detail
/// for the log; [`UpdateFailure::message`] is what the trader reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateFailure {
    /// The update address or the download could not be reached.
    Network(String),
    /// The update information is missing or unreadable.
    Manifest(String),
    /// The newest release has no update for this computer.
    NoPlatform(String),
    /// The download did not pass its signature check.
    Signature(String),
    /// The update could not be installed.
    Install(String),
}

impl UpdateFailure {
    pub fn detail(&self) -> &str {
        match self {
            UpdateFailure::Network(d)
            | UpdateFailure::Manifest(d)
            | UpdateFailure::NoPlatform(d)
            | UpdateFailure::Signature(d)
            | UpdateFailure::Install(d) => d,
        }
    }

    /// The trader's message when the download failed.
    pub fn download_message(&self, current: &str) -> String {
        match self {
            UpdateFailure::Network(_) => format!(
                "The update could not be downloaded. OpenAlgo is still on version {}. Check your internet connection and choose Update again.",
                current
            ),
            other => other.message(current),
        }
    }

    /// The trader's message, naming the cause and the next step.
    pub fn message(&self, current: &str) -> String {
        match self {
            UpdateFailure::Network(_) => "OpenAlgo could not reach the update server. Check your internet connection and try again.".into(),
            UpdateFailure::Manifest(_) => "OpenAlgo could not read the update information. Try again later, or download the newest version from the release page.".into(),
            UpdateFailure::NoPlatform(_) => "The newest version has no update for this computer yet. Try again later, or download it from the release page.".into(),
            UpdateFailure::Signature(_) => format!(
                "The downloaded update failed its signature check, so it was not installed. OpenAlgo is still on version {}. Try again later; if this keeps happening, download the installer from the release page.",
                current
            ),
            UpdateFailure::Install(_) => format!(
                "The update could not be installed. OpenAlgo is still on version {}. Download the installer from the release page and run it.",
                current
            ),
        }
    }
}

/// Download progress of the verified update, called as bytes arrive.
pub type ProgressFn = Box<dyn FnMut(u64, Option<u64>) + Send>;

/// The update plugin, behind a seam so the decisions here are tested
/// without a window.
#[async_trait]
pub trait UpdateBackend: Send + Sync {
    /// How this copy was installed.
    fn package(&self) -> Package;
    /// Ask the update address for a version newer than this one
    /// ([`is_newer`]). `None` when this is the newest.
    async fn check(&self) -> Result<Option<Offer>, UpdateFailure>;
    /// Download `version` (the one last offered) and verify its signature.
    /// Nothing on disk changes; the verified update waits for the restart.
    async fn download(&self, version: &str, progress: ProgressFn) -> Result<(), UpdateFailure>;
    /// Forget a downloaded update (the trader chose Later).
    fn discard(&self);
}

/// What a restart now would interrupt.
///
/// Sent back by the window as the trader's confirmation: a restart goes
/// ahead only while everything running is within what they confirmed
/// ([`Activity::covered_by`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Activity {
    /// Running Live strategies (the strategy module).
    pub live_strategies: u32,
    /// Running Live OpenScript runs.
    pub live_openscript_runs: u32,
    /// Live positions under scalping protection (stop or target).
    pub live_scalping_stops: u32,
    /// Open orders at the broker (open or trigger pending).
    pub live_open_orders: u32,
    /// The broker's order book could not be read.
    pub live_orders_unknown: bool,
    /// Placements whose outcome is not known yet and exits still filling.
    pub unsettled_orders: u32,
    /// Something running could not be checked.
    pub checks_failed: bool,
    pub sandbox_strategies: u32,
    pub sandbox_openscript_runs: u32,
    pub sandbox_scalping_stops: u32,
    pub sandbox_open_orders: u32,
}

/// The confirmation the trader is asked for before a restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Warning {
    /// `live` (Live trading would go unwatched) or `sandbox` (only sandbox
    /// activity pauses).
    pub level: &'static str,
    pub title: String,
    pub intro: String,
    pub items: Vec<String>,
    pub advice: String,
}

fn count(n: u32, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {}", one)
    } else {
        format!("{} {}", n, many)
    }
}

impl Activity {
    pub fn has_live(&self) -> bool {
        self.live_strategies
            + self.live_openscript_runs
            + self.live_scalping_stops
            + self.live_open_orders
            + self.unsettled_orders
            > 0
            || self.live_orders_unknown
            || self.checks_failed
    }

    pub fn has_sandbox(&self) -> bool {
        self.sandbox_strategies
            + self.sandbox_openscript_runs
            + self.sandbox_scalping_stops
            + self.sandbox_open_orders
            > 0
    }

    /// Nothing a restart would interrupt.
    pub fn is_quiet(&self) -> bool {
        !self.has_live() && !self.has_sandbox()
    }

    /// Whether everything running now is within what the trader confirmed
    /// (`confirmed`): no count grew and nothing unknown appeared.
    pub fn covered_by(&self, confirmed: &Activity) -> bool {
        let within = |now: u32, ok: u32| now <= ok;
        within(self.live_strategies, confirmed.live_strategies)
            && within(self.live_openscript_runs, confirmed.live_openscript_runs)
            && within(self.live_scalping_stops, confirmed.live_scalping_stops)
            && within(self.live_open_orders, confirmed.live_open_orders)
            && within(self.unsettled_orders, confirmed.unsettled_orders)
            && (!self.live_orders_unknown || confirmed.live_orders_unknown)
            && (!self.checks_failed || confirmed.checks_failed)
            && within(self.sandbox_strategies, confirmed.sandbox_strategies)
            && within(
                self.sandbox_openscript_runs,
                confirmed.sandbox_openscript_runs,
            )
            && within(
                self.sandbox_scalping_stops,
                confirmed.sandbox_scalping_stops,
            )
            && within(self.sandbox_open_orders, confirmed.sandbox_open_orders)
    }

    /// What the trader is told before a restart, or `None` when nothing
    /// runs. `broker` is the logged-in broker's display name.
    pub fn warning(&self, broker: Option<&str>) -> Option<Warning> {
        if self.is_quiet() {
            return None;
        }
        let at = broker.map(|b| format!(" at {}", b)).unwrap_or_default();
        let mut items = Vec::new();
        if self.live_strategies > 0 {
            items.push(format!(
                "{}. {} not checked until OpenAlgo opens again; then {} resume.",
                count(
                    self.live_strategies,
                    "running Live strategy",
                    "running Live strategies"
                ),
                if self.live_strategies == 1 {
                    "Its stops, targets and exits are"
                } else {
                    "Their stops, targets and exits are"
                },
                if self.live_strategies == 1 {
                    "it will"
                } else {
                    "they will"
                },
            ));
        }
        if self.live_openscript_runs > 0 {
            items.push(format!(
                "{}. {} paused when OpenAlgo closes; start {} again after the update.",
                count(
                    self.live_openscript_runs,
                    "running Live OpenScript run",
                    "running Live OpenScript runs"
                ),
                if self.live_openscript_runs == 1 {
                    "It is"
                } else {
                    "They are"
                },
                if self.live_openscript_runs == 1 {
                    "it"
                } else {
                    "them"
                },
            ));
        }
        if self.live_scalping_stops > 0 {
            items.push(format!(
                "{}. Stops and targets are not checked until OpenAlgo opens again; then protection resumes.",
                count(
                    self.live_scalping_stops,
                    "Live position under scalping protection",
                    "Live positions under scalping protection"
                ),
            ));
        }
        if self.live_open_orders > 0 {
            items.push(format!(
                "{}{}. {} at your broker; OpenAlgo follows {} again once it opens.",
                count(self.live_open_orders, "open order", "open orders"),
                at,
                if self.live_open_orders == 1 {
                    "It stays"
                } else {
                    "They stay"
                },
                if self.live_open_orders == 1 {
                    "it"
                } else {
                    "them"
                },
            ));
        }
        if self.live_orders_unknown {
            items.push(format!(
                "OpenAlgo could not read your open orders{}. Check your order book before you update.",
                at
            ));
        }
        if self.unsettled_orders > 0 {
            items.push(format!(
                "{}. OpenAlgo settles {} from your broker's order book once it opens again.",
                count(
                    self.unsettled_orders,
                    "order whose outcome is not known yet",
                    "orders whose outcome is not known yet"
                ),
                if self.unsettled_orders == 1 {
                    "it"
                } else {
                    "them"
                },
            ));
        }
        if self.checks_failed {
            items.push(
                "OpenAlgo could not check everything that is running. Check your strategies and positions before you update.".into(),
            );
        }
        let mut sandbox = Vec::new();
        if self.sandbox_strategies > 0 {
            sandbox.push(count(
                self.sandbox_strategies,
                "sandbox strategy",
                "sandbox strategies",
            ));
        }
        if self.sandbox_openscript_runs > 0 {
            sandbox.push(format!(
                "{} (paused; start {} again after the update)",
                count(
                    self.sandbox_openscript_runs,
                    "sandbox OpenScript run",
                    "sandbox OpenScript runs"
                ),
                if self.sandbox_openscript_runs == 1 {
                    "it"
                } else {
                    "them"
                },
            ));
        }
        if self.sandbox_scalping_stops > 0 {
            sandbox.push(count(
                self.sandbox_scalping_stops,
                "sandbox position under scalping protection",
                "sandbox positions under scalping protection",
            ));
        }
        if self.sandbox_open_orders > 0 {
            sandbox.push(count(
                self.sandbox_open_orders,
                "open sandbox order",
                "open sandbox orders",
            ));
        }
        if self.has_live() {
            if !sandbox.is_empty() {
                items.push(format!(
                    "Sandbox: {}. Nothing there is sent to your broker.",
                    join_list(&sandbox)
                ));
            }
            Some(Warning {
                level: "live",
                title: "Live trading is running".into(),
                intro: "Updating restarts OpenAlgo. Until it opens again, usually within a minute, nothing watches your Live trades:".into(),
                items,
                advice: "Update now only if that is acceptable. Otherwise choose Later and update after the market closes.".into(),
            })
        } else {
            let items = sandbox
                .into_iter()
                .map(|s| format!("{}.", capitalise(&s)))
                .collect();
            Some(Warning {
                level: "sandbox",
                title: "Sandbox activity is running".into(),
                intro:
                    "Updating restarts OpenAlgo. Your sandbox activity pauses until it opens again:"
                        .into(),
                items,
                advice: "Nothing is sent to your broker. Sandbox orders and positions are kept."
                    .into(),
            })
        }
    }
}

fn join_list(items: &[String]) -> String {
    match items.len() {
        0 => String::new(),
        1 => items[0].clone(),
        n => format!("{} and {}", items[..n - 1].join(", "), items[n - 1]),
    }
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn is_open_status(status: &str) -> bool {
    let s = status.trim().to_ascii_lowercase();
    s == "open" || s == "trigger pending" || s == "trigger_pending"
}

/// What a restart now would interrupt, read from every module that trades.
/// Reading the broker's order book is the only network call.
pub async fn activity(ctx: &AppState) -> Activity {
    use crate::strategy::dispatch::RunMode;
    let mut a = Activity::default();
    match ctx.strategy.store.list_open_runs() {
        Ok(runs) => {
            for r in runs {
                match RunMode::parse(&r.mode) {
                    Some(RunMode::Sandbox) => a.sandbox_strategies += 1,
                    // A run whose destination cannot be read counts as Live.
                    _ => a.live_strategies += 1,
                }
            }
        }
        Err(e) => {
            tracing::warn!("Update: could not read the running strategies: {}", e);
            a.checks_failed = true;
        }
    }
    for run in ctx.trading.runner.running() {
        match run.mode {
            RunMode::Live => a.live_openscript_runs += 1,
            RunMode::Sandbox => a.sandbox_openscript_runs += 1,
        }
    }
    let store = &ctx.scalping.store;
    match (
        store.active_sl(Some(crate::scalping::store::MODE_LIVE)),
        store.active_sl(Some(crate::scalping::store::MODE_ANALYZE)),
    ) {
        (Ok(live), Ok(sandbox)) => {
            a.live_scalping_stops = live.len() as u32;
            a.sandbox_scalping_stops = sandbox.len() as u32;
        }
        (l, s) => {
            if let Err(e) = l.and(s) {
                tracing::warn!("Update: could not read scalping protection: {}", e);
            }
            a.checks_failed = true;
        }
    }
    match ctx.sandbox.orderbook().await {
        Ok(book) => {
            a.sandbox_open_orders = book
                .data
                .orders
                .iter()
                .filter(|o| is_open_status(&o.order_status))
                .count() as u32;
        }
        Err(e) => {
            tracing::warn!("Update: could not read the sandbox order book: {}", e);
            a.checks_failed = true;
        }
    }
    // Live orders at the broker, whatever the analyzer toggle says: a Live
    // order rests at the broker either way.
    if let Ok(h) = crate::services::core::broker_handle(ctx) {
        match tokio::time::timeout(ORDER_BOOK_TIMEOUT, h.broker.get_order_book(&h.auth)).await {
            Ok(Ok(orders)) => {
                a.live_open_orders =
                    orders.iter().filter(|o| is_open_status(&o.status)).count() as u32;
            }
            Ok(Err(e)) => {
                tracing::warn!("Update: could not read the broker's order book: {}", e);
                a.live_orders_unknown = true;
            }
            Err(_) => {
                tracing::warn!("Update: the broker's order book did not answer in time");
                a.live_orders_unknown = true;
            }
        }
        let guard = ctx.switch_guard.read().clone();
        match guard.blockers(ctx, &h.id) {
            Ok(b) => a.unsettled_orders = b.uncertain_placements + b.partial_exits,
            Err(e) => {
                tracing::warn!("Update: could not check unsettled orders: {}", e);
                a.checks_failed = true;
            }
        }
    }
    a
}

/// The logged-in broker's display name, for the warning.
fn broker_name(ctx: &AppState) -> Option<String> {
    ctx.get_broker_session()
        .map(|s| crate::brokers::setup_guide::display_name(&s.broker_id))
}

#[derive(Debug, Clone, PartialEq)]
enum Phase {
    NotChecked,
    Checking,
    UpToDate,
    Available,
    Downloading { received: u64, total: Option<u64> },
    Ready,
    Restarting,
    Failed { message: String },
}

impl Phase {
    fn name(&self) -> &'static str {
        match self {
            Phase::NotChecked => "not_checked",
            Phase::Checking => "checking",
            Phase::UpToDate => "up_to_date",
            Phase::Available => "available",
            Phase::Downloading { .. } => "downloading",
            Phase::Ready => "ready",
            Phase::Restarting => "restarting",
            Phase::Failed { .. } => "failed",
        }
    }

    /// A download, a downloaded update or a restart is under way: a check
    /// must not replace the offer it belongs to.
    fn busy(&self) -> bool {
        matches!(
            self,
            Phase::Downloading { .. } | Phase::Ready | Phase::Restarting
        )
    }
}

/// Download progress, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Progress {
    pub received: u64,
    pub total: Option<u64>,
}

/// What the window shows (`GET /api/update/status`, the `update_status`
/// push).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UpdateStatus {
    pub current_version: String,
    /// `not_checked`, `checking`, `up_to_date`, `available`,
    /// `downloading`, `ready` (downloaded and verified, waiting for the
    /// restart), `restarting` or `failed`.
    pub state: &'static str,
    /// Why the last step failed (`failed`), in a trader's words.
    pub message: Option<String>,
    pub progress: Option<Progress>,
    pub offer: Option<Offer>,
    pub package: Package,
    /// This copy can download and install the update itself.
    pub can_install: bool,
    /// Why it cannot (a `.deb` install, a build from source).
    pub manual_reason: Option<&'static str>,
    pub releases_url: &'static str,
    /// When the last check finished (RFC 3339).
    pub checked_at: Option<String>,
}

struct Inner {
    phase: Phase,
    offer: Option<Offer>,
    checked_at: Option<DateTime<Utc>>,
}

/// The answer to an Update click or a restart request.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// The step went ahead.
    Proceed(UpdateStatus),
    /// Something is running: ask the trader, naming it, and send this
    /// activity back as the confirmation.
    Confirm {
        activity: Activity,
        warning: Warning,
    },
    /// The step cannot happen now; the message says why.
    Refused(String),
}

/// The update state of this app: one per process, in `AppState`.
pub struct UpdateService {
    current: String,
    backend: parking_lot::RwLock<Option<Arc<dyn UpdateBackend>>>,
    inner: Mutex<Inner>,
}

impl UpdateService {
    pub fn new(current_version: &str) -> Arc<Self> {
        Arc::new(Self {
            current: current_version.to_string(),
            backend: parking_lot::RwLock::new(None),
            inner: Mutex::new(Inner {
                phase: Phase::NotChecked,
                offer: None,
                checked_at: None,
            }),
        })
    }

    /// The update plugin of the desktop shell (unset in a test or the
    /// development server, where updates are not offered).
    pub fn set_backend(&self, backend: Arc<dyn UpdateBackend>) {
        *self.backend.write() = Some(backend);
    }

    fn backend(&self) -> Option<Arc<dyn UpdateBackend>> {
        self.backend.read().clone()
    }

    pub fn current_version(&self) -> &str {
        &self.current
    }

    fn package(&self) -> Package {
        self.backend()
            .map(|b| b.package())
            .unwrap_or(Package::Unknown)
    }

    pub fn status(&self) -> UpdateStatus {
        let package = self.package();
        let can_install = package.self_updates() && self.backend().is_some();
        let inner = self.inner.lock();
        let (message, progress) = match &inner.phase {
            Phase::Failed { message } => (Some(message.clone()), None),
            Phase::Downloading { received, total } => (
                None,
                Some(Progress {
                    received: *received,
                    total: *total,
                }),
            ),
            _ => (None, None),
        };
        UpdateStatus {
            current_version: self.current.clone(),
            state: inner.phase.name(),
            message,
            progress,
            offer: inner.offer.clone(),
            package,
            can_install,
            manual_reason: if can_install {
                None
            } else {
                package.manual_reason().or(Package::Unknown.manual_reason())
            },
            releases_url: RELEASES_URL,
            checked_at: inner
                .checked_at
                .map(|t| t.to_rfc3339_opts(SecondsFormat::Secs, true)),
        }
    }

    async fn push(&self, ctx: &AppState) {
        let v = serde_json::to_value(self.status()).unwrap_or(Value::Null);
        ctx.ui.emit(STATUS_EVENT, v).await;
    }

    /// Ask the update address for a newer version. A check while one is
    /// running, or while an update downloads or waits for the restart,
    /// answers with the state as it is.
    pub async fn check(&self, ctx: &AppState) -> UpdateStatus {
        let Some(backend) = self.backend() else {
            {
                let mut inner = self.inner.lock();
                if !inner.phase.busy() {
                    inner.phase = Phase::Failed {
                        message: "This copy of OpenAlgo cannot check for updates. Download the newest version from the release page.".into(),
                    };
                }
            }
            return self.status();
        };
        let already = {
            let mut inner = self.inner.lock();
            let already = inner.phase.busy() || inner.phase == Phase::Checking;
            if !already {
                inner.phase = Phase::Checking;
            }
            already
        };
        if already {
            return self.status();
        }
        self.push(ctx).await;
        let found = match tokio::time::timeout(CHECK_TIMEOUT, backend.check()).await {
            Ok(r) => r,
            Err(_) => Err(UpdateFailure::Network("the check timed out".into())),
        };
        {
            let mut inner = self.inner.lock();
            inner.checked_at = Some(ctx.now());
            match found {
                Ok(Some(offer)) => {
                    tracing::info!("Update: version {} is available", offer.version);
                    inner.offer = Some(offer);
                    inner.phase = Phase::Available;
                }
                Ok(None) => {
                    tracing::info!("Update: {} is the newest version", self.current);
                    inner.offer = None;
                    inner.phase = Phase::UpToDate;
                }
                Err(f) => {
                    tracing::warn!("Update check failed: {}", f.detail());
                    inner.offer = None;
                    inner.phase = Phase::Failed {
                        message: f.message(&self.current),
                    };
                }
            }
        }
        self.push(ctx).await;
        self.status()
    }

    /// The trader clicked Update. Refused unless a self-updating copy has an
    /// offer; asks first when something is running and `confirmed` does
    /// not cover it; otherwise starts the download (an owned task) and
    /// answers at once. Progress and the result arrive as `update_status`.
    pub async fn request_install(
        self: &Arc<Self>,
        ctx: &AppState,
        confirmed: Option<Activity>,
    ) -> Decision {
        let Some(backend) = self.backend() else {
            return Decision::Refused(
                Package::Unknown
                    .manual_reason()
                    .unwrap_or_default()
                    .to_string(),
            );
        };
        if let Some(reason) = backend.package().manual_reason() {
            return Decision::Refused(reason.to_string());
        }
        if let Some(refusal) = self.install_refusal() {
            return Decision::Refused(refusal);
        }
        let now = activity(ctx).await;
        if let Some(d) = confirm_unless_covered(now, confirmed, broker_name(ctx)) {
            return d;
        }
        let version = {
            let mut inner = self.inner.lock();
            // Claim under the lock that checks: a second click while the
            // first is downloading changes nothing.
            let can_start = matches!(inner.phase, Phase::Available | Phase::Failed { .. });
            match (&inner.offer, can_start) {
                (Some(offer), true) => {
                    let v = offer.version.clone();
                    inner.phase = Phase::Downloading {
                        received: 0,
                        total: None,
                    };
                    v
                }
                _ => return Decision::Refused(refusal_for(&inner)),
            }
        };
        tracing::info!("Update: downloading version {}", version);
        self.push(ctx).await;
        let svc = self.clone();
        let weak: Weak<AppState> = match ctx.arc() {
            Some(a) => Arc::downgrade(&a),
            None => Weak::new(),
        };
        ctx.spawn(async move { svc.run_download(weak, backend, version).await });
        Decision::Proceed(self.status())
    }

    fn install_refusal(&self) -> Option<String> {
        let inner = self.inner.lock();
        match (&inner.phase, &inner.offer) {
            (Phase::Available | Phase::Failed { .. }, Some(_)) => None,
            _ => Some(refusal_for(&inner)),
        }
    }

    async fn run_download(
        self: Arc<Self>,
        ctx: Weak<AppState>,
        backend: Arc<dyn UpdateBackend>,
        version: String,
    ) {
        let progress_svc = self.clone();
        let progress: ProgressFn = Box::new(move |received, total| {
            let mut inner = progress_svc.inner.lock();
            if let Phase::Downloading { .. } = inner.phase {
                inner.phase = Phase::Downloading { received, total };
            }
        });
        let download = backend.download(&version, progress);
        tokio::pin!(download);
        let mut tick = tokio::time::interval(PROGRESS_EVERY);
        tick.tick().await;
        let result = loop {
            tokio::select! {
                r = &mut download => break r,
                _ = tick.tick() => {
                    if let Some(c) = ctx.upgrade() {
                        self.push(&c).await;
                    }
                }
            }
        };
        {
            let mut inner = self.inner.lock();
            match &result {
                Ok(()) => {
                    tracing::info!("Update: version {} is downloaded and verified", version);
                    inner.phase = Phase::Ready;
                }
                Err(f) => {
                    tracing::warn!("Update download failed: {}", f.detail());
                    inner.phase = Phase::Failed {
                        message: f.download_message(&self.current),
                    };
                }
            }
        }
        if let Some(c) = ctx.upgrade() {
            self.push(&c).await;
        }
    }

    /// The trader chose Later after the download: forget the downloaded
    /// update (it is held in memory) and offer it again.
    pub async fn cancel(&self, ctx: &AppState) -> UpdateStatus {
        let discard = {
            let mut inner = self.inner.lock();
            if inner.phase == Phase::Ready {
                inner.phase = Phase::Available;
                true
            } else {
                false
            }
        };
        if discard {
            if let Some(b) = self.backend() {
                b.discard();
            }
            self.push(ctx).await;
        }
        self.status()
    }

    /// The window asks to restart into the downloaded update. Goes ahead
    /// only with a verified update waiting and everything running within
    /// what the trader confirmed; then the state is `restarting` and the
    /// shell installs and restarts.
    pub async fn begin_restart(&self, ctx: &AppState, confirmed: Option<Activity>) -> Decision {
        {
            let inner = self.inner.lock();
            if inner.phase != Phase::Ready {
                return Decision::Refused(refusal_for(&inner));
            }
        }
        let now = activity(ctx).await;
        if let Some(d) = confirm_unless_covered(now, confirmed, broker_name(ctx)) {
            return d;
        }
        {
            let mut inner = self.inner.lock();
            if inner.phase != Phase::Ready {
                return Decision::Refused(refusal_for(&inner));
            }
            inner.phase = Phase::Restarting;
        }
        tracing::info!("Update: restarting to install the update");
        self.push(ctx).await;
        Decision::Proceed(self.status())
    }

    /// Installing failed after [`Self::begin_restart`]: OpenAlgo stays on
    /// this version and says why.
    pub async fn install_failed(&self, ctx: &AppState, failure: UpdateFailure) {
        tracing::error!("Update install failed: {}", failure.detail());
        if let Some(b) = self.backend() {
            b.discard();
        }
        {
            let mut inner = self.inner.lock();
            inner.phase = Phase::Failed {
                message: failure.message(&self.current),
            };
        }
        self.push(ctx).await;
    }
}

/// Why Update or a restart cannot go ahead in the current state.
fn refusal_for(inner: &Inner) -> String {
    match (&inner.phase, &inner.offer) {
        (Phase::Downloading { .. }, _) => "The update is already downloading.".into(),
        (Phase::Restarting, _) => "OpenAlgo is already restarting to install the update.".into(),
        (Phase::Ready, _) => "The update is downloaded. Restart OpenAlgo to install it.".into(),
        (Phase::Checking, _) => "OpenAlgo is checking for updates. Try again in a moment.".into(),
        (Phase::Available | Phase::Failed { .. }, Some(_)) => {
            "Download the update first: choose Update.".into()
        }
        _ => "There is no update to install. Check for updates first.".into(),
    }
}

fn confirm_unless_covered(
    now: Activity,
    confirmed: Option<Activity>,
    broker: Option<String>,
) -> Option<Decision> {
    if now.is_quiet() || confirmed.is_some_and(|c| now.covered_by(&c)) {
        return None;
    }
    let warning = now.warning(broker.as_deref())?;
    Some(Decision::Confirm {
        activity: now,
        warning,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> semver::Version {
        semver::Version::parse(s).unwrap()
    }

    /// Prerelease tags: precedence, never a stable build onto a
    /// pre-release, build metadata ignored.
    #[test]
    fn newer_follows_semver_precedence_and_keeps_stable_builds_on_stable() {
        let cases = [
            ("1.0.0", "1.0.1", true),
            ("1.0.0", "1.1.0", true),
            ("1.0.0", "2.0.0", true),
            ("1.0.0", "1.0.0", false),
            ("1.0.1", "1.0.0", false),
            // A stable build is never offered a pre-release.
            ("1.0.0", "1.1.0-beta.1", false),
            ("1.1.0", "1.1.0-beta.3", false),
            // A pre-release build follows pre-releases and the release.
            ("1.1.0-beta.1", "1.1.0-beta.2", true),
            ("1.1.0-beta.2", "1.1.0-beta.10", true),
            ("1.1.0-beta.10", "1.1.0-beta.2", false),
            ("1.1.0-alpha.1", "1.1.0-beta.1", true),
            ("1.1.0-rc.1", "1.1.0", true),
            ("1.1.0-rc.1", "1.0.9", false),
            ("1.1.0-rc.1", "1.2.0-alpha.1", true),
            // Build metadata does not make a version newer.
            ("1.0.0+build.1", "1.0.0+build.2", false),
            ("1.0.0", "1.0.1+ci.7", true),
        ];
        for (current, offered, want) in cases {
            assert_eq!(
                is_newer(&v(current), &v(offered)),
                want,
                "{} -> {}",
                current,
                offered
            );
        }
    }

    #[test]
    fn only_the_three_self_updating_packages_install_themselves() {
        for p in [
            Package::MacApp,
            Package::WindowsNsis,
            Package::LinuxAppImage,
        ] {
            assert!(p.self_updates());
            assert_eq!(p.manual_reason(), None);
        }
        for p in [
            Package::LinuxDeb,
            Package::LinuxRpm,
            Package::Development,
            Package::Unknown,
        ] {
            assert!(!p.self_updates());
            assert!(p.manual_reason().is_some());
        }
        assert!(Package::LinuxDeb
            .manual_reason()
            .unwrap()
            .contains(".deb package, which OpenAlgo cannot update by itself"));
    }

    #[test]
    fn a_live_warning_names_every_item_in_a_traders_words() {
        let a = Activity {
            live_strategies: 2,
            live_openscript_runs: 1,
            live_scalping_stops: 1,
            live_open_orders: 3,
            sandbox_open_orders: 2,
            ..Default::default()
        };
        let w = a.warning(Some("Zerodha")).unwrap();
        assert_eq!(w.level, "live");
        assert_eq!(w.title, "Live trading is running");
        assert_eq!(
            w.items,
            vec![
                "2 running Live strategies. Their stops, targets and exits are not checked until OpenAlgo opens again; then they will resume.",
                "1 running Live OpenScript run. It is paused when OpenAlgo closes; start it again after the update.",
                "1 Live position under scalping protection. Stops and targets are not checked until OpenAlgo opens again; then protection resumes.",
                "3 open orders at Zerodha. They stay at your broker; OpenAlgo follows them again once it opens.",
                "Sandbox: 2 open sandbox orders. Nothing there is sent to your broker.",
            ]
        );
        let unknown = Activity {
            live_orders_unknown: true,
            ..Default::default()
        };
        assert_eq!(
            unknown.warning(Some("Fyers")).unwrap().items,
            vec!["OpenAlgo could not read your open orders at Fyers. Check your order book before you update."]
        );
    }

    #[test]
    fn sandbox_only_activity_gets_the_lighter_warning() {
        let a = Activity {
            sandbox_strategies: 1,
            sandbox_openscript_runs: 2,
            sandbox_open_orders: 1,
            ..Default::default()
        };
        assert!(!a.has_live());
        let w = a.warning(None).unwrap();
        assert_eq!(w.level, "sandbox");
        assert_eq!(
            w.items,
            vec![
                "1 sandbox strategy.",
                "2 sandbox OpenScript runs (paused; start them again after the update).",
                "1 open sandbox order.",
            ]
        );
        assert!(w.advice.starts_with("Nothing is sent to your broker."));
        assert_eq!(Activity::default().warning(None), None);
    }

    #[test]
    fn a_confirmation_covers_only_what_the_trader_saw() {
        let seen = Activity {
            live_strategies: 1,
            sandbox_open_orders: 2,
            ..Default::default()
        };
        assert!(seen.covered_by(&seen));
        // Less is still covered: a strategy stopped since.
        assert!(Activity::default().covered_by(&seen));
        // More is not: a new Live order, an unreadable order book.
        let more = Activity {
            live_open_orders: 1,
            ..seen
        };
        assert!(!more.covered_by(&seen));
        let unknown = Activity {
            live_orders_unknown: true,
            ..seen
        };
        assert!(!unknown.covered_by(&seen));
        assert!(confirm_unless_covered(seen, Some(seen), None).is_none());
        assert!(matches!(
            confirm_unless_covered(more, Some(seen), None),
            Some(Decision::Confirm { .. })
        ));
        assert!(matches!(
            confirm_unless_covered(seen, None, None),
            Some(Decision::Confirm { .. })
        ));
        assert!(confirm_unless_covered(Activity::default(), None, None).is_none());
    }

    #[test]
    fn the_confirmation_round_trips_through_json() {
        let a = Activity {
            live_scalping_stops: 1,
            live_orders_unknown: true,
            ..Default::default()
        };
        let back: Activity = serde_json::from_value(serde_json::to_value(a).unwrap()).unwrap();
        assert_eq!(back, a);
        // Missing fields read as zero.
        let partial: Activity =
            serde_json::from_value(serde_json::json!({"live_strategies": 2})).unwrap();
        assert_eq!(partial.live_strategies, 2);
        assert!(!partial.checks_failed);
    }

    #[test]
    fn failures_tell_the_trader_the_cause_and_the_next_step() {
        let sig = UpdateFailure::Signature("minisign: bad signature".into());
        let m = sig.message("1.0.0");
        assert!(m.contains("failed its signature check, so it was not installed"));
        assert!(m.contains("still on version 1.0.0"));
        assert!(!m.contains("minisign"));
        assert!(UpdateFailure::Network("dns".into())
            .message("1.0.0")
            .contains("Check your internet connection"));
    }

    #[test]
    fn the_release_page_of_an_offer_names_its_tag() {
        let o = Offer::new("1.2.0-beta.1", Some("  ".into()), None);
        assert_eq!(
            o.release_url,
            "https://github.com/marketcalls/openalgo-desktop/releases/tag/v1.2.0-beta.1"
        );
        assert_eq!(o.notes, None);
    }
}
