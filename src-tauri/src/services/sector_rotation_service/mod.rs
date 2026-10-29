//! Sector Rotation (web `services/sector_rotation_service.py`, openalgo
//! b7cdc028e, 3e37288e0, 91e35d9fb): every requested symbol's place on a
//! four-quadrant relative rotation map against a benchmark.
//!
//! Prices come from the history service, the same reads as
//! `/api/v1/history`: Historify with `source = "db"`, the broker with
//! `source = "api"` (the web reads them through `portfolio.data`). Each
//! symbol is read on its own, so one index a broker does not publish costs
//! that dot and not the map, and a newer index does not shorten every other
//! symbol's history to its own. The benchmark is read once and each symbol
//! is aligned with it alone.

mod calc;
#[cfg(test)]
mod tests;

pub use calc::{Series, Timeframe};

use crate::services::core::Reply;
use crate::state::AppState;
use async_trait::async_trait;
use chrono::{Days, NaiveDate};
use parking_lot::Mutex;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// A symbol on the map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instrument {
    pub symbol: String,
    pub exchange: String,
    pub label: String,
}

/// NSE sector indices brokers commonly publish: the default map. Financial
/// Services is the Nifty Financial Services index, which OpenAlgo names
/// FINNIFTY.
pub const SECTORS: &[(&str, &str, &str)] = &[
    ("NIFTYAUTO", "NSE_INDEX", "Auto"),
    ("NIFTYCOMMODITIES", "NSE_INDEX", "Commodities"),
    ("NIFTYENERGY", "NSE_INDEX", "Energy"),
    ("FINNIFTY", "NSE_INDEX", "Financial Services"),
    ("NIFTYFMCG", "NSE_INDEX", "FMCG"),
    ("NIFTYINFRA", "NSE_INDEX", "Infra"),
    ("NIFTYIT", "NSE_INDEX", "IT"),
    ("NIFTYMEDIA", "NSE_INDEX", "Media"),
    ("NIFTYMETAL", "NSE_INDEX", "Metal"),
    ("NIFTYPHARMA", "NSE_INDEX", "Pharma"),
    ("NIFTYPVTBANK", "NSE_INDEX", "Private Bank"),
    ("NIFTYPSE", "NSE_INDEX", "PSE"),
    ("NIFTYPSUBANK", "NSE_INDEX", "PSU Bank"),
    ("NIFTYREALTY", "NSE_INDEX", "Realty"),
];

/// Sector indices many brokers do not publish, left off the default map
/// (web 91e35d9fb); one added from search keeps its sector name.
pub const OPTIONAL_SECTORS: &[(&str, &str, &str)] = &[
    ("NIFTYCONSRDURBL", "NSE_INDEX", "Consumer Durables"),
    ("NIFTYHEALTHCARE", "NSE_INDEX", "Healthcare"),
    ("NIFTYINDDEFENCE", "NSE_INDEX", "India Defence"),
    ("NIFTYOILANDGAS", "NSE_INDEX", "Oil & Gas"),
];

pub const BENCHMARKS: &[(&str, &str, &str)] = &[
    ("NIFTY", "NSE_INDEX", "Nifty 50"),
    ("NIFTY500", "NSE_INDEX", "Nifty 500"),
    ("BANKNIFTY", "NSE_INDEX", "Bank Nifty"),
    ("SENSEX", "BSE_INDEX", "Sensex"),
    ("NIFTYNXT50", "NSE_INDEX", "Nifty Next 50"),
    ("MIDCPNIFTY", "NSE_INDEX", "Midcap Nifty"),
    ("FINNIFTY", "NSE_INDEX", "Fin Nifty"),
];

/// The benchmark symbols, for the request schema.
pub const BENCHMARK_SYMBOLS: &[&str] = &[
    "NIFTY",
    "NIFTY500",
    "BANKNIFTY",
    "SENSEX",
    "NIFTYNXT50",
    "MIDCPNIFTY",
    "FINNIFTY",
];

pub const TIMEFRAMES: &[&str] = &["daily", "weekly"];
pub const SOURCES: &[&str] = &["db", "api"];

/// Cash and index exchanges. A derivative expires and rolls, so its closes
/// are not one series a relative-strength trend can be read from.
pub const EXCHANGES: &[&str] = &[
    "NSE",
    "BSE",
    "NSE_INDEX",
    "BSE_INDEX",
    "MCX_INDEX",
    "GLOBAL_INDEX",
];

/// Each symbol is one history read on the broker source.
pub const MAX_INSTRUMENTS: usize = 40;

/// The longest trail; the page trims to its slider.
pub const MAX_TAIL: usize = 30;

/// How long one symbol's closes are reused (web `_CLOSES_CACHE`).
const CACHE_TTL: Duration = Duration::from_secs(300);
/// Closes kept at most.
const CACHE_MAX: usize = 256;

fn instrument(&(symbol, exchange, label): &(&str, &str, &str)) -> Instrument {
    Instrument {
        symbol: symbol.into(),
        exchange: exchange.into(),
        label: label.into(),
    }
}

fn listed(list: &[(&str, &str, &str)]) -> Vec<Value> {
    list.iter()
        .map(|(s, e, l)| json!({"symbol": s, "exchange": e, "label": l}))
        .collect()
}

/// `GET /api/v1/sectorrotation/options` (web `list_options`).
pub fn options() -> Reply {
    Reply::ok(json!({
        "status": "success",
        "data": {
            "benchmarks": listed(BENCHMARKS),
            "timeframes": TIMEFRAMES,
            "exchanges": EXCHANGES,
            "max_instruments": MAX_INSTRUMENTS,
            "sectors": listed(SECTORS),
            "optional_sectors": listed(OPTIONAL_SECTORS),
            "max_tail": MAX_TAIL,
        },
    }))
}

/// The requested symbols, upper-cased and de-duplicated (first kept), or
/// what is wrong with them; none means the default sector set. A symbol
/// without a label takes its sector name when it is a known sector, else
/// its own symbol (web `resolve_instruments`).
pub fn resolve_instruments(requested: &[Value]) -> Result<Vec<Instrument>, String> {
    if requested.is_empty() {
        return Ok(SECTORS.iter().map(instrument).collect());
    }
    if requested.len() > MAX_INSTRUMENTS {
        return Err(format!(
            "The map holds at most {} symbols.",
            MAX_INSTRUMENTS
        ));
    }
    let text = |v: &Value, k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    let mut out: Vec<Instrument> = Vec::new();
    for item in requested {
        let symbol = text(item, "symbol").to_uppercase();
        let exchange = text(item, "exchange").to_uppercase();
        if symbol.is_empty() {
            return Err("Every symbol on the map needs a name.".into());
        }
        if !EXCHANGES.contains(&exchange.as_str()) {
            return Err(format!(
                "{} is on {}. The map takes symbols from {}.",
                symbol,
                if exchange.is_empty() {
                    "no exchange"
                } else {
                    &exchange
                },
                EXCHANGES.join(", ")
            ));
        }
        if out
            .iter()
            .any(|i| i.symbol == symbol && i.exchange == exchange)
        {
            continue;
        }
        let known = SECTORS
            .iter()
            .chain(OPTIONAL_SECTORS)
            .find(|(s, e, _)| *s == symbol && *e == exchange)
            .map(|(_, _, l)| l.to_string());
        let label = Some(text(item, "label"))
            .filter(|l| !l.is_empty())
            .or(known)
            .unwrap_or_else(|| symbol.clone());
        out.push(Instrument {
            symbol,
            exchange,
            label,
        });
    }
    Ok(out)
}

/// Where a map's daily closes come from.
#[async_trait]
pub trait CloseLoader: Send + Sync {
    /// Daily closes of one symbol for `[start, end]`, oldest first, or why
    /// there are none (logged; the trader reads the source's reason).
    async fn daily_closes(
        &self,
        ins: &Instrument,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<Series, String>;
    /// The source and broker the closes come from, part of the cache key.
    fn scope(&self) -> String;
}

type CacheKey = (String, String, String, NaiveDate, NaiveDate);

/// One symbol's closes reused across maps for a few minutes: a default map
/// is fifteen history reads, and adding one stock to it costs one more,
/// not sixteen. Bounded and expiring.
#[derive(Default)]
pub struct ClosesCache {
    entries: Mutex<HashMap<CacheKey, (Instant, Arc<Series>)>>,
}

impl ClosesCache {
    fn get(&self, key: &CacheKey) -> Option<Arc<Series>> {
        let map = self.entries.lock();
        map.get(key)
            .filter(|(at, _)| at.elapsed() < CACHE_TTL)
            .map(|(_, s)| s.clone())
    }

    fn put(&self, key: CacheKey, closes: Arc<Series>) {
        let mut map = self.entries.lock();
        map.retain(|_, (at, _)| at.elapsed() < CACHE_TTL);
        if map.len() >= CACHE_MAX {
            if let Some(oldest) = map
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(k, _)| k.clone())
            {
                map.remove(&oldest);
            }
        }
        map.insert(key, (Instant::now(), closes));
    }
}

async fn closes(
    loader: &dyn CloseLoader,
    cache: &ClosesCache,
    ins: &Instrument,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<Arc<Series>, String> {
    let key = (
        ins.symbol.clone(),
        ins.exchange.clone(),
        loader.scope(),
        start,
        end,
    );
    if let Some(hit) = cache.get(&key) {
        return Ok(hit);
    }
    let series = Arc::new(loader.daily_closes(ins, start, end).await?);
    cache.put(key, series.clone());
    Ok(series)
}

/// What a map is asked for.
pub struct Query {
    pub benchmark: String,
    pub timeframe: String,
    pub source: String,
    pub instruments: Vec<Value>,
    /// The last session to include (today, IST).
    pub end: NaiveDate,
}

fn missing_reason(source: &str) -> String {
    if source == "db" {
        "Historify has no daily history for this symbol. Download it in Historify, or switch the data source to Broker API.".into()
    } else {
        "Your broker returned no history for this symbol. Some brokers do not publish every sector index; Historify may have it.".into()
    }
}

fn unavailable(ins: &Instrument, reason: String) -> Value {
    json!({"symbol": ins.symbol, "exchange": ins.exchange, "label": ins.label, "reason": reason})
}

/// One symbol's map entry, or its `unavailable` row.
async fn place(
    q: &Query,
    tf: Timeframe,
    bench: &Instrument,
    bench_closes: &Series,
    member: &Instrument,
    load: (&dyn CloseLoader, &ClosesCache, NaiveDate),
) -> Result<Value, Value> {
    if member.symbol == bench.symbol && member.exchange == bench.exchange {
        return Err(unavailable(
            member,
            "This is the benchmark itself, so it would always sit at 100. Pick another benchmark to see it on the map.".into(),
        ));
    }
    let (loader, cache, start) = load;
    let member_closes = match closes(loader, cache, member, start, q.end).await {
        Ok(c) => c,
        Err(e) => {
            tracing::info!("Sector rotation: {} skipped: {}", member.symbol, e);
            return Err(unavailable(member, missing_reason(&q.source)));
        }
    };
    let (sym, ben) = calc::paired_bars(&member_closes, bench_closes, tf);
    let Some(Value::Object(summary)) =
        calc::summarise(&calc::rotation_series(&sym, &ben, tf), MAX_TAIL)
    else {
        return Err(unavailable(
            member,
            format!(
                "Not enough {} history yet. The map needs at least {} {} that this symbol and the benchmark both traded.",
                tf.as_str(),
                tf.params().min_bars(),
                tf.unit()
            ),
        ));
    };
    let mut entry = Map::new();
    entry.insert("symbol".into(), json!(member.symbol));
    entry.insert("exchange".into(), json!(member.exchange));
    entry.insert("label".into(), json!(member.label));
    entry.extend(summary);
    Ok(Value::Object(entry))
}

/// Every requested symbol's place on the map against the benchmark (web
/// `compute_sector_rotation`). Symbols that cannot be placed are listed
/// under `unavailable` with a reason, never failing the map.
pub async fn compute(q: &Query, loader: &dyn CloseLoader, cache: &ClosesCache) -> Reply {
    let Some(bench) = BENCHMARKS
        .iter()
        .find(|(s, _, _)| s.eq_ignore_ascii_case(&q.benchmark))
        .map(instrument)
    else {
        return Reply::error(
            400,
            format!("benchmark must be one of {}", BENCHMARK_SYMBOLS.join(", ")),
        );
    };
    let Some(tf) = Timeframe::parse(&q.timeframe) else {
        return Reply::error(
            400,
            format!("timeframe must be one of {}", TIMEFRAMES.join(", ")),
        );
    };
    if !SOURCES.contains(&q.source.as_str()) {
        return Reply::error(400, "source must be 'db' (Historify) or 'api' (broker)");
    }
    let members = match resolve_instruments(&q.instruments) {
        Ok(m) => m,
        Err(problem) => return Reply::error(400, problem),
    };
    let start = q.end - Days::new(tf.lookback_days());
    let bench_closes = match closes(loader, cache, &bench, start, q.end).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                "Sector rotation: benchmark {} history unavailable: {}",
                bench.symbol,
                e
            );
            let message = if q.source == "db" {
                format!("Historify has no daily history for {}. Download it in Historify, or switch the data source to Broker API.", bench.label)
            } else {
                format!("Your broker returned no history for {}. Pick another benchmark, or switch the data source to Historify.", bench.label)
            };
            return Reply::error(422, message);
        }
    };
    let (mut placed, mut missing) = (Vec::new(), Vec::new());
    let mut as_of: Option<String> = None;
    for member in &members {
        match place(q, tf, &bench, &bench_closes, member, (loader, cache, start)).await {
            Ok(entry) => {
                let last = entry["tail"]
                    .as_array()
                    .and_then(|t| t.last())
                    .and_then(|p| p["date"].as_str())
                    .unwrap_or_default()
                    .to_string();
                if as_of.as_ref().is_none_or(|a| last > *a) {
                    as_of = Some(last);
                }
                placed.push(entry);
            }
            Err(row) => missing.push(row),
        }
    }
    if placed.is_empty() && !missing.is_empty() {
        let reason = missing[0]["reason"].as_str().unwrap_or_default();
        return Reply::error(
            422,
            format!("Nothing could be placed on the map. {}", reason),
        );
    }
    Reply::ok(json!({
        "status": "success",
        "data": {
            "benchmark": bench.symbol,
            "benchmark_exchange": bench.exchange,
            "benchmark_label": bench.label,
            "timeframe": tf.as_str(),
            "source": q.source,
            "as_of": as_of,
            "max_tail": MAX_TAIL,
            "instruments": placed,
            "unavailable": missing,
        },
    }))
}

/// Closes from the history service: Historify or the broker, as
/// `/api/v1/history` reads them (the Dhan bar after the close included).
struct HistoryLoader<'a> {
    ctx: &'a AppState,
    source: String,
}

#[async_trait]
impl CloseLoader for HistoryLoader<'_> {
    async fn daily_closes(
        &self,
        ins: &Instrument,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<Series, String> {
        let r = super::market_data_service::history(
            self.ctx,
            &ins.symbol,
            &ins.exchange,
            "D",
            start,
            end,
            &self.source,
        )
        .await;
        if r.status != 200 {
            return Err(format!("history answered {}: {}", r.status, r.message()));
        }
        daily_series(&r.body["data"])
    }

    fn scope(&self) -> String {
        let broker = self
            .ctx
            .get_broker_session()
            .map(|s| s.broker_id)
            .unwrap_or_default();
        format!("{}:{}", self.source, broker)
    }
}

/// History candles as one positive close per IST session, oldest first; a
/// re-read session keeps its last close (web `portfolio.data`).
pub fn daily_series(data: &Value) -> Result<Series, String> {
    let mut out: Series = data
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|c| {
            let date = crate::analytics::series::ist_date(c["timestamp"].as_i64()?)?;
            let close = c["close"].as_f64()?;
            Some((date, close))
        })
        .collect();
    if out.iter().any(|(_, c)| !c.is_finite() || *c <= 0.0) {
        return Err("a close is not a positive number".into());
    }
    out.sort_by_key(|(d, _)| *d);
    out.reverse();
    out.dedup_by_key(|(d, _)| *d);
    out.reverse();
    if out.len() < 2 {
        return Err(format!("{} session(s) of daily history", out.len()));
    }
    Ok(out)
}

fn shared_cache() -> &'static ClosesCache {
    static CACHE: OnceLock<ClosesCache> = OnceLock::new();
    CACHE.get_or_init(ClosesCache::default)
}

/// `POST /api/v1/sectorrotation` for a request the route has validated.
pub async fn sector_rotation(ctx: &AppState, req: &Map<String, Value>) -> Reply {
    let text = |k: &str| {
        req.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let q = Query {
        benchmark: text("benchmark"),
        timeframe: text("timeframe"),
        source: text("source"),
        instruments: req
            .get("instruments")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        end: super::market_calendar_service::today_ist(ctx.now()),
    };
    let loader = HistoryLoader {
        ctx,
        source: q.source.clone(),
    };
    compute(&q, &loader, shared_cache()).await
}
