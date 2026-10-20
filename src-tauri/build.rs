fn main() {
    // The e2e-debug-port feature (src/e2e.rs) builds only where CI asks for
    // it in so many words; elsewhere src/e2e.rs stops the build.
    println!("cargo::rustc-check-cfg=cfg(e2e_build_allowed)");
    println!("cargo:rerun-if-env-changed=OPENALGO_E2E_BUILD");
    if std::env::var_os("CARGO_FEATURE_E2E_DEBUG_PORT").is_some()
        && std::env::var("OPENALGO_E2E_BUILD").as_deref() == Ok("1")
    {
        println!("cargo:rustc-cfg=e2e_build_allowed");
    }
    // App commands get explicit permissions (`allow-<command>`), so each one
    // is reachable only from a capability that names it: the bundled
    // start-up page (`default`) or the app's own server page (`remote-app`).
    let app = tauri_build::AppManifest::new().commands(&[
        "startup_status",
        "retry_server",
        "restart_server",
        "open_external",
        "reset_account",
        "start_fresh",
        "window_proof",
        "restart_to_update",
    ]);
    if let Err(e) = tauri_build::try_build(tauri_build::Attributes::new().app_manifest(app)) {
        panic!("tauri build failed: {e:#}");
    }
}
