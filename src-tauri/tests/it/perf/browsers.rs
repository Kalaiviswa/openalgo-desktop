//! Browsers that keep reloading pages while a benchmark measures the app:
//! each asks for its pages in turn, the next as soon as the last answer
//! came, until told to stop (or the server goes, when the measurement
//! ends).

use super::rig::Rig;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// One request a page makes: signed in with the rig's cookie, or with its
/// API key in a JSON body (`/api/v1`).
#[derive(Clone, Debug)]
pub enum Page {
    Get(String),
    ApiPost(String),
}

impl Page {
    pub fn get(path: impl Into<String>) -> Page {
        Page::Get(path.into())
    }
}

/// What one browser saw: how long each answer took, and how many answers
/// of each status.
#[derive(Default)]
pub struct Seen {
    pub took: Vec<Duration>,
    pub statuses: BTreeMap<u16, usize>,
}

impl Seen {
    pub fn merge(mut self, other: Seen) -> Seen {
        self.took.extend(other.took);
        for (s, n) in other.statuses {
            *self.statuses.entry(s).or_default() += n;
        }
        self
    }

    /// `200:503,429:2`
    pub fn status_summary(&self) -> String {
        self.statuses
            .iter()
            .map(|(s, n)| format!("{}:{}", s, n))
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// `n` browsers on `pages`, each starting at a different page.
pub fn start(
    rig: &Rig,
    pages: Vec<Page>,
    n: usize,
    stop: Arc<AtomicBool>,
) -> Vec<tokio::task::JoinHandle<Seen>> {
    let pages = Arc::new(pages);
    (0..n)
        .map(|i| browser(rig, pages.clone(), i, stop.clone()))
        .collect()
}

/// Every browser's answers together.
pub async fn collect(browsers: Vec<tokio::task::JoinHandle<Seen>>) -> Seen {
    let mut all = Seen::default();
    for b in browsers {
        all = all.merge(b.await.unwrap());
    }
    all
}

fn browser(
    rig: &Rig,
    pages: Arc<Vec<Page>>,
    first: usize,
    stop: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<Seen> {
    let (base, cookie, key) = (rig.base.clone(), rig.cookie.clone(), rig.key.clone());
    tokio::spawn(async move {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .unwrap();
        let mut seen = Seen::default();
        let mut i = first;
        while !stop.load(Ordering::Relaxed) {
            let page = &pages[i % pages.len()];
            i += 1;
            let req = match page {
                Page::Get(path) => http
                    .get(format!("{}{}", base, path))
                    .header("cookie", &cookie)
                    .header("accept", "application/json"),
                Page::ApiPost(path) => http
                    .post(format!("{}{}", base, path))
                    .json(&serde_json::json!({ "apikey": key })),
            };
            let t = Instant::now();
            match req.send().await {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let _ = resp.bytes().await;
                    seen.took.push(t.elapsed());
                    *seen.statuses.entry(status).or_default() += 1;
                }
                // The rig stopped the server at the end of the measurement.
                Err(_) => break,
            }
        }
        seen
    })
}

/// The answer of `page` once, as JSON (to check a seeded page answers).
pub async fn fetch_json(rig: &Rig, page: &Page) -> (u16, Value) {
    let http = reqwest::Client::new();
    let req = match page {
        Page::Get(path) => http
            .get(format!("{}{}", rig.base, path))
            .header("cookie", &rig.cookie)
            .header("accept", "application/json"),
        Page::ApiPost(path) => http
            .post(format!("{}{}", rig.base, path))
            .json(&serde_json::json!({ "apikey": rig.key })),
    };
    let resp = req.send().await.unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or_default())
}
