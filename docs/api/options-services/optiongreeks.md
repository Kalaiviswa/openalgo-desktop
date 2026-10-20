# OptionGreeks

Work out implied volatility and the Greeks (delta, gamma, theta, vega, rho)
of one option. The answer is flat: there is no `data` wrapper.

## Endpoint URL

```http
POST http://127.0.0.1:5000/api/v1/optiongreeks
```

## Sample API Request

```json
{
  "apikey": "<APIKEY>",
  "symbol": "NIFTY06OCT2622400CE",
  "exchange": "NFO",
  "interest_rate": 6.5,
  "underlying_symbol": "NIFTY",
  "underlying_exchange": "NSE_INDEX"
}
```

## Sample cURL Request

```bash
curl -X POST http://127.0.0.1:5000/api/v1/optiongreeks \
  -H 'Content-Type: application/json' \
  -d '{
  "apikey": "<APIKEY>",
  "symbol": "NIFTY06OCT2622400CE",
  "exchange": "NFO",
  "interest_rate": 6.5,
  "underlying_symbol": "NIFTY",
  "underlying_exchange": "NSE_INDEX"
}'
```

## Sample API Response

```json
{
  "status": "success",
  "symbol": "NIFTY06OCT2622400CE",
  "exchange": "NFO",
  "underlying": "NIFTY",
  "strike": 22400.0,
  "option_type": "CE",
  "expiry_date": "06-Oct-2026",
  "days_to_expiry": 3.2468,
  "spot_price": 22453.35,
  "option_price": 156.95,
  "interest_rate": 6.5,
  "implied_volatility": 15.24,
  "greeks": {
    "delta": 0.5682,
    "gamma": 0.001217,
    "theta": -19.4964,
    "vega": 8.3186,
    "rho": -0.013961
  }
}
```

`expiry_date` here is `DD-Mon-YYYY` (`06-Oct-2026`), a different form from
the `DDMMMYY` used in requests and the `DD-MMM-YY` of [Expiry](../symbol-services/expiry.md).

## Request Body

| Parameter | Description | Mandatory/Optional | Default Value |
|-----------|-------------|-------------------|---------------|
| apikey | Your OpenAlgo API key | Mandatory | - |
| symbol | Option symbol, for example `NIFTY06OCT2622400CE` | Mandatory | - |
| exchange | `NFO`, `BFO`, `MCX`, `CDS`, `NCO`, `BCD`, `NCDEX` or `CRYPTO` | Mandatory | - |
| interest_rate | Annual risk-free rate in percent, 0 to 100 | Optional | 0 |
| forward_price | Price to use as the forward, skipping the lookup below | Optional | - |
| underlying_symbol | Symbol to quote for the underlying price | Optional | From the option symbol |
| underlying_exchange | Exchange of `underlying_symbol` | Optional | See below |
| expiry_time | Expiry time `HH:MM` (IST) instead of the exchange's cut-off | Optional | Exchange cut-off |

## Response Fields

| Field | Type | Description |
|-------|------|-------------|
| status | string | "success" or "error" |
| symbol | string | Option symbol |
| exchange | string | Exchange |
| underlying | string | Underlying read from the symbol |
| strike | number | Strike price |
| option_type | string | `CE` or `PE` |
| expiry_date | string | Expiry, `DD-Mon-YYYY` |
| days_to_expiry | number | Days to the expiry cut-off, with fractions |
| spot_price | number | The forward the option was priced against (see below) |
| option_price | number | The option's last price |
| interest_rate | number | The rate used; `0` when none was sent |
| implied_volatility | number | Implied volatility in percent |
| greeks | object | The Greeks |

### Greeks Object Fields

| Field | Type | Description |
|-------|------|-------------|
| delta | number | Change in option price for a 1 point move of the underlying |
| gamma | number | Change in delta for a 1 point move of the underlying |
| theta | number | Change in option price per calendar day (negative for a buyer) |
| vega | number | Change in option price for a 1% change in volatility |
| rho | number | Change in option price for a 1% change in the interest rate |

### When there is no time value

An option priced at or below its intrinsic value (deep in the money) cannot
have an implied volatility. The answer is still a success, with
`implied_volatility: 0`, a delta of `1` (call) or `-1` (put), the other Greeks
`0`, and three more fields:

| Field | Description |
|-------|-------------|
| intrinsic_value | The option's intrinsic value |
| time_value | Price minus intrinsic value, never below 0 |
| note | `Deep ITM option with no time value - theoretical Greeks returned`, or `IV calculation not possible - theoretical deep ITM Greeks returned` |

## How the price is worked out

- The model is Black-76, priced against a forward.
- **The forward.** With `forward_price`, that number. Otherwise, when
  `underlying_symbol` is not sent or is the option's own underlying, the
  synthetic future of the same expiry (ATM strike + call - put, as in
  [SyntheticFuture](./syntheticfuture.md)); if that cannot be worked out, the
  underlying's LTP. With another `underlying_symbol` (a future, for example),
  that symbol's LTP. `spot_price` in the answer is the forward used.
- **The underlying's exchange**, when not sent: `NSE_INDEX` for NIFTY,
  BANKNIFTY, FINNIFTY, MIDCPNIFTY, NIFTYNXT50, NIFTYIT, NIFTYPHARMA and
  NIFTYBANK;
  `BSE_INDEX` for SENSEX, BANKEX and SENSEX50; `CDS` for currency pairs and on
  CDS; `MCX` for commodities and on MCX; `CRYPTO` on CRYPTO; otherwise `NSE`.
- **Time to expiry** runs to the exchange's cut-off on the expiry day: 15:30
  IST for NFO, BFO and most exchanges, 12:30 for CDS, 23:30 for MCX, unless you
  send `expiry_time`.

## Errors

| HTTP | Body | Cause and fix |
|------|------|---------------|
| 400 | `{"status": "error", "message": "Validation failed", "errors": {"exchange": ["Must be one of: NFO, BFO, MCX, CDS, NCO, BCD, NCDEX, CRYPTO."]}}` | A field is missing or not valid. Field errors are under `errors`. |
| 400 | `Failed to fetch option price: Symbol 'NIFTY01JAN2010000CE' not found for exchange 'NFO'. ...` | The option is not in the master contract. |
| 400 | `Option has expired on 06-Oct-2026` | The expiry cut-off has passed. |
| 401 | `Invalid openalgo apikey` | The key is wrong, or no broker is logged in. This endpoint answers 401, not 403, as on OpenAlgo web. |
| 404 | `Option LTP not available` | The option's last price is 0. |
| 404 | `Underlying LTP not available` | The underlying's last price is 0. |
| 400 or 500 | `Failed to fetch underlying price: <reason>` | The underlying's quote failed; check `underlying_symbol` and `underlying_exchange`. |
| 500 | `Failed to calculate Implied Volatility: <reason>` | The volatility could not be solved for a reason other than missing time value. |
| 500 | `Failed to get option Greeks: Failed to parse option symbol ...` | The symbol is not an option symbol (`<UNDERLYING><DDMMMYY><STRIKE><CE/PE>`). |
| 500 | `Failed to get option Greeks: Invalid expiry_time format: ...` | `expiry_time` is not `HH:MM`. |

## Notes

- Prices come from your broker, also in analyzer mode.
- Underlyings with digits, `&` or `-` in their name (`M&M`, `BAJAJ-AUTO`,
  `360ONE`) are read correctly from the option symbol.
- For many options at once use [MultiOptionGreeks](./multioptiongreeks.md); for
  a whole chain use [OptionChain](./optionchain.md) with `with_greeks`.

## Use Cases

- **Position sizing**: delta as a hedge ratio
- **Risk management**: watch gamma
- **Time decay**: track theta
- **Volatility trading**: watch vega and implied volatility

---

**Back to**: [API Documentation](../README.md)
