//! The scripted broker of CI's native end-to-end test build (the
//! `e2e-broker` feature, `src/e2e.rs`; `scripts/ci/native_e2e.py`): a
//! [`MockBroker`] in the place of the Dhan (Sandbox) adapter, which signs
//! in from its saved keys in one action, so the run needs no broker site.
//!
//! It answers with a small NIFTY market: the index, a future and options
//! at 13 strikes around [`SPOT`] for one weekly expiry a week or more after
//! the day the app starts, each with a quote. That is enough for the Option
//! Chain page and its order form, and for a sandbox order to fill.
//!
//! Every call it receives is counted ([`counts`]: every call, and the order
//! calls among them: place, modify, cancel, close, GTT), so the E2E run can
//! show by a count that a sandbox order never reached the broker; the test
//! build reads the counts out at `GET /e2e/scripted-broker`
//! (`server::routes::e2e`). Each call is also logged with [`MARKER`], an
//! order call as an error. Never part of a release: the feature does not
//! build without `OPENALGO_E2E_BUILD=1` outside a debug build, and
//! `scripts/ci/check_no_e2e_build.py` looks for [`MARKER`] in every shipped
//! binary.

use super::common::symbols::{SymToken, SymbolResolver};
use super::mock::{MockBroker, MockCall};
use super::types::Quote;
use super::{Broker, BrokerRegistry};
use chrono::{Datelike, Duration, NaiveDate, Weekday};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// The catalogue broker whose adapter the scripted broker replaces.
pub const SLOT: &str = "dhan_sandbox";
/// In every log line of the scripted broker, and what the shipped-binary
/// check looks for.
pub const MARKER: &str = "OPENALGO_E2E_SCRIPTED_BROKER";
/// The index level the market is built around.
pub const SPOT: f64 = 25_010.0;
/// NIFTY's lot size.
pub const LOT: i32 = 65;
/// The strikes listed: 24,700 to 25,300 in steps of 50.
const STRIKES: std::ops::RangeInclusive<i64> = 494..=506;
const STRIKE_STEP: i64 = 50;

/// Calls received by scripted brokers in this process (the test build has
/// one), and the order calls among them.
static CALLS: AtomicU64 = AtomicU64::new(0);
static ORDER_CALLS: AtomicU64 = AtomicU64::new(0);

/// What the scripted broker has received so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Counts {
    /// Every call: sign-in, master contract, quotes, books, orders.
    pub calls: u64,
    /// Place, modify, cancel, cancel all, close all and GTT calls.
    pub order_calls: u64,
}

/// The counts so far.
pub fn counts() -> Counts {
    Counts {
        calls: CALLS.load(Ordering::SeqCst),
        order_calls: ORDER_CALLS.load(Ordering::SeqCst),
    }
}

/// `real`'s adapters, the very same ones, with the scripted broker (also
/// returned, for tests that read its calls) in [`SLOT`].
pub fn registry(real: &BrokerRegistry, today: NaiveDate) -> (BrokerRegistry, Arc<MockBroker>) {
    let symbols = real.symbols();
    let scripted = Arc::new(broker(symbols.clone(), today));
    let mut brokers: Vec<Arc<dyn Broker>> =
        real.list().into_iter().filter(|b| b.id() != SLOT).collect();
    brokers.push(scripted.clone());
    (BrokerRegistry::with_symbols(symbols, brokers), scripted)
}

/// The scripted broker on `symbols`, its expiry set from `today`.
fn broker(symbols: SymbolResolver, today: NaiveDate) -> MockBroker {
    let mock = MockBroker::with_symbols(SLOT, symbols);
    *mock.saved_keys_sign_in.lock() = true;
    *mock.on_call.lock() = Some(announce);
    let rows = master(expiry_after(today));
    for row in &rows {
        mock.set_quote(quote(row));
    }
    *mock.master.lock() = Some(Ok(rows));
    mock
}

/// The first Tuesday (NIFTY's weekly expiry day) a week or more after
/// `today`, so the contracts are live whenever the run happens.
pub fn expiry_after(today: NaiveDate) -> NaiveDate {
    let mut d = today + Duration::days(7);
    while d.weekday() != Weekday::Tue {
        d += Duration::days(1);
    }
    d
}

/// The OpenAlgo symbol of the NIFTY option at `strike` for `expiry`.
pub fn option_symbol(expiry: NaiveDate, strike: i64, kind: &str) -> String {
    format!("NIFTY{}{}{}", code(expiry), strike, kind)
}

/// `DDMMMYY`, as in option symbols.
fn code(expiry: NaiveDate) -> String {
    expiry.format("%d%b%y").to_string().to_uppercase()
}

fn row(
    symbol: String,
    exchange: &str,
    name: &str,
    expiry: &str,
    strike: f64,
    kind: &str,
) -> SymToken {
    SymToken {
        brsymbol: symbol.clone(),
        token: format!("{}:{}", exchange, symbol),
        symbol,
        name: name.into(),
        exchange: exchange.into(),
        brexchange: exchange.into(),
        expiry: expiry.into(),
        strike,
        lot_size: if exchange == "NSE_INDEX" { 1 } else { LOT },
        instrument_type: kind.into(),
        tick_size: 0.05,
    }
}

/// The index, its future and the options of `expiry`.
fn master(expiry: NaiveDate) -> Vec<SymToken> {
    let listed = expiry.format("%d-%b-%y").to_string().to_uppercase();
    let mut rows = vec![
        row("NIFTY".into(), "NSE_INDEX", "NIFTY 50", "", 0.0, "INDEX"),
        row(
            format!("NIFTY{}FUT", code(expiry)),
            "NFO",
            "NIFTY",
            &listed,
            0.0,
            "FUT",
        ),
    ];
    for strike in STRIKES.map(|s| s * STRIKE_STEP) {
        for kind in ["CE", "PE"] {
            rows.push(row(
                option_symbol(expiry, strike, kind),
                "NFO",
                "NIFTY",
                &listed,
                strike as f64,
                kind,
            ));
        }
    }
    rows
}

/// A quote for `row`: options at their intrinsic value plus time value,
/// with a bid and an ask a tick either side.
fn quote(row: &SymToken) -> Quote {
    let ltp = match row.instrument_type.as_str() {
        "CE" => (SPOT - row.strike).max(0.0) + 120.0,
        "PE" => (row.strike - SPOT).max(0.0) + 110.0,
        "FUT" => SPOT + 50.0,
        _ => SPOT,
    };
    let tradable = row.exchange != "NSE_INDEX";
    Quote {
        symbol: row.symbol.clone(),
        exchange: row.exchange.clone(),
        ltp,
        open: ltp,
        high: ltp,
        low: ltp,
        close: ltp,
        bid: if tradable { ltp - row.tick_size } else { 0.0 },
        ask: if tradable { ltp + row.tick_size } else { 0.0 },
        bid_qty: if tradable { 650 } else { 0 },
        ask_qty: if tradable { 650 } else { 0 },
        volume: if tradable { 100_000 } else { 0 },
        oi: if tradable { 50_000 } else { 0 },
        ..Default::default()
    }
}

/// Whether `call` would place, change or cancel an order at the broker.
fn is_order(call: &MockCall) -> bool {
    matches!(
        call,
        MockCall::PlaceOrder(_)
            | MockCall::ModifyOrder(_)
            | MockCall::CancelOrder(_)
            | MockCall::CancelAll
            | MockCall::CloseAll
            | MockCall::PlaceGtt
            | MockCall::ModifyGtt(_)
            | MockCall::CancelGtt(_)
    )
}

/// Count a call and log it by its kind (never its contents).
fn announce(call: &MockCall) {
    CALLS.fetch_add(1, Ordering::SeqCst);
    let shown = format!("{:?}", call);
    let kind = shown.split('(').next().unwrap_or("");
    if is_order(call) {
        ORDER_CALLS.fetch_add(1, Ordering::SeqCst);
        tracing::error!(
            "{}: an order reached the scripted broker ({})",
            MARKER,
            kind
        );
    } else {
        tracing::info!("{}: scripted broker call {}", MARKER, kind);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn the_expiry_is_a_tuesday_a_week_or_more_ahead() {
        // Monday, Tuesday and Wednesday.
        assert_eq!(expiry_after(day(2026, 10, 19)), day(2026, 10, 27));
        assert_eq!(expiry_after(day(2026, 10, 20)), day(2026, 10, 27));
        assert_eq!(expiry_after(day(2026, 10, 21)), day(2026, 11, 3));
        assert_eq!(
            option_symbol(day(2026, 11, 3), 25_000, "CE"),
            "NIFTY03NOV2625000CE"
        );
    }

    #[test]
    fn the_market_has_the_index_a_future_and_both_sides_of_every_strike() {
        let rows = master(day(2026, 10, 27));
        assert_eq!(rows.len(), 2 + 13 * 2);
        assert_eq!(rows[0].symbol, "NIFTY");
        assert_eq!(rows[1].symbol, "NIFTY27OCT26FUT");
        assert_eq!(rows[1].expiry, "27-OCT-26");
        let atm = rows
            .iter()
            .find(|r| r.symbol == "NIFTY27OCT2625000CE")
            .unwrap();
        assert_eq!(
            (atm.strike, atm.lot_size, atm.expiry.as_str()),
            (25_000.0, LOT, "27-OCT-26")
        );
        let q = quote(atm);
        assert_eq!(q.ltp, 130.0);
        assert!(q.bid < q.ltp && q.ltp < q.ask && q.ask - q.bid < 0.11);
        assert_eq!(quote(&rows[0]).ltp, SPOT);
    }

    #[test]
    fn every_call_is_counted_and_order_calls_apart() {
        assert!(is_order(&MockCall::CancelAll));
        assert!(is_order(&MockCall::CancelOrder("1".into())));
        assert!(!is_order(&MockCall::Authenticate));
        assert!(!is_order(&MockCall::Funds));
        // Through the broker itself, as the app calls it. No other test in
        // this binary drives a scripted broker, so the deltas are this
        // test's own.
        let before = counts();
        let (reg, _) = registry(&BrokerRegistry::new(), day(2026, 10, 19));
        let broker = reg.get(SLOT).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let auth = crate::brokers::types::AuthToken::new("mock-access-token");
        rt.block_on(async {
            broker.get_funds(&auth).await.unwrap();
            broker.cancel_order(&auth, "1").await.unwrap();
        });
        let after = counts();
        assert_eq!(after.calls - before.calls, 2);
        assert_eq!(after.order_calls - before.order_calls, 1);
    }

    #[test]
    fn the_scripted_broker_takes_its_slot_among_the_production_adapters() {
        let real = BrokerRegistry::new();
        let (reg, scripted) = registry(&real, day(2026, 10, 19));
        assert_eq!(reg.ids(), real.ids());
        for id in real.ids() {
            let (got, was) = (reg.get(&id).unwrap(), real.get(&id).unwrap());
            assert_eq!(Arc::ptr_eq(&got, &was), id != SLOT, "{}", id);
        }
        let slot = reg.get(SLOT).unwrap();
        assert_eq!(slot.name(), "Mock");
        assert!(scripted.master.lock().is_some());
        assert!(*scripted.saved_keys_sign_in.lock());
    }
}
