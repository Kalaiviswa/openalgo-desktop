//! Web `test/test_strike_profile_service.py`, case for case: the option
//! chain is replaced with synthetic chains, so the arithmetic (units,
//! signs, walls, the gamma flip and the strike window) is checked without a
//! broker. Expected figures are worked by hand in each test's comment.

use super::*;
use crate::analytics::black76;
use chrono::TimeZone;
use std::cell::RefCell;
use std::collections::HashSet;

const SPOT: f64 = 20_000.0;

fn now() -> DateTime<Utc> {
    Kolkata
        .with_ymd_and_hms(2026, 10, 5, 10, 0, 0)
        .single()
        .unwrap()
        .with_timezone(&Utc)
}

/// Web `_leg`: a leg with open interest and, when priced, its Greeks.
fn leg(oi: i64, gamma: f64, delta: f64, priced: bool) -> Value {
    let mut l = json!({"symbol": "X", "oi": oi, "ltp": 10, "lotsize": 75});
    if priced {
        l["gamma"] = json!(gamma);
        l["delta"] = json!(delta);
        l["implied_volatility"] = json!(12.0);
    }
    l
}

fn priced(oi: i64, gamma: f64, delta: f64) -> Value {
    leg(oi, gamma, delta, true)
}

fn row(strike: f64, ce: Value, pe: Value) -> Value {
    json!({"strike": strike, "ce": ce, "pe": pe})
}

/// Web `_chain`: an answer `days` before expiry.
fn chain(rows: Vec<Value>, days: f64) -> Result<ChainAnswer, Reply> {
    let ts = now().timestamp();
    Ok(ChainAnswer {
        body: json!({
            "status": "success",
            "underlying_ltp": SPOT,
            "forward_price": SPOT,
            "expiry_ts": ts + (days * 86_400.0) as i64,
            "server_ts": ts,
            "chain": rows,
        }),
        quote_missing: HashSet::new(),
        oi_missing: HashSet::new(),
    })
}

type Answer = Box<dyn Fn(i64) -> Result<ChainAnswer, Reply>>;

/// Web `FakeChains`: answers per expiry, records each call.
struct Fake {
    answers: HashMap<String, Answer>,
    calls: RefCell<Vec<(String, i64)>>,
}

impl Fake {
    fn new(answers: Vec<(&str, Answer)>) -> Self {
        Fake {
            answers: answers
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
            calls: RefCell::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<(String, i64)> {
        self.calls.borrow().clone()
    }
}

fn fixed(r: Result<ChainAnswer, Reply>) -> Answer {
    Box::new(move |_| r.clone())
}

async fn run(fake: &Fake, expiries: &[&str], range_pct: Option<f64>) -> Result<Value, Reply> {
    let req = ProfileRequest {
        underlying: "NIFTY".into(),
        exchange: "NFO".into(),
        expiries: expiries.iter().map(|e| e.to_string()).collect(),
        range_pct,
    };
    let fetch = |expiry: String, strike_count: i64| {
        fake.calls.borrow_mut().push((expiry.clone(), strike_count));
        std::future::ready((fake.answers[&expiry])(strike_count))
    };
    profile(fetch, &req, now()).await
}

fn keys(v: &Value) -> Vec<&str> {
    v.as_object().unwrap().keys().map(String::as_str).collect()
}

fn close(v: &Value, want: f64) -> bool {
    v.as_f64().is_some_and(|x| (x - want).abs() < 1e-9)
}

/// Web test_units_and_signs: gamma x OI x spot^2 x 1% / 1e7 =
/// 0.001 x 1e6 x 4e8 x 0.01 / 1e7 = 400 crore; delta x OI x spot / 1e7 =
/// 0.5 x 1e6 x 2e4 / 1e7 = 1000 crore; puts count negative for GEX and
/// put delta is already negative. Plus the answer's shape and number types.
#[tokio::test]
async fn units_and_signs() {
    let rows = vec![row(
        20_000.0,
        priced(1_000_000, 0.001, 0.5),
        priced(500_000, 0.001, -0.5),
    )];
    let fake = Fake::new(vec![("20OCT26", fixed(chain(rows, 3.0)))]);
    let body = run(&fake, &["20OCT26"], None).await.unwrap();
    let cell = &body["expiries"][0]["cells"][0];
    assert!(close(&cell["ce_gex"], 400.0), "{cell}");
    assert!(close(&cell["pe_gex"], -200.0), "{cell}");
    assert!(close(&cell["gex"], 200.0), "{cell}");
    assert!(close(&cell["ce_dex"], 1000.0), "{cell}");
    assert!(close(&cell["pe_dex"], -500.0), "{cell}");
    assert!(close(&cell["dex"], 500.0), "{cell}");
    assert!(close(&body["expiries"][0]["net_gex"], 200.0));
    // 5 Oct 10:00 IST plus three days is 8 Oct.
    assert_eq!(body["expiries"][0]["dte"], json!(3));

    assert_eq!(
        keys(&body),
        vec![
            "as_of",
            "call_wall",
            "exchange",
            "expiries",
            "flip",
            "put_wall",
            "range_pct",
            "spot_price",
            "status",
            "strikes",
            "underlying"
        ]
    );
    assert_eq!(
        keys(&body["expiries"][0]),
        vec![
            "available",
            "cells",
            "dte",
            "expiry",
            "expiry_ts",
            "forward",
            "lot_size",
            "net_dex",
            "net_gex",
            "years"
        ]
    );
    assert_eq!(
        keys(cell),
        vec!["ce_dex", "ce_gex", "ce_oi", "dex", "gex", "pe_dex", "pe_gex", "pe_oi", "unpriced"]
    );
    // Python's types: rounded floats print as floats, OI as it came.
    assert_eq!(cell["ce_oi"].to_string(), "1000000");
    assert_eq!(cell["ce_gex"].to_string(), "400.0");
    assert_eq!(body["strikes"].to_string(), "[20000.0]");
    assert_eq!(body["spot_price"].to_string(), "20000.0");
    assert_eq!(body["range_pct"], Value::Null);
    assert_eq!(body["as_of"], json!(now().timestamp()));
    assert_eq!(body["expiries"][0]["lot_size"], json!(75));
    assert_eq!(body["expiries"][0]["forward"].to_string(), "20000.0");
    // 3 days over a 365-day year, six places: 0.008219.
    assert_eq!(body["expiries"][0]["years"], json!(0.008219));
    assert_eq!(
        (body["call_wall"].clone(), body["put_wall"].clone()),
        (json!(20_000.0), json!(20_000.0))
    );
}

/// Web test_unpriced_open_interest_is_flagged: open interest with no
/// Greeks to weight it is unknown, not zero.
#[tokio::test]
async fn unpriced_open_interest_is_flagged() {
    let rows = vec![row(
        20_000.0,
        leg(1000, 0.0, 0.0, false),
        leg(0, 0.0, 0.0, false),
    )];
    let fake = Fake::new(vec![("20OCT26", fixed(chain(rows, 3.0)))]);
    let body = run(&fake, &["20OCT26"], None).await.unwrap();
    let cell = &body["expiries"][0]["cells"][0];
    assert_eq!(cell["unpriced"], json!(true));
    assert!(close(&cell["gex"], 0.0));
}

/// Web test_walls_and_union_of_strikes. Near GEX per unit of OI is
/// 0.001 x 4e8 x 0.01 / 1e7 = 0.0004: call GEX 0.04, 0.2, 3.2 and put GEX
/// 3.6, 0.2, 0.04 on 19900, 20000, 20100; the far 20200 adds 0.008 and
/// 0.0008. Call wall 20100, put wall 19900.
#[tokio::test]
async fn walls_and_union_of_strikes() {
    let near = vec![
        row(19_900.0, priced(100, 0.001, 0.6), priced(9000, 0.001, -0.4)),
        row(20_000.0, priced(500, 0.001, 0.5), priced(500, 0.001, -0.5)),
        row(20_100.0, priced(8000, 0.001, 0.4), priced(100, 0.001, -0.6)),
    ];
    let far = vec![row(
        20_200.0,
        priced(100, 0.0002, 0.3),
        priced(10, 0.0002, -0.7),
    )];
    let fake = Fake::new(vec![
        ("20OCT26", fixed(chain(near, 3.0))),
        ("27OCT26", fixed(chain(far, 10.0))),
    ]);
    let body = run(&fake, &["20OCT26", "27OCT26"], None).await.unwrap();
    assert_eq!(
        body["strikes"],
        json!([19_900.0, 20_000.0, 20_100.0, 20_200.0])
    );
    assert_eq!(body["call_wall"], json!(20_100.0));
    assert_eq!(body["put_wall"], json!(19_900.0));
    // The far expiry does not list the near strikes: null, not zero.
    let far_cells = body["expiries"][1]["cells"].as_array().unwrap();
    assert_eq!(far_cells[..3], [Value::Null, Value::Null, Value::Null]);
    assert!(close(&far_cells[3]["gex"], 0.0072), "{}", far_cells[3]);
    assert!(close(&body["expiries"][0]["cells"][2]["ce_gex"], 3.2));
}

/// Web test_gamma_flip_sits_between_put_and_call_concentrations: puts
/// below 20000 and calls above, 1e6 OI each at a 12% IV, 3 days out, so
/// total GEX is negative under spot and positive over it. The crossing,
/// worked outside the kernel (a separate Black-76 with `math.erf` over the
/// same 241-point grid from 19500 to 20500), is 19996.8354.
#[tokio::test]
async fn gamma_flip_sits_between_put_and_call_concentrations() {
    let t = 3.0 / 365.0;
    let rows: Vec<Value> = (19_500..20_550)
        .step_by(50)
        .map(|k| {
            let k = f64::from(k);
            let g = black76::gamma(SPOT, k, t, 0.0, 0.12);
            let ce_oi = if k > SPOT { 1_000_000 } else { 0 };
            let pe_oi = if k < SPOT { 1_000_000 } else { 0 };
            row(k, priced(ce_oi, g, 0.5), priced(pe_oi, g, -0.5))
        })
        .collect();
    let fake = Fake::new(vec![("20OCT26", fixed(chain(rows, 3.0)))]);
    let body = run(&fake, &["20OCT26"], None).await.unwrap();
    let flip = body["flip"].as_f64().unwrap();
    assert!(19_900.0 < flip && flip < 20_100.0, "{flip}");
    assert!((flip - 19_996.84).abs() <= 0.01, "{flip}");
}

/// Web test_no_flip_without_a_sign_change: calls only, so no flip and no
/// put wall.
#[tokio::test]
async fn no_flip_without_a_sign_change() {
    let rows: Vec<Value> = [19_900.0, 20_000.0, 20_100.0]
        .iter()
        .map(|k| row(*k, priced(1000, 0.001, 0.5), priced(0, 0.0, 0.0)))
        .collect();
    let fake = Fake::new(vec![("20OCT26", fixed(chain(rows, 3.0)))]);
    let body = run(&fake, &["20OCT26"], None).await.unwrap();
    assert_eq!(body["flip"], Value::Null);
    assert_eq!(body["put_wall"], Value::Null);
}

fn one_strike() -> Result<ChainAnswer, Reply> {
    chain(
        vec![row(
            20_000.0,
            priced(10, 0.001, 0.5),
            priced(10, 0.001, -0.5),
        )],
        3.0,
    )
}

/// Web test_later_expiry_failure_is_marked_not_fatal.
#[tokio::test]
async fn later_expiry_failure_is_marked_not_fatal() {
    let fake = Fake::new(vec![
        ("20OCT26", fixed(one_strike())),
        ("27OCT26", fixed(Err(Reply::error(404, "No strikes")))),
    ]);
    let body = run(&fake, &["20OCT26", "27OCT26"], None).await.unwrap();
    let e = &body["expiries"][1];
    assert_eq!(e["available"], json!(false));
    assert_eq!(e["cells"], json!([null]));
    assert_eq!(e["message"], "No option data for this expiry.");
    assert_eq!(
        keys(e),
        vec![
            "available",
            "cells",
            "dte",
            "expiry",
            "expiry_ts",
            "message"
        ]
    );
}

/// Web test_first_expiry_failure_and_busy_refusal_are_returned: a busy
/// refusal on any expiry is the answer.
#[tokio::test]
async fn first_expiry_failure_and_busy_refusal_are_returned() {
    let busy = || Err(Reply::error(429, "busy"));
    let fake = Fake::new(vec![("20OCT26", fixed(busy()))]);
    let r = run(&fake, &["20OCT26"], None).await.unwrap_err();
    assert_eq!((r.status, r.message()), (429, "busy".to_string()));

    let fake = Fake::new(vec![
        ("20OCT26", fixed(one_strike())),
        ("27OCT26", fixed(busy())),
    ]);
    let r = run(&fake, &["20OCT26", "27OCT26"], None).await.unwrap_err();
    assert_eq!(r.status, 429);

    // Any other refusal of the first expiry is the answer too.
    let fake = Fake::new(vec![(
        "20OCT26",
        fixed(Err(Reply::error(404, "No strikes"))),
    )]);
    assert_eq!(
        run(&fake, &["20OCT26"], None).await.unwrap_err().status,
        404
    );
}

fn widening(strike_count: i64) -> Result<ChainAnswer, Reply> {
    let rows = (-strike_count..=strike_count)
        .map(|i| {
            row(
                SPOT + 50.0 * i as f64,
                priced(10, 0.001, 0.5),
                priced(10, 0.001, -0.5),
            )
        })
        .collect();
    chain(rows, 3.0)
}

/// Web test_window_widens_the_strike_count_from_the_probe: 10% of 20000
/// is 2000 points, 40 strikes of 50 plus one, capped at 40.
#[tokio::test]
async fn window_widens_the_strike_count_from_the_probe() {
    let fake = Fake::new(vec![("20OCT26", Box::new(widening) as Answer)]);
    let body = run(&fake, &["20OCT26"], Some(10.0)).await.unwrap();
    assert_eq!(
        fake.calls(),
        vec![
            ("20OCT26".to_string(), sp::DEFAULT_STRIKE_COUNT),
            ("20OCT26".to_string(), 40)
        ]
    );
    assert_eq!(body["strikes"].as_array().unwrap().len(), 81);
    assert_eq!(body["range_pct"].to_string(), "10.0");
}

/// A narrow window is not widened: the first chain is used as it came and
/// every later expiry asks for the narrower count (2% of 20000 over a 50
/// step: 8 strikes, plus one).
#[tokio::test]
async fn narrow_window_is_kept_for_later_expiries() {
    let fake = Fake::new(vec![
        ("20OCT26", Box::new(widening) as Answer),
        ("27OCT26", Box::new(widening) as Answer),
    ]);
    run(&fake, &["20OCT26", "27OCT26"], Some(2.0))
        .await
        .unwrap();
    assert_eq!(
        fake.calls(),
        vec![("20OCT26".to_string(), 20), ("27OCT26".to_string(), 9)]
    );
}

/// Web test_empty_expiry_list_is_refused.
#[tokio::test]
async fn empty_expiry_list_is_refused() {
    let fake = Fake::new(vec![]);
    let r = run(&fake, &[], None).await.unwrap_err();
    assert_eq!(r.status, 400);
    assert!(fake.calls().is_empty());
}

/// The first chain without a price for the underlying has no spot to
/// value anything against.
#[tokio::test]
async fn no_underlying_price_is_refused() {
    let mut answer = one_strike().unwrap();
    answer.body["underlying_ltp"] = json!(0);
    let fake = Fake::new(vec![("20OCT26", fixed(Ok(answer)))]);
    let r = run(&fake, &["20OCT26"], None).await.unwrap_err();
    assert_eq!(
        (r.status, r.message()),
        (404, "The underlying price is not available.".to_string())
    );
}

/// Web parity (maintainer 2026-10-20: Strike Profile matches the web
/// exactly): a put whose quote did not come back reaches the service as the
/// chain fills it, zero open interest and no Greeks, and is totalled as
/// zero, on the first expiry and on a later one alike; neither is refused
/// nor marked unavailable. The call is web test_units_and_signs's: 0.001 x
/// 1e6 x 20000^2 x 1% / 1e7 = 400 crore of GEX and 0.5 x 1e6 x 20000 / 1e7
/// = 1000 crore of DEX, and with the put at zero those are the strike's
/// totals.
#[tokio::test]
async fn an_open_interest_gap_is_totalled_as_zero() {
    let gap = || {
        let mut a = chain(
            vec![row(
                20_000.0,
                priced(1_000_000, 0.001, 0.5),
                leg(0, 0.0, 0.0, false),
            )],
            3.0,
        )
        .unwrap();
        a.quote_missing.insert("NIFTY20OCT2620000PE".into());
        a.oi_missing.insert("NIFTY20OCT2620000PE".into());
        Ok(a)
    };
    let fake = Fake::new(vec![("20OCT26", fixed(gap())), ("27OCT26", fixed(gap()))]);
    let body = run(&fake, &["20OCT26", "27OCT26"], None).await.unwrap();
    for e in body["expiries"].as_array().unwrap() {
        assert_eq!(e["available"], json!(true), "{e}");
        let cell = &e["cells"][0];
        assert!(close(&cell["ce_gex"], 400.0), "{cell}");
        assert!(close(&cell["pe_gex"], 0.0), "{cell}");
        assert!(close(&cell["gex"], 400.0), "{cell}");
        assert!(close(&cell["ce_dex"], 1000.0), "{cell}");
        assert!(close(&cell["pe_dex"], 0.0), "{cell}");
        assert!(close(&cell["dex"], 1000.0), "{cell}");
        assert_eq!(cell["pe_oi"], json!(0));
        // No open interest, so nothing unpriced to flag.
        assert_eq!(cell["unpriced"], json!(false));
        assert!(close(&e["net_gex"], 400.0), "{e}");
    }
    // Every call GEX is positive and no put has any: a call wall, no put
    // wall.
    assert_eq!(body["call_wall"], json!(20_000.0));
    assert_eq!(body["put_wall"], Value::Null);
}
