//! NEW-SEC-01: browser origins are compared whole (scheme, host and port),
//! apart from the `Host` names the server answers to. With a public origin
//! configured, the same name on another port or scheme is another site.

use super::*;
use crate::server::origin;

fn set_settings(h: &H, column: &str, value: &str) {
    h.ctx()
        .sqlite
        .conn()
        .unwrap()
        .execute(
            &format!("UPDATE settings SET {} = ?1 WHERE id = 1", column),
            [value],
        )
        .unwrap();
    h.ctx().reload_config().unwrap();
}

const PUBLIC: &str = "https://trader.example";

#[tokio::test]
async fn only_this_apps_exact_origins_are_trusted() {
    let h = H::new();
    set_settings(&h, "host_server", PUBLIC);
    let port = h.ctx().server_config().http_port;
    let ok = |o: &str, host: Option<&str>| origin::allowed(h.ctx(), o, host);

    // The configured public origin, exactly; the same name elsewhere is not.
    assert!(ok(PUBLIC, None));
    for other in [
        "https://trader.example:8443",
        "http://trader.example:8000",
        "http://trader.example",
        "https://evil.example",
        "https://sub.trader.example",
    ] {
        assert!(!ok(other, None), "{}", other);
    }
    // Malformed or decorated values.
    for bad in [
        "null",
        "",
        "trader.example",
        "https://trader.example/",
        "https://trader.example/path",
        "https://user@trader.example",
        "https://trader.example?x=1",
        "javascript:alert(1)",
        "file:///etc/passwd",
    ] {
        assert!(!ok(bad, None), "{:?}", bad);
    }
    // The app on this computer: its port, plain http, loopback names.
    for local in [
        format!("http://127.0.0.1:{}", port),
        format!("http://localhost:{}", port),
        format!("http://[::1]:{}", port),
    ] {
        assert!(ok(&local, None), "{}", local);
    }
    assert!(!ok(&format!("https://127.0.0.1:{}", port), None));
    assert!(!ok(
        &format!("http://127.0.0.1:{}", port.wrapping_add(1)),
        None
    ));
    // Another machine's address, on the app's port: never on a loopback
    // bind; with access from other devices on, only the address the
    // request was sent to.
    let lan = format!("http://203.0.113.99:{}", port);
    let to_lan = format!("203.0.113.99:{}", port);
    let to_other = format!("192.168.1.5:{}", port);
    assert!(!ok(&lan, Some(&to_lan)));
    set_settings(&h, "bind_host", "0.0.0.0");
    assert!(ok(&lan, Some(&to_lan)));
    assert!(!ok(&lan, Some(&to_other)), "another machine's page");
    assert!(!ok(&lan, None));
    assert!(!ok(
        &format!("https://203.0.113.99:{}", port),
        Some(&to_lan)
    ));
}

/// A credentialed GET from another port of the public name (a page that
/// could read the profile's TOTP seed or the API key) gets no CORS grant;
/// the exact origin still does.
#[tokio::test]
async fn a_credentialed_get_from_another_port_of_the_public_name_is_not_granted() {
    let h = H::new();
    h.setup();
    set_settings(&h, "host_server", PUBLIC);
    let (cookie, _) = h.session(true);
    let read = |origin: &str| {
        let mut r = with_session(get("/auth/profile-data"), &cookie, None);
        r.headers_mut()
            .insert(header::ORIGIN, origin.parse().unwrap());
        r
    };
    // No Access-Control-Allow-Origin: the browser keeps the answer from
    // the page (tower-http sends Allow-Credentials on every answer; without
    // the origin grant it grants nothing).
    let (_, headers, _) = h.send(read("https://trader.example:8443")).await;
    assert!(headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_none());
    let (_, headers, _) = h.send(read(PUBLIC)).await;
    assert_eq!(headers[header::ACCESS_CONTROL_ALLOW_ORIGIN], PUBLIC);
    assert_eq!(headers[header::ACCESS_CONTROL_ALLOW_CREDENTIALS], "true");
}

/// A write with the session and its CSRF token from another port of the
/// public name is refused before the handler; the exact origin passes the
/// origin check.
#[tokio::test]
async fn a_write_from_another_port_of_the_public_name_is_refused() {
    let h = H::new();
    h.setup();
    set_settings(&h, "host_server", PUBLIC);
    let (cookie, csrf) = h.session(true);
    let write = |origin: &str| {
        let mut r = with_session(post_json("/auth/logout", json!({})), &cookie, Some(&csrf));
        r.headers_mut()
            .insert(header::ORIGIN, origin.parse().unwrap());
        r
    };
    let (s, _, b) = h.send(write("https://trader.example:8443")).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{}", String::from_utf8_lossy(&b));
    let (s, _, _) = h.send(write("http://trader.example:8000")).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = h.send(write(PUBLIC)).await;
    assert_ne!(s, StatusCode::FORBIDDEN);
}
