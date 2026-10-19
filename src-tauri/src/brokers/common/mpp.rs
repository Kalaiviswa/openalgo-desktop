//! Market Price Protection (web `utils/mpp_slab.py`).
//!
//! Where a broker only accepts LIMIT (Kite GTT), a MARKET request is sent as
//! a LIMIT at LTP plus (BUY) or minus (SELL) a slab percentage, rounded to
//! the instrument's tick size. A price that is not finite matches no slab
//! and is refused (web #2052), so a NaN or infinite quote never becomes a
//! limit price.

use super::mapping::Action;
use crate::error::{AppError, Result};

/// Python `round()` (round half to even), so prices match the web exactly.
pub fn py_round(v: f64, ndigits: i32) -> f64 {
    let m = 10f64.powi(ndigits);
    let x = v * m;
    let r = x.round();
    let r = if (x - x.trunc()).abs() == 0.5 {
        // Tie: pick the even neighbour.
        let down = x.trunc();
        if (down as i64) % 2 == 0 {
            down
        } else {
            down + x.signum()
        }
    } else {
        r
    };
    r / m
}

/// `EQ`, `FUT`, `CE` or `PE` from a symbol's suffix.
pub fn instrument_type_from_symbol(symbol: &str) -> &'static str {
    let s = symbol.to_ascii_uppercase();
    if s.ends_with("CE") {
        "CE"
    } else if s.ends_with("PE") {
        "PE"
    } else if s.ends_with("FUT") {
        "FUT"
    } else {
        "EQ"
    }
}

/// Protection percentage for a price (web `get_mpp_percentage`). A NaN or
/// infinite price is refused as a validation error: the order is not sent.
pub fn mpp_percentage(price: f64, instrument_type: &str) -> Result<f64> {
    if !price.is_finite() {
        tracing::warn!(
            "Market price protection refused a price that is not finite: {}",
            price
        );
        return Err(AppError::Validation(
            "The market price for this order is not a valid number, so price protection could not be applied and the order was not sent. Try again in a moment."
                .into(),
        ));
    }
    Ok(if matches!(instrument_type, "CE" | "PE") {
        if price < 10.0 {
            5.0
        } else if price < 100.0 {
            3.0
        } else if price < 500.0 {
            2.0
        } else {
            1.0
        }
    } else if price < 100.0 {
        2.0
    } else if price < 500.0 {
        1.0
    } else {
        0.5
    })
}

pub fn round_to_tick(price: f64, tick_size: Option<f64>) -> f64 {
    match tick_size {
        Some(t) if t > 0.0 => py_round(py_round(price / t, 0) * t, 2),
        _ => py_round(price, 2),
    }
}

/// LIMIT price that protects a MARKET order (web
/// `calculate_protected_price`); refused like [`mpp_percentage`].
pub fn protected_price(
    price: f64,
    action: Action,
    instrument_type: &str,
    tick_size: Option<f64>,
) -> Result<f64> {
    let m = mpp_percentage(price, instrument_type)? / 100.0;
    let p = match action {
        Action::Buy => price * (1.0 + m),
        Action::Sell => price * (1.0 - m),
    };
    Ok(round_to_tick(p, tick_size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_rounding() {
        assert_eq!(py_round(2.5, 0), 2.0);
        assert_eq!(py_round(3.5, 0), 4.0);
        assert_eq!(py_round(102.0111, 2), 102.01);
    }

    #[test]
    fn slabs_match_web_examples() {
        assert_eq!(mpp_percentage(50.0, "EQ").unwrap(), 2.0);
        assert_eq!(mpp_percentage(50.0, "CE").unwrap(), 3.0);
        assert_eq!(mpp_percentage(5.0, "PE").unwrap(), 5.0);
        assert_eq!(mpp_percentage(600.0, "FUT").unwrap(), 0.5);
        assert_eq!(
            protected_price(100.0, Action::Buy, "EQ", None).unwrap(),
            101.0
        );
        assert_eq!(
            protected_price(5.0, Action::Buy, "CE", Some(0.05)).unwrap(),
            5.25
        );
        assert_eq!(
            protected_price(1000.0, Action::Sell, "EQ", Some(0.05)).unwrap(),
            995.0
        );
        assert_eq!(round_to_tick(102.0111, Some(0.05)), 102.0);
        assert_eq!(instrument_type_from_symbol("NIFTY24DECFUT"), "FUT");
        assert_eq!(instrument_type_from_symbol("SBIN"), "EQ");
        // Same suffix rule as the web: an equity ending in CE reads as CE.
        assert_eq!(instrument_type_from_symbol("RELIANCE"), "CE");
    }

    /// Web test/test_mpp_slab_validation.py
    /// `TestGetMppPercentageSlabs::test_boundary_slabs_unchanged`.
    #[test]
    fn boundary_slabs_match_the_web() {
        for (price, itype, pct) in [
            (50.0, "EQ", 2.0),
            (99.99, "EQ", 2.0),
            (100.0, "EQ", 1.0),
            (499.99, "EQ", 1.0),
            (500.0, "EQ", 0.5),
            (10000.0, "EQ", 0.5),
            (50.0, "FUT", 2.0),
            (5.0, "PE", 5.0),
            (9.99, "CE", 5.0),
            (10.0, "CE", 3.0),
            (99.99, "CE", 3.0),
            (100.0, "CE", 2.0),
            (500.0, "CE", 1.0),
        ] {
            assert_eq!(
                mpp_percentage(price, itype).unwrap(),
                pct,
                "{} {}",
                price,
                itype
            );
        }
    }

    /// Web #2052 (test/test_mpp_slab_validation.py
    /// `TestGetMppPercentageValidation`): a NaN or infinite price matches no
    /// slab and is refused, never turned into a limit price. (The web's
    /// non-numeric cases cannot be written here: the price is an f64.)
    #[test]
    fn a_price_that_is_not_finite_is_refused() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for itype in ["EQ", "CE"] {
                let e = mpp_percentage(bad, itype).unwrap_err();
                assert!(matches!(e, AppError::Validation(_)), "{:?}", e);
                assert!(e.client_message().contains("not a valid number"));
            }
            for action in [Action::Buy, Action::Sell] {
                assert!(protected_price(bad, action, "EQ", Some(0.05)).is_err());
            }
        }
    }
}
