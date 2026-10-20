# OptionSymbol

Find the option contract for an underlying, expiry, offset (ATM, ITM or OTM)
and option type. The strike is worked out from the underlying's current price.
The answer is flat: there is no `data` wrapper.

## Endpoint URL

```http
POST http://127.0.0.1:5000/api/v1/optionsymbol
```

## Sample API Request (ATM Option)

```json
{
  "apikey": "<APIKEY>",
  "underlying": "NIFTY",
  "exchange": "NSE_INDEX",
  "expiry_date": "06OCT26",
  "offset": "ATM",
  "option_type": "CE"
}
```

## Sample cURL Request

```bash
curl -X POST http://127.0.0.1:5000/api/v1/optionsymbol \
  -H 'Content-Type: application/json' \
  -d '{
  "apikey": "<APIKEY>",
  "underlying": "NIFTY",
  "exchange": "NSE_INDEX",
  "expiry_date": "06OCT26",
  "offset": "ATM",
  "option_type": "CE"
}'
```

## Sample API Response (ATM Option)

```json
{
  "status": "success",
  "symbol": "NIFTY06OCT2622400CE",
  "exchange": "NFO",
  "lotsize": 65,
  "tick_size": 0.05,
  "freeze_qty": 1800,
  "underlying_ltp": 22421.95
}
```

## Sample API Request (OTM Put)

```json
{
  "apikey": "<APIKEY>",
  "underlying": "NIFTY",
  "exchange": "NSE_INDEX",
  "expiry_date": "06OCT26",
  "offset": "OTM2",
  "option_type": "PE"
}
```

```json
{
  "status": "success",
  "symbol": "NIFTY06OCT2622300PE",
  "exchange": "NFO",
  "lotsize": 65,
  "tick_size": 0.05,
  "freeze_qty": 1800,
  "underlying_ltp": 22421.95
}
```

## Sample API Request (Expiry Taken from a Future)

```json
{
  "apikey": "<APIKEY>",
  "underlying": "NIFTY27OCT26FUT",
  "exchange": "NFO",
  "offset": "ATM",
  "option_type": "CE"
}
```

```json
{
  "status": "success",
  "symbol": "NIFTY27OCT2622400CE",
  "exchange": "NFO",
  "lotsize": 65,
  "tick_size": 0.05,
  "freeze_qty": 1800,
  "underlying_ltp": 22421.95
}
```

## Request Body

| Parameter | Description | Mandatory/Optional | Default Value |
|-----------|-------------|-------------------|---------------|
| apikey | Your OpenAlgo API key | Mandatory | - |
| underlying | Underlying, for example `NIFTY`, `BANKNIFTY`, `SENSEX`, `RELIANCE`, `CRUDEOIL`, or a future such as `NIFTY27OCT26FUT` | Mandatory | - |
| exchange | Exchange of the underlying: `NSE_INDEX`, `BSE_INDEX`, `NSE`, `BSE`, `NFO`, `BFO`, `MCX`, `CDS`, `BCD`, `NCO`, `NCDEX` or `CRYPTO` | Mandatory | - |
| expiry_date | Expiry in `DDMMMYY` form, for example `06OCT26` | Optional when `underlying` is a future that names its expiry | - |
| offset | `ATM`, `ITM1` to `ITM50`, or `OTM1` to `OTM50` | Mandatory | - |
| option_type | `CE` or `PE` (any case) | Mandatory | - |
| strike_int | Strike interval in points, a whole number of at least 1. When set, the strike is counted in steps of this size from the multiple of it nearest the price, instead of from the listed strikes. | Optional | - |
| strategy | Accepted and ignored, so order payloads can be reused | Optional | - |

### The expiry format

`expiry_date` is `DDMMMYY` with no dashes (`06OCT26`). [Expiry](../symbol-services/expiry.md)
returns dates as `DD-MMM-YY` (`06-OCT-26`); remove the dashes before sending.
`06-OCT-26` is not read as a date and the answer is 404 "No strikes found".

When `underlying` is a future such as `NIFTY27OCT26FUT` and `expiry_date` is
also sent, `expiry_date` wins.

## Response Fields

| Field | Type | Description |
|-------|------|-------------|
| status | string | "success" or "error" |
| symbol | string | The option symbol |
| exchange | string | The options exchange (NFO, BFO, MCX, CDS ...) |
| lotsize | number | Lot size |
| tick_size | number | Minimum price step |
| freeze_qty | number | Exchange freeze quantity for the underlying, from Admin > Freeze Quantity |
| underlying_ltp | number | The price the ATM strike was worked out from |

## Understanding Offset

| Offset | Description | CE strike | PE strike |
|--------|-------------|-----------|-----------|
| ATM | At the money | Listed strike nearest the price | Listed strike nearest the price |
| ITM1-ITM50 | In the money | Below ATM | Above ATM |
| OTM1-OTM50 | Out of the money | Above ATM | Below ATM |

When the price is exactly between two listed strikes, the lower one is ATM.

## Which price sets ATM

| Exchange | Price used |
|----------|-----------|
| NSE_INDEX, BSE_INDEX, NSE, BSE | The underlying's LTP. Options are on NFO or BFO. |
| NFO, BFO with a future (`NIFTY27OCT26FUT`) | The index or stock LTP (`NIFTY`); the expiry comes from the future |
| MCX, CDS, BCD, NCO, NCDEX | The near-month unexpired future, as these have no spot |
| CRYPTO | The perpetual future `<UNDERLYING>USDFUT` |

## Errors

| HTTP | Body | Cause and fix |
|------|------|---------------|
| 400 | `{"status": "error", "message": "Validation error", "errors": {"offset": ["Offset must be ATM, ITM1-ITM50, or OTM1-OTM50"]}}` | A field is missing or not valid. Field errors are under `errors`. |
| 400 | `Expiry date required. Provide via expiry_date parameter or embed in underlying (e.g., NIFTY28OCT25FUT).` | No expiry anywhere. |
| 400 | `Offset ITM40 is out of range for available strikes. Please use a smaller offset.` | Fewer listed strikes than the offset. |
| 403 | `Invalid openalgo apikey` | The key is wrong, or no broker is logged in. |
| 404 | `No strikes found for NIFTY expiring 06-OCT-26. Please check expiry date or update master contract.` | No contracts for that expiry: a dashed or wrong date, an expiry that has passed, or an old master contract. |
| 404 | `Option symbol NIFTY06OCT2622425CE not found in NFO. Symbol may not exist or master contract needs update.` | With `strike_int`, the computed strike is not listed (here `strike_int` 25 on NIFTY, whose strikes are 50 apart). |
| 404 | `No unexpired futures contract for CRUDEOIL on MCX, so the ATM reference price cannot be determined. ...` | No near-month future in the master contract. |
| 404 | `No perpetual futures found for BTC on CRYPTO` | No `BTCUSDFUT` in the master contract. |
| 400 or 500 | `Failed to fetch LTP for NIFTY. <reason>` | The underlying's quote failed. |

## Notes

- The underlying's price comes from your broker, also in analyzer mode.
- To place the order straight away from an offset use
  [OptionsOrder](../order-management/optionsorder.md).
- In the desktop an `expiry_date` that is not a real `DDMMMYY` day is refused
  with the 404 above before the broker is asked for a price.

---

**Back to**: [API Documentation](../README.md)
