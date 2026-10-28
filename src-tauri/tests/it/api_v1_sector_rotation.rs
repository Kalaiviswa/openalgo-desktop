//! Web test/test_sector_rotation_api.py (openalgo b7cdc028e): the
//! guarantees of `/api/v1/sectorrotation`, against the desktop server. An
//! invalid key never reaches the service, a broker-sourced map needs a
//! broker session, nothing arrives unvalidated, a valid request passes its
//! symbols through, and the options list the map's choices.

use crate::api_v1_support::H;
use axum::body::Body;
use axum::http::Request;
use openalgo_desktop_lib::brokers::mock::MockCall;
use openalgo_desktop_lib::brokers::types::Candle;
use serde_json::{json, Value};

const REFUSED: &str = "These map settings were not accepted. Choose the benchmark and timeframe again, or use Reset to sectors if you added symbols.";

fn body(h: &H, overrides: Value) -> Value {
    let mut b = json!({"benchmark": "NIFTY", "timeframe": "daily", "source": "db"});
    for (k, v) in overrides.as_object().unwrap() {
        b[k] = v.clone();
    }
    h.with_key(b)
}

async fn post(h: &H, b: Value) -> (u16, Value) {
    let (s, v) = h.post("/api/v1/sectorrotation", b).await;
    (s.as_u16(), v)
}

fn history_reads(h: &H) -> usize {
    h.mock
        .calls
        .lock()
        .iter()
        .filter(|c| matches!(c, MockCall::History(_)))
        .count()
}

/// test_an_invalid_api_key_never_reaches_the_service, on both sources.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_invalid_api_key_never_reaches_the_service() {
    let h = H::new().await;
    for source in ["db", "api"] {
        let mut b = body(&h, json!({"source": source}));
        b["apikey"] = json!("not-the-key");
        let (s, v) = post(&h, b).await;
        assert_eq!(s, 403, "{}", v);
        assert_eq!(v["message"], "Invalid openalgo apikey");
    }
    assert_eq!(history_reads(&h), 0);
    h.shutdown().await;
}

/// test_broker_source_requires_a_broker_session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn broker_source_requires_a_broker_session() {
    let h = H::new().await;
    h.ctx.set_broker_session(None);
    let (s, v) = post(&h, body(&h, json!({"source": "api"}))).await;
    assert_eq!(s, 403);
    assert!(
        v["message"].as_str().unwrap().contains("broker login"),
        "{}",
        v
    );
    h.shutdown().await;
}

/// test_invalid_input_is_refused_before_the_service.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_input_is_refused_before_the_service() {
    let h = H::new().await;
    let many: Vec<Value> = (0..41)
        .map(|i| json!({"symbol": format!("S{}", i), "exchange": "NSE"}))
        .collect();
    for overrides in [
        json!({"benchmark": "DOWJONES"}),
        json!({"timeframe": "monthly"}),
        json!({"source": "csv"}),
        json!({"instruments": [{"symbol": "NIFTY30OCT26FUT", "exchange": "NFO"}]}),
        json!({"instruments": [{"exchange": "NSE"}]}),
        json!({"instruments": many}),
        json!({"unexpected": 1}),
    ] {
        let (s, v) = post(&h, body(&h, overrides.clone())).await;
        assert_eq!(
            (s, v),
            (400, json!({"status": "error", "message": REFUSED})),
            "{}",
            overrides
        );
    }
    let (s, _) = post(&h, json!({"benchmark": "NIFTY"})).await;
    assert_eq!(s, 400, "the key is required");
    assert_eq!(history_reads(&h), 0);
    h.shutdown().await;
}

/// test_valid_request_passes_the_instrument_list_through: from the broker,
/// RELIANCE is read and placed against Nifty 50.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_valid_request_places_its_symbols() {
    let h = H::new().await;
    // 300 sessions ending today, IST midnight stamps.
    let today = h.ctx.now().timestamp() / 86_400 * 86_400 - 19_800;
    let bars: Vec<Candle> = (0..300)
        .map(|i| {
            let close = 100.0 + (i as f64 * 0.37).sin();
            Candle {
                timestamp: today - (299 - i) * 86_400,
                open: close,
                high: close,
                low: close,
                close,
                ..Default::default()
            }
        })
        .collect();
    *h.mock.history.lock() = Some(Ok(bars));
    let one = json!({"source": "api", "instruments": [{"symbol": "RELIANCE", "exchange": "NSE"}]});
    let (s, v) = post(&h, body(&h, one)).await;
    assert_eq!(s, 200, "{}", v);
    let data = &v["data"];
    assert_eq!(data["benchmark"], "NIFTY");
    assert_eq!(data["instruments"][0]["symbol"], "RELIANCE");
    assert_eq!(data["instruments"][0]["label"], "RELIANCE");
    // The web's response shape, key for key.
    let keys = |v: &Value| {
        let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
        k.sort();
        k
    };
    assert_eq!(
        keys(data),
        [
            "as_of",
            "benchmark",
            "benchmark_exchange",
            "benchmark_label",
            "instruments",
            "max_tail",
            "source",
            "timeframe",
            "unavailable"
        ]
    );
    assert_eq!(
        keys(&data["instruments"][0]),
        [
            "bars_in_quadrant",
            "exchange",
            "label",
            "quadrant",
            "rs_momentum",
            "rs_ratio",
            "symbol",
            "tail"
        ]
    );
    assert_eq!(
        keys(&data["instruments"][0]["tail"][0]),
        ["date", "rs_momentum", "rs_ratio"]
    );
    // The same closes for both: the symbol sits at the centre.
    assert_eq!(data["instruments"][0]["rs_ratio"], json!(100.0));
    assert_eq!(history_reads(&h), 2);
    // Historify holds nothing here: the benchmark's absence names the fix.
    let (s, v) = post(&h, body(&h, json!({}))).await;
    assert_eq!(s, 422);
    assert!(
        v["message"].as_str().unwrap().contains("Historify"),
        "{}",
        v
    );
    h.shutdown().await;
}

/// test_options_lists_sectors_benchmarks_and_exchanges: no key needed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn options_list_sectors_benchmarks_and_exchanges() {
    let h = H::new().await;
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/sectorrotation/options")
        .body(Body::empty())
        .unwrap();
    let (s, _, bytes) = h.send(req).await;
    assert_eq!(s.as_u16(), 200);
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    let data = &v["data"];
    let has = |list: &str, sym: &str| {
        data[list]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["symbol"] == sym)
    };
    assert!(has("benchmarks", "NIFTY") && has("benchmarks", "SENSEX"));
    assert!(has("sectors", "NIFTYIT"));
    assert!(!data["exchanges"]
        .as_array()
        .unwrap()
        .contains(&json!("NFO")));
    assert_eq!(data["max_instruments"], 40);
    h.shutdown().await;
}
