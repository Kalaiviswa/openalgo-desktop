//! A fake broker that answers every request with one scripted reply, for
//! the cross-adapter tables (`broker_order_outcomes`,
//! `broker_malformed_replies`, `broker_unread_routes`).

use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Router;
use openalgo_desktop_lib::brokers::common::symbols::{SymToken, SymbolResolver};
use parking_lot::Mutex;
use std::sync::Arc;

/// What the fake answers.
#[derive(Clone)]
pub struct Answer {
    pub status: u16,
    pub body: String,
}

pub fn answer(status: u16, body: &str) -> Answer {
    Answer {
        status,
        body: body.to_string(),
    }
}

pub struct Fake {
    /// The answer to every request no route matches.
    pub answer: Arc<Mutex<Answer>>,
    /// Answers for the requests whose path and query contain a key,
    /// before `answer`.
    pub routes: Arc<Mutex<Vec<(&'static str, Answer)>>>,
    /// The path and query of every request, in order.
    pub calls: Arc<Mutex<Vec<String>>>,
    pub base: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Fake {
    /// Answer every request with `status` and `body` from now on.
    pub fn reply(&self, status: u16, body: &str) {
        *self.answer.lock() = answer(status, body);
    }

    /// Answer the requests whose path and query contain `key` with
    /// `status` and `body`, ahead of the default answer.
    pub fn route(&self, key: &'static str, status: u16, body: &str) {
        let mut routes = self.routes.lock();
        routes.retain(|(k, _)| *k != key);
        routes.push((key, answer(status, body)));
    }

    /// How many requests went to a path and query containing `key`.
    pub fn sent(&self, key: &str) -> usize {
        self.calls.lock().iter().filter(|c| c.contains(key)).count()
    }
}

pub async fn fake() -> Fake {
    let answer = Arc::new(Mutex::new(Answer {
        status: 200,
        body: "{}".into(),
    }));
    let routes: Arc<Mutex<Vec<(&'static str, Answer)>>> = Arc::default();
    let calls: Arc<Mutex<Vec<String>>> = Arc::default();
    let (a, r, c) = (answer.clone(), routes.clone(), calls.clone());
    let app = Router::new().fallback(move |uri: Uri| {
        let target = uri
            .path_and_query()
            .map_or_else(|| uri.path().to_string(), |p| p.to_string());
        c.lock().push(target.clone());
        let a = r
            .lock()
            .iter()
            .find(|(p, _)| target.contains(p))
            .map(|(_, a)| a.clone())
            .unwrap_or_else(|| a.lock().clone());
        async move {
            let r: Response = (
                StatusCode::from_u16(a.status).unwrap(),
                [("content-type", "application/json")],
                a.body,
            )
                .into_response();
            r
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Fake {
        answer,
        routes,
        calls,
        base,
        task,
    }
}

/// A master with SBIN on NSE.
pub fn symbols() -> SymbolResolver {
    let r = SymbolResolver::new();
    r.load(vec![sbin()]);
    r
}

/// SBIN on NSE, as the brokers' masters list it.
pub fn sbin() -> SymToken {
    SymToken {
        symbol: "SBIN".into(),
        brsymbol: "SBIN-EQ".into(),
        name: "SBIN".into(),
        exchange: "NSE".into(),
        brexchange: "NSE".into(),
        token: "3045".into(),
        expiry: String::new(),
        strike: 0.0,
        lot_size: 1,
        instrument_type: "EQ".into(),
        tick_size: 0.05,
    }
}
