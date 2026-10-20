# OptionChain

Get the option chain of an underlying for one expiry: quotes for the call and
put at every strike and, if you ask, implied volatility and Greeks for every
leg. The answer is flat: there is no `data` wrapper.

## Endpoint URL

```http
POST http://127.0.0.1:5000/api/v1/optionchain
```

## Sample API Request

```json
{
  "apikey": "<APIKEY>",
  "underlying": "NIFTY",
  "exchange": "NSE_INDEX",
  "expiry_date": "06OCT26",
  "strike_count": 3,
  "with_greeks": true,
  "interest_rate": 6.5
}
```

## Sample cURL Request

```bash
curl -X POST http://127.0.0.1:5000/api/v1/optionchain \
  -H 'Content-Type: application/json' \
  -d '{
  "apikey": "<APIKEY>",
  "underlying": "NIFTY",
  "exchange": "NSE_INDEX",
  "expiry_date": "06OCT26",
  "strike_count": 3,
  "with_greeks": true,
  "interest_rate": 6.5
}'
```

## Sample API Response

Recorded with the market closed (bid, ask and volume are 0) and trimmed to two
of the seven strikes. Without `with_greeks` the five Greek fields are absent
and `forward_price` is `null`.

```json
{
  "status": "success",
  "underlying": "NIFTY",
  "underlying_symbol": "NIFTY",
  "underlying_exchange": "NSE_INDEX",
  "underlying_ltp": 22421.95,
  "underlying_prev_close": 22620.45,
  "expiry_date": "06OCT26",
  "expiry_ts": 1791280800,
  "server_ts": 1791000272,
  "atm_strike": 22400.0,
  "quotes_included": true,
  "greeks_included": true,
  "forward_price": 22453.35,
  "chain": [
    {
      "strike": 22250.0,
      "ce": {
        "symbol": "NIFTY06OCT2622250CE",
        "label": "ITM3",
        "ltp": 259.2,
        "bid": 0,
        "ask": 0,
        "bid_qty": 0,
        "ask_qty": 0,
        "open": 381.45,
        "high": 406.75,
        "low": 157.15,
        "prev_close": 426.05,
        "volume": 0,
        "oi": 474565,
        "lotsize": 65,
        "tick_size": 0.05,
        "implied_volatility": 15.91,
        "delta": 0.7299,
        "gamma": 0.00098,
        "theta": -17.0901,
        "vega": 6.994
      },
      "pe": {
        "symbol": "NIFTY06OCT2622250PE",
        "label": "OTM3",
        "ltp": 55.85,
        "bid": 0,
        "ask": 0,
        "bid_qty": 0,
        "ask_qty": 0,
        "open": 40,
        "high": 154.65,
        "low": 24.9,
        "prev_close": 28.15,
        "volume": 0,
        "oi": 1942785,
        "lotsize": 65,
        "tick_size": 0.05,
        "implied_volatility": 15.89,
        "delta": -0.2693,
        "gamma": 0.000981,
        "theta": -17.1015,
        "vega": 6.9913
      }
    },
    {
      "strike": 22300.0,
      "ce": {
        "symbol": "NIFTY06OCT2622300CE",
        "label": "ITM2",
        "ltp": 220.8,
        "bid": 0,
        "ask": 0,
        "bid_qty": 0,
        "ask_qty": 0,
        "open": 367.55,
        "high": 367.6,
        "low": 132.6,
        "prev_close": 382.5,
        "volume": 0,
        "oi": 3235180,
        "lotsize": 65,
        "tick_size": 0.05,
        "implied_volatility": 15.46,
        "delta": 0.6831,
        "gamma": 0.001087,
        "theta": -17.8929,
        "vega": 7.5342
      },
      "pe": {
        "symbol": "NIFTY06OCT2622300PE",
        "label": "OTM2",
        "ltp": 68.3,
        "bid": 0,
        "ask": 0,
        "bid_qty": 0,
        "ask_qty": 0,
        "open": 65.55,
        "high": 180.6,
        "low": 31.05,
        "prev_close": 34.65,
        "volume": 0,
        "oi": 8348925,
        "lotsize": 65,
        "tick_size": 0.05,
        "implied_volatility": 15.56,
        "delta": -0.3174,
        "gamma": 0.001081,
        "theta": -18.063,
        "vega": 7.5449
      }
    }
  ]
}
```

## Request Body

| Parameter | Description | Mandatory/Optional | Default Value |
|-----------|-------------|-------------------|---------------|
| apikey | Your OpenAlgo API key | Mandatory | - |
| underlying | Underlying, for example `NIFTY`, `BANKNIFTY`, `SENSEX`, `CRUDEOIL` | Mandatory | - |
| exchange | Exchange of the underlying, for example `NSE_INDEX`, `BSE_INDEX`, `NSE`, `MCX`, `CDS`, `CRYPTO` | Mandatory | - |
| expiry_date | Expiry in `DDMMMYY` form, for example `06OCT26` (no dashes) | Mandatory | - |
| strike_count | Strikes on each side of ATM, 1 to 100 | Optional | Every strike of the expiry |
| with_greeks | Add implied volatility and Greeks to every leg | Optional | `false` |
| interest_rate | Annual risk-free rate in percent, 0 to 100, used for the Greeks | Optional | 0 |

[Expiry](../symbol-services/expiry.md) returns `DD-MMM-YY` (`06-OCT-26`).
Remove the dashes before sending it here.

## Response Fields

| Field | Type | Description |
|-------|------|-------------|
| status | string | "success" or "error" |
| underlying | string | The underlying |
| underlying_symbol | string | The symbol quoted for the underlying price: the index or stock, the near-month future on MCX, CDS, BCD, NCO and NCDEX, or the perpetual future on CRYPTO |
| underlying_exchange | string | Where it was quoted |
| underlying_ltp | number | The underlying's last price |
| underlying_prev_close | number | The underlying's previous close |
| expiry_date | string | The expiry, `DDMMMYY` |
| expiry_ts | number or null | The expiry instant in epoch **seconds**, at the exchange's cut-off time; `null` if it cannot be worked out |
| server_ts | number | The time of the answer in epoch seconds |
| atm_strike | number | The listed strike nearest the underlying's price |
| quotes_included | boolean | Always `true` |
| greeks_included | boolean | Whether Greeks were asked for |
| forward_price | number or null | The forward the Greeks were priced against; `null` without `with_greeks` |
| chain | array | One row per strike, lowest first |

### Chain Array Fields

| Field | Type | Description |
|-------|------|-------------|
| strike | number | Strike price |
| ce | object or null | The call, `null` when that contract is not in the master contract |
| pe | object or null | The put, `null` when that contract is not in the master contract |

### Option Data Fields

| Field | Type | Description |
|-------|------|-------------|
| symbol | string | Option symbol |
| label | string | `ATM`, `ITM1`, `ITM2` ... or `OTM1`, `OTM2` ... |
| ltp | number | Last traded price |
| bid | number | Best bid |
| ask | number | Best ask |
| bid_qty | number | Quantity at the best bid |
| ask_qty | number | Quantity at the best ask |
| open | number | Day's open |
| high | number | Day's high |
| low | number | Day's low |
| prev_close | number | Previous close |
| volume | number | Volume traded today |
| oi | number | Open interest. 0 when the broker sent none: see below. |
| lotsize | number | Lot size |
| tick_size | number | Minimum price step |

### Greek Fields

Present on a leg only when `with_greeks` is set, the leg has a price above 0
and the contract has not expired.

| Field | Type | Description |
|-------|------|-------------|
| implied_volatility | number | Implied volatility in percent (`15.91` is 15.91%) |
| delta | number | Change in option price for a 1 point move of the underlying |
| gamma | number | Change in delta for a 1 point move of the underlying |
| theta | number | Change in option price per calendar day |
| vega | number | Change in option price for a 1% change in volatility |

## Greeks

- Black-76, priced against the forward from the ATM call and put
  (`forward = ATM strike + call price - put price`). When either ATM leg has
  no price, the underlying's LTP is used instead.
- Time to expiry runs to the exchange's cut-off: 15:30 IST for NFO, BFO and
  most exchanges, 12:30 for CDS, 23:30 for MCX. `expiry_ts` is that instant.
- A leg with no time value (priced at or below its intrinsic value), or whose
  volatility cannot be solved, gets `implied_volatility: 0`, a delta of `1`
  (call) or `-1` (put) when it is in the money and `0` when it is not, and the
  other Greeks `0`, rather than an error.
- The Greeks come from the quotes already fetched, so `with_greeks` costs no
  extra broker call. Units match [OptionGreeks](./optiongreeks.md).

## Open interest a broker did not send

When the broker answers a leg without open interest, or does not answer a
leg at all, its `oi` here is `0`, exactly as OpenAlgo web answers. The API
cannot tell such a leg from one whose open interest really is 0.

Inside the desktop, the option tools built on this chain know which legs came
back without open interest. OI Tracker, Max Pain, GEX, Gamma Density and the
OI profile refuse to show a figure that would count a missing value as zero
and ask you to try again. Strike Profile is the exception: like OpenAlgo web,
it totals a missing value as zero.

With Fyers, the chain is read from Fyers' own option-chain call for the
requested expiry, and a leg whose open interest Fyers sent as missing, empty
or not a number is treated as missing, not as a measured zero.

## Errors

| HTTP | Body | Cause and fix |
|------|------|---------------|
| 400 | `{"status": "error", "message": "Validation error", "errors": {"strike_count": ["Must be greater than or equal to 1 and less than or equal to 100."]}}` | A field is missing or not valid. Field errors are under `errors`. |
| 403 | `Invalid openalgo apikey` | The key is wrong, or no broker is logged in. |
| 404 | `No strikes found for NIFTY expiring 01JAN20. Please check expiry date or update master contract.` | No contracts for that expiry: a dashed or wrong date, an expiry that has passed, or an old master contract. |
| 404 | `No unexpired futures found for CRUDEOIL on MCX. MCX options are priced against the near-month future, which this product does not currently have.` | No near-month future to price against. |
| 404 | `No valid option symbols found for the given parameters` | The strikes have no listed contracts. |
| 400 or 500 | `Failed to fetch LTP for NIFTY: <reason>` | The underlying's quote failed. |
| 500 | `An error occurred while fetching option chain: <reason>` | The broker refused the quotes or could not be reached. |

## Notes

- Without `strike_count` the whole chain of the expiry is returned.
- Calls below ATM and puts above ATM are in the money.
- Quotes come from your broker, also in analyzer mode.
- If `underlying` is a future that names its expiry (`NIFTY27OCT26FUT`), that
  expiry is used and `expiry_date` is ignored.

## Related

- [OptionGreeks](./optiongreeks.md) - Greeks for one option
- [MultiOptionGreeks](./multioptiongreeks.md) - Greeks for up to 50 options. For a whole chain prefer this endpoint with `with_greeks`.
- [SyntheticFuture](./syntheticfuture.md) - the put-call parity forward on its own

---

**Back to**: [API Documentation](../README.md)
