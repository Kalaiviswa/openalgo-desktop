//! Strike Profile arithmetic (web `strike_profile_service.py`): gamma
//! exposure (GEX) and delta exposure (DEX) per strike, the gamma flip, the
//! call and put walls and the strike window the first chain is fetched with.
//!
//! Units, chosen so every cell of the strike by expiry grid is in the same
//! currency:
//!
//! - GEX = gamma x OI x spot^2 x 1% / 1e7: rupees crore hedgers would trade
//!   for a 1% move in the underlying. Calls count positive and puts
//!   negative (the dealer is long the calls and short the puts).
//! - DEX = delta x OI x spot / 1e7: the delta-weighted notional of the open
//!   interest, rupees crore. Put delta is already negative.
//!
//! OI is the broker's open interest in units (quantity).

use super::chain::safe_gamma;

/// One crore: exposures are reported in rupees crore.
pub const CRORE: f64 = 1e7;

/// Strikes fetched either side of ATM for the first chain, and when the
/// window cannot be worked out.
pub const DEFAULT_STRIKE_COUNT: i64 = 20;

/// The ceiling on any window, and the window of "All": 40 a side is 162
/// symbols per expiry, the order of the largest chain the other tools ask
/// for.
pub const MAX_STRIKE_COUNT: i64 = 40;

/// Hypothetical spot prices tried when searching for the gamma flip.
pub const FLIP_GRID_POINTS: usize = 241;

/// Shortest time to expiry a flip leg is priced with (about an hour, below
/// which Black-76 is unstable; web `_MIN_YEARS`).
pub const MIN_YEARS: f64 = 0.0001;

/// GEX of one leg in rupees crore; `sign` is +1 for a call, -1 for a put.
pub fn gex(gamma: f64, oi: f64, spot: f64, sign: f64) -> f64 {
    sign * gamma * oi * spot * spot * 0.01 / CRORE
}

/// DEX of one leg in rupees crore.
pub fn dex(delta: f64, oi: f64, spot: f64) -> f64 {
    delta * oi * spot / CRORE
}

/// One option leg in the gamma-flip search.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlipLeg {
    pub strike: f64,
    /// The forward its chain was priced against.
    pub forward: f64,
    pub years: f64,
    /// Implied volatility as a decimal (0.12, not 12).
    pub iv: f64,
    pub oi: f64,
    /// +1 for a call, -1 for a put.
    pub sign: f64,
}

/// The underlying price at which total GEX changes sign (web
/// `find_gamma_flip`): every leg's forward moves in proportion to spot, its
/// IV is held, and its Black-76 gamma is re-priced at each of `points`
/// prices from `low` to `high`. Returns the crossing nearest to `spot`,
/// linearly interpolated, or `None` when the total keeps one sign.
pub fn find_gamma_flip(
    legs: &[FlipLeg],
    spot: f64,
    low: f64,
    high: f64,
    points: usize,
) -> Option<f64> {
    if legs.is_empty() || spot <= 0.0 || low <= 0.0 || high <= low || points < 2 {
        return None;
    }
    let grid = linspace(low, high, points);
    let totals: Vec<f64> = grid
        .iter()
        .map(|&price| {
            let scale = price / spot;
            let dot: f64 = legs
                .iter()
                .map(|l| {
                    safe_gamma(l.forward * scale, l.strike, l.years, 0.0, l.iv) * (l.oi * l.sign)
                })
                .sum();
            dot * price * price
        })
        .collect();
    let mut best: Option<f64> = None;
    for i in 0..points - 1 {
        let (a, b) = (totals[i], totals[i + 1]);
        let crossing = if a == 0.0 {
            grid[i]
        } else if (a < 0.0) != (b < 0.0) && b != 0.0 {
            grid[i] + (grid[i + 1] - grid[i]) * (-a) / (b - a)
        } else {
            continue;
        };
        if best.is_none_or(|b| (crossing - spot).abs() < (b - spot).abs()) {
            best = Some(crossing);
        }
    }
    best
}

/// NumPy's `linspace(low, high, points)`: `low + i * step`, the last point
/// exactly `high`.
fn linspace(low: f64, high: f64, points: usize) -> Vec<f64> {
    let step = (high - low) / (points - 1) as f64;
    (0..points)
        .map(|i| {
            if i == points - 1 {
                high
            } else {
                i as f64 * step + low
            }
        })
        .collect()
}

/// Strikes a side needed to cover spot +/- `range_pct` percent, from the
/// strikes of a probe chain (web `_strike_count_for_window`). No window
/// ("All") is the widest; a chain with no readable strike step keeps the
/// default.
pub fn strike_count_for_window(strikes: &[f64], spot: f64, range_pct: Option<f64>) -> i64 {
    let Some(pct) = range_pct.filter(|p| *p != 0.0) else {
        return MAX_STRIKE_COUNT;
    };
    let mut sorted: Vec<f64> = strikes.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted.dedup();
    if sorted.len() < 2 {
        return DEFAULT_STRIKE_COUNT;
    }
    let step = sorted
        .windows(2)
        .map(|w| w[1] - w[0])
        .filter(|d| *d > 0.0)
        .fold(f64::INFINITY, f64::min);
    if !(step.is_finite() && step > 0.0) {
        return DEFAULT_STRIKE_COUNT;
    }
    let needed = (spot * pct / 100.0 / step).ceil() as i64 + 1;
    needed.clamp(5, MAX_STRIKE_COUNT)
}

/// The strike with the largest total, the lowest strike on a tie (Python's
/// `max` keeps the first); `None` when no strike's total is positive.
pub fn wall(strikes: &[f64], totals: &[f64]) -> Option<f64> {
    let mut best: Option<(f64, f64)> = None;
    for (k, v) in strikes.iter().zip(totals) {
        if best.is_none_or(|(_, b)| *v > b) {
            best = Some((*k, *v));
        }
    }
    best.filter(|(_, v)| *v > 0.0).map(|(k, _)| k)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPOT: f64 = 20_000.0;

    /// Web test_units_and_signs, on the formulas: worked by hand,
    /// 0.001 x 1e6 x 20000^2 x 1% / 1e7 = 400 crore of call GEX, and
    /// 0.5 x 1e6 x 20000 / 1e7 = 1000 crore of call DEX.
    #[test]
    fn units_and_signs() {
        assert!((gex(0.001, 1_000_000.0, SPOT, 1.0) - 400.0).abs() < 1e-9);
        assert!((gex(0.001, 500_000.0, SPOT, -1.0) + 200.0).abs() < 1e-9);
        assert!((dex(0.5, 1_000_000.0, SPOT) - 1000.0).abs() < 1e-9);
        assert!((dex(-0.5, 500_000.0, SPOT) + 500.0).abs() < 1e-9);
    }

    /// Black-76 gamma at r = 0, written out here rather than taken from
    /// the kernel the flip search uses.
    fn gamma_by_hand(f: f64, k: f64, t: f64, sigma: f64) -> f64 {
        let st = sigma * t.sqrt();
        let d1 = ((f / k).ln() + sigma * sigma * t / 2.0) / st;
        (-d1 * d1 / 2.0).exp() / (2.0 * std::f64::consts::PI).sqrt() / (f * st)
    }

    fn leg(strike: f64, oi: f64, sign: f64) -> FlipLeg {
        FlipLeg {
            strike,
            forward: SPOT,
            years: 3.0 / 365.0,
            iv: 0.12,
            oi,
            sign,
        }
    }

    /// Web test_gamma_flip_sits_between_put_and_call_concentrations:
    /// puts below 20000 and calls above, 1e6 OI each, so total GEX is
    /// negative under spot and positive over it.
    #[test]
    fn gamma_flip_sits_between_put_and_call_concentrations() {
        let legs: Vec<FlipLeg> = (19_500..20_550)
            .step_by(50)
            .filter(|k| *k != 20_000)
            .map(|k| {
                let k = f64::from(k);
                let sign = if k > SPOT { 1.0 } else { -1.0 };
                leg(k, 1_000_000.0, sign)
            })
            .collect();
        let flip = find_gamma_flip(&legs, SPOT, 19_500.0, 20_500.0, FLIP_GRID_POINTS).unwrap();
        assert!(19_900.0 < flip && flip < 20_100.0, "{flip}");
        // Checked by hand: the total at the flip, re-priced outside the
        // search, is a small fraction of the total either side of it.
        let total = |price: f64| -> f64 {
            legs.iter()
                .map(|l| {
                    gamma_by_hand(l.forward * price / SPOT, l.strike, l.years, l.iv) * l.oi * l.sign
                })
                .sum::<f64>()
                * price
                * price
        };
        assert!(total(flip - 25.0) < 0.0 && total(flip + 25.0) > 0.0);
        assert!(total(flip).abs() < total(flip + 25.0).abs() / 10.0);
    }

    /// Web test_no_flip_without_a_sign_change.
    #[test]
    fn no_flip_without_a_sign_change() {
        let legs: Vec<FlipLeg> = [19_900.0, 20_000.0, 20_100.0]
            .iter()
            .map(|k| leg(*k, 1000.0, 1.0))
            .collect();
        assert_eq!(
            find_gamma_flip(&legs, SPOT, 19_900.0, 20_100.0, FLIP_GRID_POINTS),
            None
        );
        // Nothing to search over.
        assert_eq!(find_gamma_flip(&[], SPOT, 19_900.0, 20_100.0, 241), None);
        assert_eq!(find_gamma_flip(&legs, SPOT, 20_000.0, 20_000.0, 241), None);
    }

    /// A grid point where the total is exactly zero counts as a crossing,
    /// and of several crossings the one nearest spot wins: a call and a put
    /// with the same strike, IV and OI cancel at every price, so each of
    /// 19000, 20000 and 21000 is one, and 20000 is nearest.
    #[test]
    fn crossing_nearest_spot_wins() {
        let legs = [leg(20_000.0, 1000.0, 1.0), leg(20_000.0, 1000.0, -1.0)];
        assert_eq!(
            find_gamma_flip(&legs, SPOT, 19_000.0, 21_000.0, 3),
            Some(20_000.0)
        );
    }

    /// Web test_window_widens_the_strike_count_from_the_probe: 10% of
    /// 20000 is 2000 points, 40 strikes of 50 plus one, capped at 40.
    #[test]
    fn strike_window() {
        let strikes: Vec<f64> = (-20..=20).map(|i| SPOT + 50.0 * f64::from(i)).collect();
        assert_eq!(strike_count_for_window(&strikes, SPOT, Some(10.0)), 40);
        // 2%: 400 points over a 50 step is 8, plus one.
        assert_eq!(strike_count_for_window(&strikes, SPOT, Some(2.0)), 9);
        // A narrow window is never under five a side.
        assert_eq!(strike_count_for_window(&strikes, 1000.0, Some(2.0)), 5);
        // "All" asks for the widest window.
        assert_eq!(strike_count_for_window(&strikes, SPOT, None), 40);
        // No step to read: the default.
        assert_eq!(strike_count_for_window(&[SPOT], SPOT, Some(5.0)), 20);
        assert_eq!(strike_count_for_window(&[SPOT, SPOT], SPOT, Some(5.0)), 20);
    }

    /// Web test_walls_and_union_of_strikes, on the wall rule.
    #[test]
    fn walls() {
        let strikes = [19_900.0, 20_000.0, 20_100.0];
        assert_eq!(wall(&strikes, &[1.0, 5.0, 3.0]), Some(20_000.0));
        // A tie keeps the lowest strike.
        assert_eq!(wall(&strikes, &[5.0, 5.0, 3.0]), Some(19_900.0));
        // Nothing positive: no wall.
        assert_eq!(wall(&strikes, &[0.0, 0.0, 0.0]), None);
        assert_eq!(wall(&[], &[]), None);
    }
}
