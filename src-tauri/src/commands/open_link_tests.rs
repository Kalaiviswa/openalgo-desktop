//! A web link on the app page opens in the trader's browser: the page may
//! call the shell's `open`, and the shipped link rule lets an http(s)
//! address through. Both halves failed silently before 1.0.4: the rule was
//! written already anchored (`^https?://`), the plugin anchors it again
//! (`^^https?://$`), and no real address matched, so the ChatGPT sign-in
//! page and the update notice's release page never opened.

/// The shipped rule as the shell plugin compiles it (`tauri-plugin-shell`
/// 2.5.0, `lib.rs`: `format!("^{validator}$")`).
fn shipped_link_rule() -> regex::Regex {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json");
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let rule = config["plugins"]["shell"]["open"].as_str().unwrap();
    regex::Regex::new(&format!("^{}$", rule)).unwrap()
}

#[test]
fn the_shipped_link_rule_lets_web_addresses_through_and_nothing_else() {
    let rule = shipped_link_rule();
    for url in [
        "https://auth.openai.com/codex/device",
        "https://github.com/marketcalls/openalgo-desktop/releases/latest",
        "http://example.com/a?b=c#d",
    ] {
        assert!(rule.is_match(url), "{} is refused", url);
    }
    for url in [
        "https://",
        "file:///etc/hosts",
        "javascript:alert(1)",
        "mailto:someone@example.com",
        "ftp://example.com/",
        " https://example.com/",
        "https://example.com/ with a space",
    ] {
        assert!(!rule.is_match(url), "{} is let through", url);
    }
}

/// The permission half runs on the mock runtime, which is not built on
/// Windows (CLAUDE.md, "Builds and CI").
#[cfg(not(windows))]
mod permission {
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{get_ipc_response, mock_builder, INVOKE_KEY};
    use tauri::webview::InvokeRequest;

    /// The page asks the shell to open `path`; the answer the page gets.
    fn open_from(page: &str, path: &str) -> String {
        let app = mock_builder()
            .plugin(tauri_plugin_shell::init())
            .build(tauri::generate_context!(test = true))
            .unwrap();
        super::super::trust_app_origin(app.handle(), 5000);
        let window = tauri::WebviewWindowBuilder::new(
            &app,
            "main",
            tauri::WebviewUrl::External(page.parse().unwrap()),
        )
        .build()
        .unwrap();
        let answer = get_ipc_response(
            &window,
            InvokeRequest {
                cmd: "plugin:shell|open".into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: page.parse().unwrap(),
                body: InvokeBody::Json(serde_json::json!({ "path": path })),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.to_string(),
            },
        );
        format!("{:?}", answer)
    }

    /// The app page reaches the link rule (a mail link is refused by the rule,
    /// not by a missing permission); a page on another port does not. An
    /// address the rule allows is not tried here: it would open a browser.
    #[test]
    fn the_app_page_may_ask_for_a_link_and_another_page_may_not() {
        let app_page = open_from("http://127.0.0.1:5000/agent/config", "mailto:x");
        assert!(app_page.contains("failed regex validation"), "{}", app_page);
        let other = open_from("http://127.0.0.1:6000/agent/config", "mailto:x");
        assert!(other.contains("not allowed"), "{}", other);
    }
}
