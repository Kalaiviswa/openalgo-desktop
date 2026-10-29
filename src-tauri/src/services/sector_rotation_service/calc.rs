//! The relative rotation calculation (web `services/sector_rotation_service.py`):
//! pure, every input an argument.
//!
//! RS-Ratio (is the symbol stronger than the benchmark?) and RS-Momentum (is
//! that strength still building?), both centred on 100, from linearly
//! weighted moving averages of the relative strength on daily bars, or on
//! weekly bars folded from them.

use crate::brokers::common::mpp::py_round;
use chrono::{Datelike, Days, NaiveDate};
use serde_json::{json, Value};

/// Closes by session date, oldest first, one per date.
pub type Series = Vec<(NaiveDate, f64)>;

/// The map's two timeframes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timeframe {
    Daily,
    Weekly,
}

impl Timeframe {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "daily" => Some(Timeframe::Daily),
            "weekly" => Some(Timeframe::Weekly),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Timeframe::Daily => "daily",
            Timeframe::Weekly => "weekly",
        }
    }

    /// The lengths, in bars of the timeframe (web `PARAMS`).
    pub fn params(self) -> Params {
        match self {
            Timeframe::Daily => Params {
                fast: 14,
                slow: 36,
                momentum: 8,
            },
            Timeframe::Weekly => Params {
                fast: 5,
                slow: 15,
                momentum: 5,
            },
        }
    }

    /// Calendar days of history read (web `LOOKBACK_DAYS`).
    pub fn lookback_days(self) -> u64 {
        match self {
            Timeframe::Daily => 365,
            Timeframe::Weekly => 730,
        }
    }

    /// What a bar of the timeframe is called in a reason.
    pub fn unit(self) -> &'static str {
        match self {
            Timeframe::Daily => "sessions",
            Timeframe::Weekly => "weeks",
        }
    }
}

/// Lengths used by the calculation (web `RotationParams`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Params {
    pub fast: usize,
    pub slow: usize,
    pub momentum: usize,
}

impl Params {
    /// Bars needed before the first point exists.
    pub fn min_bars(self) -> usize {
        self.slow + self.momentum
    }
}

/// One point of a trail.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub date: NaiveDate,
    pub rs_ratio: f64,
    pub rs_momentum: f64,
}

/// The Friday that closes the week of `d` (pandas `W-FRI`: a week runs
/// Saturday to Friday).
fn week_end(d: NaiveDate) -> NaiveDate {
    let ahead = (4 + 7 - d.weekday().num_days_from_monday()) % 7;
    d + Days::new(u64::from(ahead))
}

/// One close per week ending Friday, stamped with the last session that
/// traded, so a week in progress is not dated in the future and a week
/// that ended on a Thursday holiday shows the Thursday (web `to_weekly`).
pub fn to_weekly(closes: &[(NaiveDate, f64)]) -> Series {
    let mut out: Series = Vec::new();
    let mut week = None;
    for &(d, c) in closes {
        let w = week_end(d);
        match out.last_mut() {
            Some(last) if week == Some(w) => *last = (d, c),
            _ => {
                out.push((d, c));
                week = Some(w);
            }
        }
    }
    out
}

/// The sessions both series traded, oldest first.
fn joined(a: &[(NaiveDate, f64)], b: &[(NaiveDate, f64)]) -> Vec<(NaiveDate, f64, f64)> {
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < a.len() && j < b.len() {
        match a[i].0.cmp(&b[j].0) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                if a[i].1.is_finite() && b[j].1.is_finite() {
                    out.push((a[i].0, a[i].1, b[j].1));
                }
                i += 1;
                j += 1;
            }
        }
    }
    out
}

/// The symbol's and the benchmark's bars on a shared calendar: daily
/// sessions aligned before any weekly fold, so a week that ended on a
/// holiday for one side only still pairs (web `paired_bars`).
pub fn paired_bars(closes: &Series, benchmark: &Series, tf: Timeframe) -> (Series, Series) {
    let pair = joined(closes, benchmark);
    let symbol: Series = pair.iter().map(|&(d, s, _)| (d, s)).collect();
    let bench: Series = pair.iter().map(|&(d, _, b)| (d, b)).collect();
    match tf {
        Timeframe::Weekly => (to_weekly(&symbol), to_weekly(&bench)),
        Timeframe::Daily => (symbol, bench),
    }
}

/// Linearly weighted moving average, weights 1..=len with the newest bar
/// heaviest; `None` until `len` bars exist (web `wma`).
pub fn wma(values: &[f64], len: usize) -> Vec<Option<f64>> {
    let total = (len * (len + 1) / 2) as f64;
    (0..values.len())
        .map(|i| {
            (len > 0 && i + 1 >= len).then(|| {
                let window = &values[i + 1 - len..=i];
                let dot: f64 = window
                    .iter()
                    .enumerate()
                    .map(|(k, v)| (k + 1) as f64 * v)
                    .sum();
                dot / total
            })
        })
        .collect()
}

/// RS-Ratio and RS-Momentum for every bar where both are defined, on the
/// sessions both traded (web `rotation_series`).
pub fn rotation_series(sector: &Series, benchmark: &Series, tf: Timeframe) -> Vec<Point> {
    let p = tf.params();
    let pair = joined(sector, benchmark);
    let rs: Vec<f64> = pair.iter().map(|&(_, s, b)| 100.0 * s / b).collect();
    let (fast, slow) = (wma(&rs, p.fast), wma(&rs, p.slow));
    let ratio: Vec<Option<f64>> = fast
        .iter()
        .zip(&slow)
        .map(|(f, s)| Some(100.0 * (*f)? / (*s)?))
        .collect();
    (0..pair.len())
        .filter_map(|i| {
            let r = ratio[i]?;
            let before = ratio[i.checked_sub(p.momentum)?]?;
            Some(Point {
                date: pair[i].0,
                rs_ratio: r,
                rs_momentum: 100.0 * r / before,
            })
        })
        .collect()
}

/// The quadrant of a point; exactly 100 counts as strong (web `quadrant`).
pub fn quadrant(rs_ratio: f64, rs_momentum: f64) -> &'static str {
    match (rs_ratio >= 100.0, rs_momentum >= 100.0) {
        (true, true) => "leading",
        (true, false) => "weakening",
        (false, true) => "improving",
        (false, false) => "lagging",
    }
}

/// Consecutive bars, ending with the latest, in its quadrant.
pub fn bars_in_quadrant(points: &[Point]) -> usize {
    let Some(last) = points.last() else {
        return 0;
    };
    let current = quadrant(last.rs_ratio, last.rs_momentum);
    points
        .iter()
        .rev()
        .take_while(|p| quadrant(p.rs_ratio, p.rs_momentum) == current)
        .count()
}

/// The latest position, its quadrant, its run length and its trail of at
/// most `tail` points (web `summarise`).
pub fn summarise(points: &[Point], tail: usize) -> Option<Value> {
    let last = points.last()?;
    let trail = &points[points.len().saturating_sub(tail)..];
    Some(json!({
        "quadrant": quadrant(last.rs_ratio, last.rs_momentum),
        "rs_ratio": py_round(last.rs_ratio, 2),
        "rs_momentum": py_round(last.rs_momentum, 2),
        "bars_in_quadrant": bars_in_quadrant(points),
        "tail": trail.iter().map(|p| json!({
            "date": p.date.format("%Y-%m-%d").to_string(),
            "rs_ratio": py_round(p.rs_ratio, 2),
            "rs_momentum": py_round(p.rs_momentum, 2),
        })).collect::<Vec<_>>(),
    }))
}
