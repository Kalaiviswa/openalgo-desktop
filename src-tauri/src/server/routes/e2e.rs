//! Routes of CI's native E2E test build (the `e2e-broker` feature,
//! `src/e2e.rs`); there are none in any other build.
//!
//! `GET /e2e/scripted-broker`, for the signed-in user: what the scripted
//! broker has received (`{"calls": n, "order_calls": n}`), so the run shows
//! by a count, not by a log line that might be missing for another reason,
//! that a sandbox order never reached the broker.

use super::RouteSpec;

/// The test build's routes; empty in every other build.
pub fn table() -> Vec<RouteSpec> {
    #[cfg(feature = "e2e-broker")]
    {
        vec![scripted::route()]
    }
    #[cfg(not(feature = "e2e-broker"))]
    {
        Vec::new()
    }
}

#[cfg(feature = "e2e-broker")]
mod scripted {
    use super::super::Access;
    use super::RouteSpec;
    use crate::server::routes::webui::{no_store, ok};
    use axum::{http::Method, response::Response, routing::get};
    use serde_json::json;

    pub(super) fn route() -> RouteSpec {
        RouteSpec {
            path: "/e2e/scripted-broker",
            method: Method::GET,
            access: Access::User,
            make: || get(counts),
        }
    }

    async fn counts() -> Response {
        no_store(ok(json!(crate::brokers::scripted::counts())))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_the_test_build_has_the_count_route() {
        let paths: Vec<&str> = super::table().iter().map(|r| r.path).collect();
        if cfg!(feature = "e2e-broker") {
            assert_eq!(paths, ["/e2e/scripted-broker"]);
        } else {
            assert!(paths.is_empty());
        }
    }
}
