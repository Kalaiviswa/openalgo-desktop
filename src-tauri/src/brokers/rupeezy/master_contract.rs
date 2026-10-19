//! The Vortex instrument master -> OpenAlgo symbol master (web
//! `database/master_contract_db.py`, `mapping/exchange.py`).
//!
//! `https://static.rupeezy.in/master.csv` is public (about 160k rows):
//! `token, exchange, symbol, instrument_name, series, expiry_date,
//! option_type, strike_price, tick, lot_size, eligibility, security_desc,
//! asm_gsm_stage, last_trading_date, isin_code, ticker, has_cas_session`.
//!
//! * `exchange`: `NSE_EQ`, `BSE_EQ`, `NSE_FO`, `BSE_FO`, `MCX_FO`; indices
//!   are `EQIDX` rows inside `NSE_EQ` / `BSE_EQ`.
//! * `expiry_date` `YYYYMMDD`; `option_type` `CE` / `PE`, `XX` for futures;
//!   `strike_price` in rupees (futures carry -0.01 / 0); `tick` in paise;
//!   `lot_size` the real lot, MCX included.
//! * `brsymbol` is the ticker without its prefix, `brexchange` the Vortex
//!   exchange.
//! * Vortex marks a contract it does not allow with `eligibility` 0. Index
//!   rows always carry 0 and are kept: quotes, charts and option chains need
//!   them.

use super::mapping::split_ticker;
use super::RupeezyBroker;
use crate::brokers::common::http::DOWNLOAD_TIMEOUT;
use crate::brokers::common::master_contract::{
    expiry_compact, format_expiry, format_strike, split_csv_line, CsvHeader,
};
use crate::brokers::common::symbols::SymToken;
use crate::error::{AppError, Result};
use chrono::NaiveDate;
use std::collections::HashSet;

/// Vortex index `symbol` -> OpenAlgo `NSE_INDEX` symbol
/// (`docs/prompt/symbol-format.md`); indices not listed keep their name.
const NSE_INDEX_SYMBOLS: &[(&str, &str)] = &[
    ("AUTO", "NIFTYAUTO"),
    ("BEESNAV", "HANGSENGBEESNAV"),
    ("COMMODITIES", "NIFTYCOMMODITIES"),
    ("CONSUMPTION", "NIFTYCONSUMPTION"),
    ("DIVOPPS", "NIFTYDIVOPPS50"),
    ("DIVPOINT", "NIFTY50DIVPOINT"),
    ("ENEGRY", "NIFTYENERGY"),
    ("FMCG", "NIFTYFMCG"),
    ("GROWTH", "NIFTYGROWSECT15"),
    ("INFRA", "NIFTYINFRA"),
    ("MEDIA", "NIFTYMEDIA"),
    ("METAL", "NIFTYMETAL"),
    ("MNC", "NIFTYMNC"),
    ("PHARMA", "NIFTYPHARMA"),
    ("REALTY", "NIFTYREALTY"),
    ("SERVICE", "NIFTYSERVSECTOR"),
    ("VALUE20", "NIFTY50VALUE20"),
    ("NIFGS10YRCLN", "NIFTYGS10YRCLN"),
    ("NIFGS1115YR", "NIFTYGS1115YR"),
    ("NIFGS15YRPLS", "NIFTYGS15YRPLUS"),
    ("NIFGSCOMPS", "NIFTYGSCOMPSITE"),
    ("NIFTY100EQL", "NIFTY100EQLWGT"),
    ("NIFTY100ESGSL", "NIFTY100ESGSECLDR"),
    ("NIFTY100LV30", "NIFTY100LOWVOL30"),
    ("NIFTY200MTM30", "NIFTY200MOMENTM30"),
    ("NIFTY200QT30", "NIFTY200QUALTY30"),
    ("NIFTY500MTCAP", "NIFTY500MULTICAP"),
    ("NIFTY50EQL", "NIFTY50EQLWGT"),
    ("NIFTYALOWVOL", "NIFTYALPHALOWVOL"),
    ("NIFTYCONDUR", "NIFTYCONSRDURBL"),
    ("NIFTYFINS2550", "NIFTYFINSRV2550"),
    ("NIFTYHELCARE", "NIFTYHEALTHCARE"),
    ("NIFTYLIQ15", "NIFTY100LIQ15"),
    ("NIFTYLMID250", "NIFTYLARGEMID250"),
    ("NIFTYMCAP150", "NIFTYMIDCAP150"),
    ("NIFTYMDSL400", "NIFTYMIDSML400"),
    ("NIFTYMIC250", "NIFTYMICROCAP250"),
    ("NIFTYMID100", "NIFTYMIDCAP100"),
    ("NIFTYMID50", "NIFTYMIDCAP50"),
    ("NIFTYMIDLQ15", "NIFTYMIDLIQ15"),
    ("NIFTYMLTINF", "NIFTYMULTIINFRA"),
    ("NIFTYMLTMFG", "NIFTYMULTIMFG"),
    ("NIFTYMSHLTH", "NIFTYMIDSMLHLTH"),
    ("NIFTYOIL&GAS", "NIFTYOILANDGAS"),
    ("NIFTYPR1X", "NIFTY50PR1XINV"),
    ("NIFTYPR2X", "NIFTY50PR2XLEV"),
    ("NIFTYQTY30", "NIFTYQUALITY30"),
    ("NIFTYSCAP250", "NIFTYSMLCAP250"),
    ("NIFTYSCAP50", "NIFTYSMLCAP50"),
    ("NIFTYSMALL", "NIFTYSMLCAP100"),
    ("NIFTYTATA25", "NIFTYTATA25CAP"),
    ("NIFTYTR1X", "NIFTY50TR1XINV"),
    ("NIFTYTR2X", "NIFTY50TR2XLEV"),
];

/// Vortex index `symbol` -> OpenAlgo `BSE_INDEX` symbol.
const BSE_INDEX_SYMBOLS: &[(&str, &str)] = &[
    ("ALLCAP", "BSEALLCAP"),
    ("BHRT22", "BSEBHARAT22INDEX"),
    ("BSECD", "BSECONSUMERDURABLES"),
    ("BSECG", "BSECAPITALGOODS"),
    ("BSEFMC", "BSEFASTMOVINGCONSUMERGOODS"),
    ("BSEHC", "BSEHEALTHCARE"),
    ("BSEIT", "BSEINFORMATIONTECHNOLOGY"),
    ("BSEMETL", "BSEMETAL"),
    ("BSEPBI", "BSEPRIVATEBANKS"),
    ("CARBON", "BSECARBONEX"),
    ("CONDIS", "BSECONSUMERDISCRETIONARYGOODS&SERVICES"),
    ("DFRGRI", "BSEDIVERSIFIEDFINANCIALSREVENUEGROWTHINDEX"),
    ("DOL100", "BSEDOLLEX100"),
    ("DOL200", "BSEDOLLEX200"),
    ("DOL30", "BSEDOLLEX30"),
    ("ENERGY", "BSEENERGY"),
    ("ESG100", "BSE100ESG"),
    ("FINSER", "BSEFINANCIALSERVICES"),
    ("GREENX", "BSEGREENEX"),
    ("INDSTR", "BSEINDUSTRIALS"),
    ("INFRA", "BSEINDIAINFRASTRUCTUREINDEX"),
    ("LCTMCI", "BSE100LARGECAPTMCINDEX"),
    ("LMI250", "BSE250LARGEMIDCAPINDEX"),
    ("LRGCAP", "BSELARGECAP"),
    ("MFG", "BSEINDIAMANUFACTURING"),
    ("MID150", "BSE150MIDCAPINDEX"),
    ("MIDCAP", "BSEMIDCAP"),
    ("MIDSEL", "BSEMIDCAPSELECTINDEX"),
    ("MSL400", "BSE400MIDSMALLCAPINDEX"),
    ("OILGAS", "BSEOIL&GAS"),
    ("POWER", "BSEPOWER"),
    ("REALTY", "BSEREALTY"),
    ("SML250", "BSE250SMALLCAPINDEX"),
    ("SMLCAP", "BSESMALLCAP"),
    ("SMLSEL", "BSESMALLCAPSELECTINDEX"),
    ("SMEIPO", "BSESMEIPO"),
    ("SNSX50", "SENSEX50"),
    ("SNXT50", "BSESENSEXNEXT50"),
    ("TELCOM", "BSETELECOM"),
    ("UTILS", "BSEUTILITIES"),
];

/// A Vortex index symbol as OpenAlgo names it on `exchange` (keyed per
/// exchange: NSE and BSE both publish INFRA and REALTY).
pub fn index_symbol(vortex: &str, exchange: &str) -> String {
    let table = match exchange {
        "NSE_INDEX" => NSE_INDEX_SYMBOLS,
        "BSE_INDEX" => BSE_INDEX_SYMBOLS,
        _ => &[],
    };
    table
        .iter()
        .find(|(k, _)| *k == vortex)
        .map(|(_, v)| (*v).to_string())
        .unwrap_or_else(|| vortex.to_string())
}

/// The OpenAlgo exchange of a master row: indices by their Vortex exchange,
/// tradable rows by theirs; `None` for a segment OpenAlgo does not list.
fn row_exchange(brexchange: &str, is_index: bool) -> Option<&'static str> {
    Some(match (brexchange, is_index) {
        ("NSE_EQ", true) => "NSE_INDEX",
        ("BSE_EQ", true) => "BSE_INDEX",
        ("NSE_EQ", false) => "NSE",
        ("BSE_EQ", false) => "BSE",
        ("NSE_FO", false) => "NFO",
        ("BSE_FO", false) => "BFO",
        ("MCX_FO", false) => "MCX",
        _ => return None,
    })
}

fn round6(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

fn bad_format(col: &str) -> AppError {
    tracing::error!("Rupeezy instrument master has no '{}' column", col);
    AppError::Broker(
        "Rupeezy's instrument list has an unexpected format. Try downloading the master contract again later."
            .into(),
    )
}

/// The columns a master row is read from, by header name.
struct Columns {
    token: usize,
    exchange: usize,
    symbol: usize,
    instrument: usize,
    expiry: usize,
    option_type: usize,
    strike: usize,
    tick: usize,
    lot: usize,
    eligibility: usize,
    desc: usize,
    ticker: usize,
}

impl Columns {
    fn parse(header: &CsvHeader) -> Result<Self> {
        let col = |n: &str| header.index(n).ok_or_else(|| bad_format(n));
        Ok(Self {
            token: col("token")?,
            exchange: col("exchange")?,
            symbol: col("symbol")?,
            instrument: col("instrument_name")?,
            expiry: col("expiry_date")?,
            option_type: col("option_type")?,
            strike: col("strike_price")?,
            tick: col("tick")?,
            lot: col("lot_size")?,
            eligibility: col("eligibility")?,
            desc: col("security_desc")?,
            ticker: col("ticker")?,
        })
    }
}

/// One CSV line as a master row; `None` for a segment OpenAlgo does not
/// list, a contract Vortex does not allow, or a row without a ticker.
fn parse_row(c: &Columns, line: &str) -> Option<SymToken> {
    let f = split_csv_line(line);
    let g = |i: usize| f.get(i).map(|s| s.trim()).unwrap_or("");
    let brexchange = g(c.exchange).to_ascii_uppercase();
    let is_index = g(c.instrument).eq_ignore_ascii_case("EQIDX");
    let eligible = g(c.eligibility) != "0";
    let exchange = row_exchange(&brexchange, is_index)?;
    let brsymbol = split_ticker(g(c.ticker)).to_string();
    if brsymbol.is_empty() || !(eligible || is_index) {
        return None;
    }
    let base = g(c.symbol).to_string();
    let option_type = g(c.option_type).to_ascii_uppercase();
    let is_derivative = brexchange.ends_with("_FO");
    let is_option = is_derivative && (option_type == "CE" || option_type == "PE");
    let expiry = NaiveDate::parse_from_str(g(c.expiry), "%Y%m%d")
        .map(format_expiry)
        .unwrap_or_default();
    let strike = if is_option {
        g(c.strike)
            .parse::<f64>()
            .ok()
            .filter(|s| s.is_finite())
            .unwrap_or(0.0)
            .max(0.0)
    } else {
        0.0
    };
    let (symbol, instrument_type) = if is_option {
        (
            format!(
                "{}{}{}{}",
                base,
                expiry_compact(&expiry),
                format_strike(strike),
                option_type
            ),
            option_type,
        )
    } else if is_derivative {
        (
            format!("{}{}FUT", base, expiry_compact(&expiry)),
            "FUT".into(),
        )
    } else if is_index {
        (index_symbol(&base, exchange), "EQ".into())
    } else {
        (brsymbol.clone(), "EQ".into())
    };
    let desc = g(c.desc);
    Some(SymToken {
        symbol,
        name: if is_derivative || desc.is_empty() {
            base
        } else {
            desc.to_string()
        },
        brsymbol,
        exchange: exchange.into(),
        brexchange,
        token: g(c.token).to_string(),
        expiry,
        strike: strike + 0.0,
        lot_size: g(c.lot).parse::<f64>().map(|v| v as i32).unwrap_or(0),
        instrument_type,
        tick_size: round6(g(c.tick).parse::<f64>().unwrap_or(0.0) / 100.0),
    })
}

/// `master.csv` -> master rows (web `process_master`), the first row of a
/// `(symbol, exchange)` pair kept.
pub fn parse_master(csv: &str) -> Result<Vec<SymToken>> {
    let mut lines = csv.lines();
    let header = CsvHeader::parse(lines.next().ok_or_else(|| bad_format("token"))?);
    let c = Columns::parse(&header)?;
    let mut seen = HashSet::new();
    Ok(lines
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| parse_row(&c, l))
        .filter(|r| seen.insert((r.symbol.clone(), r.exchange.clone())))
        .collect())
}

pub async fn download(b: &RupeezyBroker) -> Result<Vec<SymToken>> {
    // About 15 MB: the download budget, not the REST one.
    let resp = b
        .http
        .get(&b.urls.master)
        .timeout(DOWNLOAD_TIMEOUT)
        .send()
        .await
        .map_err(crate::brokers::common::redact::http)?;
    let status = resp.status();
    if !status.is_success() {
        tracing::warn!(
            status = status.as_u16(),
            "Rupeezy instrument download failed"
        );
        return Err(AppError::Broker(
            "Rupeezy did not send its instrument list. Try downloading the master contract again shortly."
                .into(),
        ));
    }
    let text = resp
        .text()
        .await
        .map_err(crate::brokers::common::redact::http)?;
    let rows = parse_master(&text)?;
    if rows.is_empty() {
        return Err(AppError::Broker(
            "Rupeezy's instrument list was empty. Your existing symbols were kept; try the download again later."
                .into(),
        ));
    }
    Ok(rows)
}
