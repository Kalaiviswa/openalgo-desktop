//! Which browser origins the app trusts (NEW-SEC-01), apart from which
//! `Host` names it answers to ([`super::middleware::host_allowed`], the
//! DNS-rebinding guard).
//!
//! An origin is a scheme, a host and a port, and is compared whole. It is
//! one of this app's when it is:
//!
//! - the app on this computer: `http://127.0.0.1`, `http://localhost` or
//!   `http://[::1]` on the app's port (and, in a debug build, the Vite dev
//!   server's 5173);
//! - the configured public origin (`host_server`, a tunnel's https address
//!   for example), exactly as configured: the same name on another port or
//!   scheme is another site, which may serve someone else's pages;
//! - with access from other devices on: `http://<IP address>:<app port>`
//!   where that address is the one the request itself was sent to (its
//!   `Host`), so the page came from this app at that address. Another
//!   machine's address is not.
//!
//! Anything else is not: `null`, a value with user info, a path, a query
//! or a fragment, one that does not parse, another scheme or port. Callers
//! that send no `Origin` (programs, not browsers) are decided elsewhere.

use crate::state::AppState;
use axum::http::{header, HeaderMap};
use url::{Host, Url};

/// The Vite dev server's port: debug builds load the page from it.
const DEV_SERVER_PORT: u16 = 5173;

/// Whether `origin` (an `Origin` header) is this app's; `request_host` is
/// the request's `Host` (or HTTP/2 authority), when it has one.
pub fn allowed(ctx: &AppState, origin: &str, request_host: Option<&str>) -> bool {
    let Some(o) = parse_origin(origin) else {
        return false;
    };
    let cfg = ctx.server_config();
    if cfg
        .host_server
        .as_deref()
        .and_then(|h| Url::parse(h.trim()).ok())
        .is_some_and(|public| same_origin(&public, &o))
    {
        return true;
    }
    let port = o.port_or_known_default();
    let app_port = port.is_some_and(|p| {
        p == cfg.http_port
            || p == ctx.listening_port()
            || (cfg!(debug_assertions) && p == DEV_SERVER_PORT)
    });
    if o.scheme() != "http" || !app_port {
        return false;
    }
    match o.host() {
        Some(Host::Domain(d)) => d == "localhost",
        Some(Host::Ipv4(ip)) if ip.is_loopback() => true,
        Some(Host::Ipv6(ip)) if ip.is_loopback() => true,
        // Another device: the address the request was sent to.
        Some(Host::Ipv4(_)) | Some(Host::Ipv6(_)) => {
            !cfg.is_loopback()
                && request_host
                    .and_then(|h| Url::parse(&format!("http://{}", h.trim())).ok())
                    .is_some_and(|r| same_origin(&r, &o))
        }
        None => false,
    }
}

/// [`allowed`] for the origin of a `Referer` (a full page address).
pub fn referer_allowed(ctx: &AppState, referer: &str, request_host: Option<&str>) -> bool {
    let Ok(u) = Url::parse(referer.trim()) else {
        return false;
    };
    if !matches!(u.scheme(), "http" | "https") || !u.username().is_empty() || u.password().is_some()
    {
        return false;
    }
    allowed(ctx, &u.origin().ascii_serialization(), request_host)
}

/// The request's `Host`, for [`allowed`].
pub fn request_host(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::HOST).and_then(|v| v.to_str().ok())
}

/// `origin` as an origin and nothing more, or None.
fn parse_origin(origin: &str) -> Option<Url> {
    if origin == "null" || origin.ends_with('/') {
        return None;
    }
    let u = Url::parse(origin).ok()?;
    let plain = matches!(u.scheme(), "http" | "https")
        && u.username().is_empty()
        && u.password().is_none()
        && u.path() == "/"
        && u.query().is_none()
        && u.fragment().is_none()
        && u.host().is_some();
    plain.then_some(u)
}

fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host() == b.host()
        && a.port_or_known_default() == b.port_or_known_default()
}
