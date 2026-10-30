//! Sahi master contract (web `broker/sahi/database/master_contract_db.py`).
//!
//! One public CSV, rebuilt around 06:00 IST each trading day:
//! `id,isin,symbol,company_name,series,underlying_symbol,exchange,segment,
//! token,instrument_type,option_type,expiry_date,strike_price,lot_size,
//! tick_size,...`
//!
//! * `token` is Sahi's `id`: market data and the tick stream are addressed
//!   by it, never by the exchange token.
//! * `brsymbol` is Sahi's `symbol`, which orders take with the segment.
//!   Index rows carry no symbol (they cannot be traded), so their display
//!   name (`Nifty 50`, `Sensex`) is used instead.
//! * `brexchange` is Sahi's `segment`. Only NSE, BSE, NFO and BFO are
//!   loaded: the file also lists MCX and NCOM, which Sahi does not trade.
//! * Derivative symbols are built from underlying, expiry, strike and
//!   option type rather than parsed from Sahi's symbol, whose shape differs
//!   between NFO and BFO.

use super::USER_AGENT;
use crate::brokers::common::http;
use crate::brokers::common::master_contract::{
    format_expiry, future_symbol, option_symbol, split_csv_line, CsvHeader,
};
use crate::brokers::types::SymbolData;
use crate::error::{AppError, Result};
use chrono::NaiveDate;
use std::collections::HashMap;

const SEGMENTS: &[&str] = &["NSE", "BSE", "NFO", "BFO"];

/// NSE index display names whose OpenAlgo symbol is not simply the name in
/// capitals without spaces and punctuation (`docs/prompt/symbol-format.md`).
const NSE_INDEX_SYMBOLS: &[(&str, &str)] = &[
    ("NIFTY 50", "NIFTY"),
    ("NIFTY BANK", "BANKNIFTY"),
    ("NIFTY FIN SERVICE", "FINNIFTY"),
    ("NIFTY MID SELECT", "MIDCPNIFTY"),
    ("NIFTY NEXT 50", "NIFTYNXT50"),
    ("INDIA VIX", "INDIAVIX"),
    ("NIFTY SMALLCAP 50", "NIFTYSMLCAP50"),
    ("NIFTY SMALLCAP 100", "NIFTYSMLCAP100"),
    ("NIFTY SMALLCAP 250", "NIFTYSMLCAP250"),
    ("NIFTY MIDSMALLCAP 400", "NIFTYMIDSML400"),
    ("NIFTY MIDCAP LIQUID 15", "NIFTYMIDLIQ15"),
    ("NIFTY INFRASTRUCTURE", "NIFTYINFRA"),
    ("NIFTY INDIA CONSUMPTION", "NIFTYCONSUMPTION"),
    ("NIFTY DIVIDEND OPPORTUNITIES 50", "NIFTYDIVOPPS50"),
    ("NIFTY GROWTH SECTORS 15", "NIFTYGROWSECT15"),
    ("NIFTY PRIVATE BANK", "NIFTYPVTBANK"),
    ("NIFTY SERVICES SECTOR", "NIFTYSERVSECTOR"),
    ("NIFTY 10 YR BENCHMARK G-SEC", "NIFTYGS10YR"),
    (
        "NIFTY 10 YR BENCHMARK G-SEC (CLEAN PRICE)",
        "NIFTYGS10YRCLN",
    ),
    ("NIFTY 4-8 YR G-SEC INDEX", "NIFTYGS48YR"),
    ("NIFTY 8-13 YR G-SEC", "NIFTYGS813YR"),
    ("NIFTY 11-15 YR G-SEC INDEX", "NIFTYGS1115YR"),
    ("NIFTY 15 YR AND ABOVE G-SEC INDEX", "NIFTYGS15YRPLUS"),
    ("NIFTY COMPOSITE G-SEC INDEX", "NIFTYGSCOMPSITE"),
    ("NIFTY100 EQUAL WEIGHT", "NIFTY100EQLWGT"),
    ("NIFTY100 LIQUID 15", "NIFTY100LIQ15"),
    ("NIFTY100 LOW VOLATILITY 30", "NIFTY100LOWVOL30"),
    ("NIFTY100 QUALITY 30", "NIFTY100QUALTY30"),
    ("NIFTY200 QUALITY 30", "NIFTY200QUALTY30"),
    ("NIFTY50 DIVIDEND POINTS", "NIFTY50DIVPOINT"),
    ("NIFTY50 EQUAL WEIGHT", "NIFTY50EQLWGT"),
    ("NIFTY50 PR 1X INVERSE", "NIFTY50PR1XINV"),
    ("NIFTY50 PR 2X LEVERAGE", "NIFTY50PR2XLEV"),
    ("NIFTY50 TR 1X INVERSE", "NIFTY50TR1XINV"),
    ("NIFTY50 TR 2X LEVERAGE", "NIFTY50TR2XLEV"),
];

/// BSE index rows carry the exchange's short code as their name.
const BSE_INDEX_SYMBOLS: &[(&str, &str)] = &[
    ("SENSEX", "SENSEX"),
    ("BANKEX", "BANKEX"),
    ("SENSEX 50", "SENSEX50"),
    ("BSE100", "BSE100"),
    ("MID150", "BSE150MIDCAPINDEX"),
    ("BSE200", "BSE200"),
    ("LMI250", "BSE250LARGEMIDCAPINDEX"),
    ("MSL400", "BSE400MIDSMALLCAPINDEX"),
    ("BSE500", "BSE500"),
    ("AUTO", "BSEAUTO"),
    ("BSE CG", "BSECAPITALGOODS"),
    ("CARBON", "BSECARBONEX"),
    ("BSE CD", "BSECONSUMERDURABLES"),
    ("CPSE", "BSECPSE"),
    ("ENERGY", "BSEENERGY"),
    ("BSEFMC", "BSEFASTMOVINGCONSUMERGOODS"),
    ("FINSER", "BSEFINANCIALSERVICES"),
    ("GREENX", "BSEGREENEX"),
    ("BSE HC", "BSEHEALTHCARE"),
    ("INFRA", "BSEINDIAINFRASTRUCTUREINDEX"),
    ("INDSTR", "BSEINDUSTRIALS"),
    ("BSE IT", "BSEINFORMATIONTECHNOLOGY"),
    ("BSEIPO", "BSEIPO"),
    ("LRGCAP", "BSELARGECAP"),
    ("METAL", "BSEMETAL"),
    ("MIDCAP", "BSEMIDCAP"),
    ("MIDSEL", "BSEMIDCAPSELECTINDEX"),
    ("OILGAS", "BSEOIL&GAS"),
    ("POWER", "BSEPOWER"),
    ("BSEPSU", "BSEPSU"),
    ("REALTY", "BSEREALTY"),
    ("SNXT50", "BSESENSEXNEXT50"),
    ("SMLCAP", "BSESMALLCAP"),
    ("SMLSEL", "BSESMALLCAPSELECTINDEX"),
    ("SMEIPO", "BSESMEIPO"),
    ("TECK", "BSETECK"),
    ("TELCOM", "BSETELECOM"),
    ("ALLCAP", "BSEALLCAP"),
    ("BHRT22", "BSEBHARAT22INDEX"),
    ("SML250", "BSE250SMALLCAPINDEX"),
    ("UTILS", "BSEUTILITIES"),
    ("ESG100", "BSE100ESG"),
    ("LCTMCI", "BSE100LARGECAPTMCINDEX"),
    ("DFRGRI", "BSEDIVERSIFIEDFINANCIALSREVENUEGROWTHINDEX"),
    ("BSEDSI", "BSEDIVIDENDSTABILITY"),
    ("BSEEVI", "BSEENHANCEDVALUE"),
    ("BSELVI", "BSELOWVOLATILITY"),
    ("BSEMOI", "BSEMOMENTUM"),
    ("BSEPBI", "BSEPRIVATEBANKS"),
    ("BSEQUI", "BSEQUALITY"),
    ("BSHOSP", "BSEHOSPITALS"),
    ("FOCIT", "BSEFOCUSEDIT"),
    ("CONDIS", "BSECONSUMERDISCRETIONARYGOODS&SERVICES"),
];

pub async fn download(client: &reqwest::Client, url: &str) -> Result<Vec<SymbolData>> {
    let resp = client
        .get(url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .timeout(http::DOWNLOAD_TIMEOUT)
        .send()
        .await?;
    if !resp.status().is_success() {
        tracing::warn!(
            status = resp.status().as_u16(),
            "Sahi scrip master download failed"
        );
        return Err(AppError::Broker(
            "The Sahi instrument list could not be downloaded. Your existing symbols were kept; try the download again later.".into(),
        ));
    }
    let text = resp.text().await?;
    let rows = parse(&text)?;
    tracing::info!("Sahi master contract mapped: {} instruments", rows.len());
    Ok(rows)
}

/// OpenAlgo index symbol for a Sahi index display name.
pub fn index_symbol(name: &str, segment: &str) -> String {
    let key = name
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_uppercase();
    let table = if segment == "NSE" {
        NSE_INDEX_SYMBOLS
    } else {
        BSE_INDEX_SYMBOLS
    };
    match table.iter().find(|(from, _)| *from == key) {
        Some((_, to)) => (*to).to_string(),
        None => key
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '&')
            .collect(),
    }
}

/// The columns a row is read by.
struct Cols {
    id: usize,
    symbol: usize,
    company: usize,
    series: usize,
    underlying: usize,
    segment: usize,
    itype: usize,
    option: usize,
    expiry: usize,
    strike: usize,
    lot: usize,
    tick: usize,
}

impl Cols {
    fn of(h: &CsvHeader) -> Result<Self> {
        let col = |n: &str| {
            h.index(n).ok_or_else(|| {
                tracing::warn!("Sahi scrip master has no {} column", n);
                AppError::Broker(
                    "The Sahi instrument list has changed shape and could not be read. Your existing symbols were kept.".into(),
                )
            })
        };
        Ok(Self {
            id: col("id")?,
            symbol: col("symbol")?,
            company: col("company_name")?,
            series: col("series")?,
            underlying: col("underlying_symbol")?,
            segment: col("segment")?,
            itype: col("instrument_type")?,
            option: col("option_type")?,
            expiry: col("expiry_date")?,
            strike: col("strike_price")?,
            lot: col("lot_size")?,
            tick: col("tick_size")?,
        })
    }
}

/// Every supported row of the CSV, deduplicated by (symbol, exchange)
/// keeping the lowest id: the BSE index list repeats some codes under
/// several ids, and a cash symbol can repeat across series.
pub fn parse(text: &str) -> Result<Vec<SymbolData>> {
    let mut lines = text.lines();
    let header = CsvHeader::parse(lines.next().unwrap_or(""));
    let cols = Cols::of(&header)?;
    let mut best: HashMap<(String, String), (i64, SymbolData)> = HashMap::new();
    let mut unreadable_expiry = 0usize;
    for line in lines.filter(|l| !l.trim().is_empty()) {
        let f = split_csv_line(line);
        let get = |i: usize| f.get(i).map(|s| s.trim()).unwrap_or("");
        match row(&cols, &get) {
            Row::Skip => {}
            Row::BadExpiry => unreadable_expiry += 1,
            Row::Ok(boxed) => {
                let r = *boxed;
                let id = get(cols.id).parse::<i64>().unwrap_or(i64::MAX);
                let key = (r.symbol.clone(), r.exchange.clone());
                if best.get(&key).is_none_or(|(old, _)| id < *old) {
                    best.insert(key, (id, r));
                }
            }
        }
    }
    if unreadable_expiry > 0 {
        tracing::warn!(
            "Skipped {} Sahi derivative rows with an unreadable expiry",
            unreadable_expiry
        );
    }
    let mut rows: Vec<(i64, SymbolData)> = best.into_values().collect();
    rows.sort_by_key(|(id, _)| *id);
    Ok(rows.into_iter().map(|(_, r)| r).collect())
}

enum Row {
    // Boxed: a master row is far larger than the other variants.
    Ok(Box<SymbolData>),
    Skip,
    BadExpiry,
}

fn row<'a>(c: &Cols, get: &dyn Fn(usize) -> &'a str) -> Row {
    let segment = get(c.segment);
    if !SEGMENTS.contains(&segment) {
        return Row::Skip;
    }
    let itype = get(c.itype);
    let option = get(c.option);
    let num = |i: usize| get(i).parse::<f64>().unwrap_or(0.0);
    let mut r = SymbolData {
        symbol: get(c.symbol).to_string(),
        brsymbol: get(c.symbol).to_string(),
        name: get(c.company).to_string(),
        exchange: segment.to_string(),
        brexchange: segment.to_string(),
        token: get(c.id).to_string(),
        expiry: String::new(),
        strike: 0.0,
        lot_size: num(c.lot) as i32,
        instrument_type: "EQ".to_string(),
        tick_size: num(c.tick),
    };
    match itype {
        "STK" | "ETF" => r.symbol = cash_symbol(&r.symbol, segment, get(c.series)),
        "IDX" => {
            // No tradable symbol: the display name is the broker symbol.
            r.brsymbol = r.name.clone();
            r.exchange = format!("{}_INDEX", segment);
            r.symbol = index_symbol(&r.name, segment);
        }
        "FUTIDX" | "FUTSTK" | "OPTIDX" | "OPTSTK" => {
            let is_option = itype.starts_with("OPT");
            if is_option && !matches!(option, "CE" | "PE") {
                return Row::Skip;
            }
            let Ok(date) = NaiveDate::parse_from_str(get(c.expiry), "%d-%m-%Y") else {
                return Row::BadExpiry;
            };
            let underlying = get(c.underlying);
            r.expiry = format_expiry(date).to_uppercase();
            r.name = underlying.to_string();
            if is_option {
                r.strike = num(c.strike) + 0.0;
                r.instrument_type = option.to_string();
                r.symbol = option_symbol(underlying, &r.expiry, r.strike, option);
            } else {
                r.instrument_type = "FUT".to_string();
                r.symbol = future_symbol(underlying, &r.expiry);
            }
        }
        _ => return Row::Skip,
    }
    if r.symbol.is_empty() || r.brsymbol.is_empty() {
        return Row::Skip;
    }
    Row::Ok(Box::new(r))
}

/// Sahi drops the `-EQ` suffix on NSE but keeps other series
/// (`3IINFOLTD-BE`); OpenAlgo's symbol is the bare name, as on other
/// brokers. Only a suffix equal to the row's own series is stripped, so a
/// name with a hyphen of its own (`BAJAJ-AUTO`) is untouched.
pub fn cash_symbol(symbol: &str, segment: &str, series: &str) -> String {
    if segment != "NSE" || series.is_empty() || series == "EQ" {
        return symbol.to_string();
    }
    symbol
        .strip_suffix(&format!("-{}", series))
        .unwrap_or(symbol)
        .to_string()
}
