//! Test-only build for CI's Windows native end-to-end run
//! (`.github/workflows/native-e2e-windows.yml`): the main window's WebView2
//! opens a remote debugging port, so msedgedriver can attach to it and
//! drive the installed app through its own window. WebView2 does not take
//! that switch from the environment for an app that sets its own browser
//! arguments, as Tauri does (runs 38275143364 and 38280440540), so the app
//! has to ask for it itself.
//!
//! Guardrails:
//! - the feature (`e2e-debug-port`) does not build unless the build is a
//!   debug one or `OPENALGO_E2E_BUILD=1` is set for it (build.rs and the
//!   `compile_error!` in lib.rs), and does nothing off Windows;
//! - the port comes from `OPENALGO_E2E_DEBUG_PORT` at start (the CI script
//!   picks a free one per run); without it nothing is opened. Chromium's
//!   debugging server listens on 127.0.0.1 only;
//! - release.yml never sets the feature, and the install smoke and the
//!   release check that the shipped Windows binary carries none of these
//!   strings.
//!
//! Hidden OpenScript runner windows keep the default arguments; WebView2
//! refuses webviews with different arguments in one data folder, so the
//! runner is not available in this build.

/// The debugging port asked for at start, if any (1024 and above).
pub(crate) fn debug_port() -> Option<u16> {
    std::env::var("OPENALGO_E2E_DEBUG_PORT")
        .ok()
        .and_then(|p| p.trim().parse::<u16>().ok())
        .filter(|p| *p >= 1024)
}

/// wry's own default arguments (kept, since giving any replaces them)
/// plus the debugging port.
pub(crate) fn browser_args(port: u16) -> String {
    format!(
        "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --remote-debugging-port={}",
        port
    )
}

#[cfg(test)]
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
