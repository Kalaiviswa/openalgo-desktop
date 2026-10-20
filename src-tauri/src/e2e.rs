//! CI's native end-to-end test build (`.github/workflows/native-e2e-test-build.yml`,
//! `scripts/ci/native_e2e.py`). Two features, each only for that build:
//!
//! - `e2e-broker` (Linux and Windows): the app signs in to a scripted
//!   broker (`brokers::scripted`) in the place of Dhan (Sandbox), so a
//!   sandbox order can be placed from the order form against a market of
//!   known prices, and the run can show by the broker's own count
//!   (`GET /e2e/scripted-broker`, `server::routes::e2e`) that it never
//!   reached the broker.
//! - `e2e-debug-port` (Windows): the main window's WebView2 opens a remote
//!   debugging port, so msedgedriver can attach to it and drive the app
//!   through its own window. WebView2 does not take that switch from the
//!   environment for an app that sets its own browser arguments, as Tauri
//!   does (runs 38275143364 and 38280440540), so the app has to ask for it
//!   itself.
//!
//! Guardrails:
//! - neither feature builds unless the build is a debug one or
//!   `OPENALGO_E2E_BUILD=1` is set for it (build.rs and the
//!   `compile_error!` in lib.rs; the workflow's Linux job shows the build
//!   failing without it);
//! - without `e2e-broker` every adapter is the production one and the count
//!   route does not exist (the tests in lib.rs and `server::routes::e2e`,
//!   which the workflow also runs with the feature on);
//! - the debugging port comes from `OPENALGO_E2E_DEBUG_PORT` at start (the
//!   CI script picks a free one per run); without it nothing is opened.
//!   Chromium's debugging server listens on 127.0.0.1 only, which the
//!   script checks;
//! - release.yml never sets either feature, and the install smoke and the
//!   release check that every shipped binary carries none of their strings
//!   (`scripts/ci/check_no_e2e_build.py`).
//!
//! On Windows, hidden OpenScript runner windows keep the default arguments;
//! WebView2 refuses webviews with different arguments in one data folder,
//! so the runner is not available in the debug-port build.

#[cfg(feature = "e2e-broker")]
use crate::state::OpenOptions;
#[cfg(feature = "e2e-broker")]
use std::sync::Arc;

/// The installed app's stores, with the scripted broker in its slot. The
/// scripted market's expiry follows the day the app starts (IST).
#[cfg(feature = "e2e-broker")]
pub(crate) fn with_scripted_broker(options: OpenOptions) -> OpenOptions {
    use crate::brokers::scripted;
    let today = options
        .clock
        .now()
        .with_timezone(&chrono_tz::Asia::Kolkata)
        .date_naive();
    tracing::warn!(
        "{}: this test build signs in to a scripted broker in the place of {}",
        scripted::MARKER,
        scripted::SLOT
    );
    let (brokers, _) = scripted::registry(&options.brokers, today);
    OpenOptions {
        brokers: Arc::new(brokers),
        ..options
    }
}

/// The debugging port asked for at start, if any (1024 and above).
#[cfg(all(windows, feature = "e2e-debug-port"))]
pub(crate) fn debug_port() -> Option<u16> {
    std::env::var("OPENALGO_E2E_DEBUG_PORT")
        .ok()
        .and_then(|p| p.trim().parse::<u16>().ok())
        .filter(|p| *p >= 1024)
}

/// wry's own default arguments (kept, since giving any replaces them)
/// plus the debugging port.
#[cfg(all(windows, feature = "e2e-debug-port"))]
pub(crate) fn browser_args(port: u16) -> String {
    format!(
        "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --remote-debugging-port={}",
        port
    )
}

#[cfg(all(test, windows, feature = "e2e-debug-port"))]
mod tests {
    use super::*;

    #[test]
    fn the_port_is_asked_for_explicitly_and_keeps_wrys_defaults() {
        let args = browser_args(45678);
        assert!(args.starts_with("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection "));
        assert!(args.ends_with(" --remote-debugging-port=45678"));
        assert!(!args.contains("remote-debugging-address"));
    }
}
