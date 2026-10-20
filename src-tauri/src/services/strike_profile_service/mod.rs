//! Strike Profile (web `strike_profile_service.py`): gamma exposure (GEX)
//! and delta exposure (DEX) for every strike of several expiries at once,
//! as a strike by expiry grid, in rupees crore.
//!
//! Each expiry is one option chain with Greeks: the chain's own Black-76
//! pass against its put-call-parity forward, so the only broker calls are
//! one chain per expiry, plus one refetch of the first when the strike
//! window needs more strikes than the first chain loaded. The arithmetic
//! (units, signs, the gamma flip, the walls, the window) is in
//! [`crate::analytics::strike_profile`].
//!
//! The answer matches the web exactly (maintainer 2026-10-20: Strike
//! Profile matches the web exactly). A leg whose quote or open interest did
//! not come back is totalled as zero open interest, as on the web; unlike
//! the other chain tools, which refuse such a chain (TOOLS-13), it is
//! neither refused nor marked unavailable.

use super::core::{float, Reply};
use super::options_service::{option_chain_answer, ChainAnswer};
use crate::analytics::py_round;
use crate::analytics::strike_profile::{self as sp, FlipLeg};
use crate::state::AppState;
use chrono::{DateTime, TimeZone, Utc};
use chrono_tz::Asia::Kolkata;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::future::Future;

/// Expiries one request may ask for: each is one option chain fetch.
pub const MAX_EXPIRIES: usize = 20;

/// Strike windows the page offers, percent either side of spot.
pub const ALLOWED_RANGES: [f64; 3] = [2.0, 5.0, 10.0];

/// A chain refused because the broker queue was full (web
/// `BROKER_BUSY_STATUS`): returned as is, whichever expiry it was.
const BROKER_BUSY: u16 = 429;

const SECONDS_PER_YEAR: f64 = 365.0 * 86_400.0;

const NO_DATA: &str = "No option data for this expiry.";

/// What the page asked for, validated.
#[derive(Debug, Clone)]
pub struct ProfileRequest {
    pub underlying: String,
    pub exchange: String,
    /// `DDMMMYY`, nearest first, at most [`MAX_EXPIRIES`].
    pub expiries: Vec<String>,
    /// Percent either side of spot; `None` loads the widest window.
    pub range_pct: Option<f64>,
}

/// `POST /strikeprofile/api/profile-data`.
pub async fn strike_profile(ctx: &AppState, req: &ProfileRequest) -> Reply {
    let fetch = |expiry: String, strike_count: i64| async move {
        option_chain_answer(
            ctx,
            &req.underlying,
            &req.exchange,
            &expiry,
            Some(strike_count),
            true,
            None,
        )
        .await
    };
    match profile(fetch, req, ctx.now()).await {
        Ok(body) => Reply::ok(body),
        Err(r) => r,
    }
}

/// One strike of one expiry.
struct Cell {
    json: Value,
    /// Rounded as in `json`: the walls add up the rounded figures.
    ce_gex: f64,
    pe_gex: f64,
}

/// One expiry's column: its heading fields and its cells by strike.
struct Column {
    meta: Map<String, Value>,
    cells: HashMap<u64, Cell>,
}

impl Column {
    fn unavailable(expiry: &str, message: &str) -> Self {
        let meta = json!({
            "expiry": expiry,
            "expiry_ts": null,
            "dte": null,
            "available": false,
            "message": message,
        });
        Column {
            meta: meta.as_object().cloned().unwrap_or_default(),
            cells: HashMap::new(),
        }
    }

    fn cell(&self, strike: f64) -> Option<&Cell> {
        self.cells.get(&strike.to_bits())
    }
}

/// The price and time every expiry is read against: the first chain's.
struct Basis {
    spot: f64,
    server_ts: Value,
}

/// The grid built so far.
#[derive(Default)]
struct Grid {
    columns: Vec<Column>,
    strikes: Vec<f64>,
    flip_legs: Vec<FlipLeg>,
}

/// The whole profile, with the chain fetch passed in (`fetch(expiry,
/// strikes each side)`) so the arithmetic is tested on synthetic chains.
async fn profile<F, Fut>(
    mut fetch: F,
    req: &ProfileRequest,
    now: DateTime<Utc>,
) -> Result<Value, Reply>
where
    F: FnMut(String, i64) -> Fut,
    Fut: Future<Output = Result<ChainAnswer, Reply>>,
{
    if req.expiries.is_empty() {
        return Err(Reply::error(400, "Select at least one expiry."));
    }
    let mut basis: Option<Basis> = None;
    let mut strike_count = sp::DEFAULT_STRIKE_COUNT;
    let mut grid = Grid::default();
    for (index, expiry) in req.expiries.iter().enumerate() {
        let answer = match fetch(expiry.clone(), strike_count).await {
            Ok(a) => a,
            Err(r) if r.status == BROKER_BUSY || index == 0 => return Err(r),
            Err(r) => {
                tracing::warn!(
                    "Strike profile: skipping {} {}: {}",
                    req.underlying,
                    expiry,
                    r.message()
                );
                grid.columns.push(Column::unavailable(expiry, NO_DATA));
                continue;
            }
        };
        let (answer, spot) = match &basis {
            Some(b) => (answer, b.spot),
            None => {
                let (b, a, count) = first_chain(&mut fetch, expiry, answer, req.range_pct).await?;
                strike_count = count;
                let spot = b.spot;
                basis = Some(b);
                (a, spot)
            }
        };
        add_column(&mut grid, expiry, &answer.body, spot, now);
    }
    match basis {
        Some(b) if !grid.strikes.is_empty() => Ok(finish(req, &b, grid, now)),
        _ => Err(Reply::error(404, "No option data for these expiries.")),
    }
}

/// The first chain sets the spot and doubles as a probe for the strike
/// step: when the window needs more strikes than it loaded, it is fetched
/// again with the wider count, which every later expiry then uses.
async fn first_chain<F, Fut>(
    fetch: &mut F,
    expiry: &str,
    answer: ChainAnswer,
    range_pct: Option<f64>,
) -> Result<(Basis, ChainAnswer, i64), Reply>
where
    F: FnMut(String, i64) -> Fut,
    Fut: Future<Output = Result<ChainAnswer, Reply>>,
{
    let Some(spot) = answer
        .body
        .get("underlying_ltp")
        .and_then(Value::as_f64)
        .filter(|s| *s > 0.0)
    else {
        return Err(Reply::error(404, "The underlying price is not available."));
    };
    let basis = Basis {
        spot,
        server_ts: answer.body.get("server_ts").cloned().unwrap_or(Value::Null),
    };
    let strikes: Vec<f64> = rows(&answer.body)
        .iter()
        .filter_map(|r| r.get("strike").and_then(Value::as_f64))
        .collect();
    let wanted = sp::strike_count_for_window(&strikes, spot, range_pct);
    if wanted > sp::DEFAULT_STRIKE_COUNT {
        let refetched = fetch(expiry.to_string(), wanted).await?;
        return Ok((basis, refetched, wanted));
    }
    Ok((basis, answer, wanted))
}

fn rows(body: &Value) -> &[Value] {
    body.get("chain")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

/// What one option leg contributes (web `_leg_exposure`).
struct Leg {
    /// The chain's figure as it came, `0` when missing.
    oi_json: Value,
    oi: f64,
    gex: f64,
    dex: f64,
    /// Percent.
    iv: f64,
    /// The chain carried Greeks for it.
    priced: bool,
}

fn leg_exposure(leg: Option<&Value>, spot: f64, sign: f64) -> Leg {
    let Some(m) = leg.and_then(Value::as_object).filter(|m| !m.is_empty()) else {
        return Leg {
            oi_json: json!(0),
            oi: 0.0,
            gex: 0.0,
            dex: 0.0,
            iv: 0.0,
            priced: false,
        };
    };
    let n = |k: &str| m.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let oi = n("oi");
    Leg {
        oi_json: m
            .get("oi")
            .filter(|_| oi != 0.0)
            .cloned()
            .unwrap_or(json!(0)),
        oi,
        gex: sp::gex(n("gamma"), oi, spot, sign),
        dex: sp::dex(n("delta"), oi, spot),
        iv: n("implied_volatility"),
        priced: m.contains_key("gamma") && m.contains_key("delta"),
    }
}

fn r4(x: f64) -> f64 {
    py_round(x, 4)
}

/// One strike's cell; `None` for a strike that is not a positive number.
fn strike_cell(row: &Value, spot: f64) -> Option<(f64, Leg, Leg, Cell)> {
    let strike = row
        .get("strike")
        .and_then(Value::as_f64)
        .filter(|k| *k > 0.0)?;
    let ce = leg_exposure(row.get("ce"), spot, 1.0);
    let pe = leg_exposure(row.get("pe"), spot, -1.0);
    // Open interest with no Greeks to weight it is unknown, not zero.
    let unpriced = (ce.oi > 0.0 && !ce.priced) || (pe.oi > 0.0 && !pe.priced);
    let cell = Cell {
        json: json!({
            "ce_oi": ce.oi_json,
            "pe_oi": pe.oi_json,
            "ce_gex": float(r4(ce.gex)),
            "pe_gex": float(r4(pe.gex)),
            "ce_dex": float(r4(ce.dex)),
            "pe_dex": float(r4(pe.dex)),
            "gex": float(r4(ce.gex + pe.gex)),
            "dex": float(r4(ce.dex + pe.dex)),
            "unpriced": unpriced,
        }),
        ce_gex: r4(ce.gex),
        pe_gex: r4(pe.gex),
    };
    Some((strike, ce, pe, cell))
}

/// Add one expiry's column to the grid, and its priced legs to the flip
/// search.
fn add_column(grid: &mut Grid, expiry: &str, body: &Value, spot: f64, now: DateTime<Utc>) {
    let expiry_ts = body.get("expiry_ts").and_then(Value::as_i64);
    let years = expiry_years(body, now);
    let forward = body
        .get("forward_price")
        .and_then(Value::as_f64)
        .filter(|f| *f != 0.0)
        .unwrap_or(spot);
    let mut cells = HashMap::new();
    let (mut total_gex, mut total_dex) = (0.0, 0.0);
    for row in rows(body) {
        let Some((strike, ce, pe, cell)) = strike_cell(row, spot) else {
            continue;
        };
        total_gex += ce.gex + pe.gex;
        total_dex += ce.dex + pe.dex;
        grid.strikes.push(strike);
        cells.insert(strike.to_bits(), cell);
        if years > 0.0 {
            for (leg, sign) in [(&ce, 1.0), (&pe, -1.0)] {
                if leg.oi > 0.0 && leg.priced && leg.iv > 0.0 {
                    grid.flip_legs.push(FlipLeg {
                        strike,
                        forward,
                        years: years.max(sp::MIN_YEARS),
                        iv: leg.iv / 100.0,
                        oi: leg.oi,
                        sign,
                    });
                }
            }
        }
    }
    let meta = json!({
        "expiry": expiry,
        "expiry_ts": body.get("expiry_ts").cloned().unwrap_or(Value::Null),
        "dte": calendar_dte(expiry_ts, now),
        "years": float(py_round(years, 6)),
        "forward": float(py_round(forward, 2)),
        "lot_size": lot_size(body),
        "available": true,
        "net_gex": float(r4(total_gex)),
        "net_dex": float(r4(total_dex)),
    });
    grid.columns.push(Column {
        meta: meta.as_object().cloned().unwrap_or_default(),
        cells,
    });
}

/// The first leg's lot size: each row's call, else its put.
fn lot_size(body: &Value) -> Value {
    rows(body)
        .iter()
        .find_map(|row| {
            let leg = ["ce", "pe"].iter().find_map(|s| {
                row.get(*s)
                    .filter(|l| l.as_object().is_some_and(|m| !m.is_empty()))
            })?;
            leg.get("lotsize")
                .filter(|v| v.as_f64().is_some_and(|x| x != 0.0))
                .cloned()
        })
        .unwrap_or(Value::Null)
}

/// Years to expiry from the chain's own expiry and server timestamps.
fn expiry_years(body: &Value, now: DateTime<Utc>) -> f64 {
    let Some(expiry_ts) = body
        .get("expiry_ts")
        .and_then(Value::as_f64)
        .filter(|t| *t != 0.0)
    else {
        return 0.0;
    };
    let server_ts = body
        .get("server_ts")
        .and_then(Value::as_f64)
        .filter(|t| *t != 0.0)
        .unwrap_or(now.timestamp() as f64);
    ((expiry_ts - server_ts) / SECONDS_PER_YEAR).max(0.0)
}

/// Whole calendar days from today (IST) to the expiry date.
fn calendar_dte(expiry_ts: Option<i64>, now: DateTime<Utc>) -> Value {
    let Some(ts) = expiry_ts.filter(|t| *t != 0) else {
        return Value::Null;
    };
    let Some(expiry) = Kolkata.timestamp_opt(ts, 0).single() else {
        return Value::Null;
    };
    let today = now.with_timezone(&Kolkata).date_naive();
    json!((expiry.date_naive() - today).num_days().max(0))
}

/// The grid aligned to the union of strikes, the walls, the flip.
fn finish(req: &ProfileRequest, basis: &Basis, mut grid: Grid, now: DateTime<Utc>) -> Value {
    grid.strikes.sort_by(f64::total_cmp);
    grid.strikes.dedup();
    let strikes = grid.strikes;
    let side_total = |k: f64, side: fn(&Cell) -> f64| -> f64 {
        grid.columns
            .iter()
            .map(|c| c.cell(k).map(side).unwrap_or(0.0))
            .sum()
    };
    let calls: Vec<f64> = strikes
        .iter()
        .map(|k| side_total(*k, |c| c.ce_gex))
        .collect();
    let puts: Vec<f64> = strikes
        .iter()
        .map(|k| side_total(*k, |c| c.pe_gex.abs()))
        .collect();
    let flip = match (strikes.first(), strikes.last()) {
        (Some(low), Some(high)) => sp::find_gamma_flip(
            &grid.flip_legs,
            basis.spot,
            *low,
            *high,
            sp::FLIP_GRID_POINTS,
        ),
        _ => None,
    };
    let opt = |v: Option<f64>| v.map(float).unwrap_or(Value::Null);
    let expiries: Vec<Value> = grid
        .columns
        .iter()
        .map(|c| {
            let cells: Vec<Value> = strikes
                .iter()
                .map(|k| c.cell(*k).map(|x| x.json.clone()).unwrap_or(Value::Null))
                .collect();
            let mut meta = c.meta.clone();
            meta.insert("cells".into(), Value::Array(cells));
            Value::Object(meta)
        })
        .collect();
    let as_of = match &basis.server_ts {
        v if v.as_f64().is_some_and(|t| t != 0.0) => v.clone(),
        _ => json!(now.timestamp()),
    };
    json!({
        "status": "success",
        "underlying": req.underlying,
        "exchange": req.exchange,
        "spot_price": float(py_round(basis.spot, 2)),
        "as_of": as_of,
        "range_pct": opt(req.range_pct),
        "strikes": strikes.iter().map(|k| float(*k)).collect::<Vec<_>>(),
        "flip": opt(flip.map(|f| py_round(f, 2))),
        "call_wall": opt(sp::wall(&strikes, &calls)),
        "put_wall": opt(sp::wall(&strikes, &puts)),
        "expiries": expiries,
    })
}

#[cfg(test)]
mod tests;
