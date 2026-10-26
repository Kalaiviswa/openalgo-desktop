//! Angel scrip master (web `database/master_contract_db.py`
//! `process_angel_json`).
//!
//! One JSON array; every exchange is kept (no exchange filter, as on the web).
//! * CDS `UNDIRC` and NCDEX `AMXIDX` rows are dropped ([`is_listed`]).
//! * `brsymbol` = Angel `symbol`, `brexchange` = `exch_seg`.
//! * `AMXIDX` rows on NSE/BSE/MCX move to `NSE_INDEX`/`BSE_INDEX`/`MCX_INDEX`.
//! * `-EQ`/`-BE`/`-MF`/`-SG` are removed anywhere in the symbol.
//! * expiry `19MAR2024` -> `19-MAR-24` (unparsable kept, uppercased).
//! * strike `/100`; CDS `OPTCUR`/`OPTIRC` a further `/100000`.
//! * CDS, MCX and BFO derivatives are rebuilt as `name + DDMMMYY + FUT` or
//!   `name + DDMMMYY + strike + CE|PE`; NFO keeps Angel's symbol.
//! * Index symbols come from `name` (upper, no spaces or hyphens, BSE also
//!   without `S&P `), then the web's override table, applied to every row.
//! * instrumenttype: options -> CE/PE by symbol suffix, futures -> FUT,
//!   index rows -> EQ.

use super::AngelBroker;
use crate::brokers::common::de::{f64_lenient, i64_lenient, string_lenient};
use crate::brokers::common::http;
use crate::brokers::common::master_contract::{expiry_compact, format_expiry};
use crate::brokers::common::symbols::SymToken;
use crate::error::Result;
use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AngelScrip {
    #[serde(deserialize_with = "string_lenient")]
    pub token: String,
    #[serde(deserialize_with = "string_lenient")]
    pub symbol: String,
    #[serde(deserialize_with = "string_lenient")]
    pub name: String,
    #[serde(deserialize_with = "string_lenient")]
    pub expiry: String,
    #[serde(deserialize_with = "f64_lenient")]
    pub strike: f64,
    #[serde(deserialize_with = "i64_lenient")]
    pub lotsize: i64,
    #[serde(deserialize_with = "string_lenient")]
    pub instrumenttype: String,
    #[serde(deserialize_with = "string_lenient")]
    pub exch_seg: String,
    #[serde(deserialize_with = "f64_lenient")]
    pub tick_size: f64,
}

/// web `convert_date` + `.str.upper()`.
pub fn convert_expiry(s: &str) -> String {
    match chrono::NaiveDate::parse_from_str(&s.trim().to_ascii_uppercase(), "%d%b%Y") {
        Ok(d) => format_expiry(d),
        Err(_) => s.to_ascii_uppercase(),
    }
}

/// Python `str(float)` then `.replace(r"\.0", "")` as a regex (the web's
/// strike text): `25000.0` -> `25000`, `83.25` -> `83.25`, and an interior
/// `.0` is dropped too (`100.05` -> `1005`), kept for symbol parity.
pub fn strike_text(strike: f64) -> String {
    let s = if strike.is_finite() && strike.fract() == 0.0 && strike.abs() < 1e16 {
        format!("{}.0", strike as i64)
    } else {
        format!("{}", strike)
    };
    s.replace(".0", "")
}

/// Index symbol from the scrip `name` (web NSE_INDEX / BSE_INDEX rules).
pub fn index_symbol(name: &str, exchange: &str) -> String {
    let mut s = name.to_uppercase();
    if exchange == "BSE_INDEX" {
        s = s.replace("S&P ", "");
    }
    s.replace([' ', '-'], "")
}

/// The web's final `df["symbol"].replace({...})` over every row.
pub fn symbol_override(s: &str) -> Option<&'static str> {
    Some(match s {
        "NIFTY50" => "NIFTY",
        "NIFTYBANK" => "BANKNIFTY",
        "NIFTYFINSERVICE" => "FINNIFTY",
        "NIFTYNEXT50" => "NIFTYNXT50",
        "NIFTYMIDSELECT" | "NIFTYMIDCAPSELECT" => "MIDCPNIFTY",
        // BSE_INDEX symbols come from the name: "S&P BSE SENSEX 50".
        "SNSX50" | "BSESENSEX50" => "SENSEX50",
        _ => return None,
    })
}

/// One master row -> `SymToken` (web `process_angel_json`).
pub fn process_scrip(s: AngelScrip) -> SymToken {
    let brsymbol = s.symbol.clone();
    let brexchange = s.exch_seg.clone();
    let it = s.instrumenttype.as_str();
    let exchange = match (it, s.exch_seg.as_str()) {
        ("AMXIDX", "NSE") => "NSE_INDEX".to_string(),
        ("AMXIDX", "BSE") => "BSE_INDEX".to_string(),
        ("AMXIDX", "MCX") => "MCX_INDEX".to_string(),
        (_, other) => other.to_string(),
    };
    let mut symbol = ["-EQ", "-BE", "-MF", "-SG"]
        .iter()
        .fold(brsymbol.clone(), |acc, suf| acc.replace(suf, ""));
    let expiry = convert_expiry(&s.expiry);
    let mut strike = s.strike / 100.0;
    let ex = exchange.as_str();
    if ex == "CDS" && matches!(it, "OPTCUR" | "OPTIRC") {
        strike /= 100_000.0;
    }
    let date = expiry_compact(&expiry);
    let last2: String = {
        let chars: Vec<char> = symbol.chars().collect();
        chars[chars.len().saturating_sub(2)..].iter().collect()
    };
    match (it, ex) {
        ("FUTCUR" | "FUTIRC", "CDS") | ("FUTCOM", "MCX") | ("FUTIDX" | "FUTSTK", "BFO") => {
            symbol = format!("{}{}FUT", s.name, date);
        }
        ("OPTCUR" | "OPTIRC", "CDS") | ("OPTFUT", "MCX") => {
            symbol = format!("{}{}{}{}", s.name, date, strike_text(strike), last2);
        }
        ("OPTIDX" | "OPTSTK", "BFO") if last2 == "CE" || last2 == "PE" => {
            symbol = format!("{}{}{}{}", s.name, date, strike_text(strike), last2);
        }
        _ => {}
    }
    if ex == "NSE_INDEX" || ex == "BSE_INDEX" {
        symbol = index_symbol(&s.name, ex);
    }
    if let Some(o) = symbol_override(&symbol) {
        symbol = o.to_string();
    }
    let instrument_type = match it {
        // Indices are typed EQ, as Kite types its INDICES segment; the
        // *_INDEX exchange is what says the row is an index (QA MC-04).
        _ if ex.ends_with("_INDEX") => "EQ".to_string(),
        "OPTIDX" | "OPTSTK" | "OPTFUT" | "OPTCUR" | "OPTIRC" if symbol.ends_with("CE") => {
            "CE".to_string()
        }
        "OPTIDX" | "OPTSTK" | "OPTFUT" | "OPTCUR" | "OPTIRC" if symbol.ends_with("PE") => {
            "PE".to_string()
        }
        "FUTIDX" | "FUTSTK" | "FUTCOM" | "FUTCUR" | "FUTIRC" | "FUTIRT" => "FUT".to_string(),
        other => other.to_string(),
    };
    SymToken {
        symbol,
        brsymbol,
        name: s.name,
        exchange,
        brexchange,
        token: s.token,
        expiry,
        strike,
        lot_size: i32::try_from(s.lotsize).unwrap_or(1),
        instrument_type,
        tick_size: s.tick_size / 100.0,
    }
}

/// Whether a scrip master row belongs in the symbol master (web QA MC-03).
/// Two kinds are not tradable contracts and have nowhere to go; Kite's and
/// Dhan's masters carry neither:
/// * CDS `UNDIRC`: the government bonds under the interest-rate derivatives.
///   Angel names some R-series rows without their `R-` prefix, so token 108
///   (`R636GS2031`) repeats token 107's symbol `GOI6.36%2031`. The bonds
///   trade on NSE as `-GS` rows, which stay.
/// * NCDEX `AMXIDX`: indices, but there is no NCDEX index exchange, so they
///   would sit on NCDEX and `AGRIDEX` would clash with the spot row.
fn is_listed(s: &AngelScrip) -> bool {
    !matches!(
        (s.instrumenttype.as_str(), s.exch_seg.as_str()),
        ("UNDIRC", "CDS") | ("AMXIDX", "NCDEX")
    )
}

fn process_rows(rows: Vec<AngelScrip>) -> Vec<SymToken> {
    rows.into_iter()
        .filter(is_listed)
        .map(process_scrip)
        .collect()
}

/// Parse the whole scrip master JSON.
pub fn parse_master(json: &[u8]) -> Result<Vec<SymToken>> {
    let rows: Vec<AngelScrip> = serde_json::from_slice(json)?;
    Ok(process_rows(rows))
}

pub async fn download(b: &AngelBroker) -> Result<Vec<SymToken>> {
    let resp = b
        .http
        .get(&b.master_url)
        .timeout(http::DOWNLOAD_TIMEOUT)
        .send()
        .await?;
    let (_, rows): (_, Vec<AngelScrip>) = http::read_json("angel", resp).await?;
    let out = process_rows(rows);
    tracing::info!("Angel One master contract: {} instruments", out.len());
    Ok(out)
}
