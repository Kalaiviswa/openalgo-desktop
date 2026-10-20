# SyntheticFuture

Work out the synthetic future price of an underlying for one expiry from its
ATM call and put (put-call parity). The answer is flat: there is no `data`
wrapper.

## Endpoint URL

```http
POST http://127.0.0.1:5000/api/v1/syntheticfuture
```

## Sample API Request

```json
{
  "apikey": "<APIKEY>",
  "underlying": "NIFTY",
  "exchange": "NSE_INDEX",
  "expiry_date": "06OCT26"
}
```

## Sample cURL Request

```bash
curl -X POST http://127.0.0.1:5000/api/v1/syntheticfuture \
  -H 'Content-Type: application/json' \
  -d '{
  "apikey": "<APIKEY>",
  "underlying": "NIFTY",
  "exchange": "NSE_INDEX",
  "expiry_date": "06OCT26"
}'
```

## Sample API Response

```json
{
  "status": "success",
  "underlying": "NIFTY",
  "underlying_ltp": 22421.95,
  "expiry": "06OCT26",
  "atm_strike": 22400.0,
  "synthetic_future_price": 22453.35
}
```

## Request Body

| Parameter | Description | Mandatory/Optional | Default Value |
|-----------|-------------|-------------------|---------------|
| apikey | Your OpenAlgo API key | Mandatory | - |
| underlying | Underlying, for example `NIFTY`, `BANKNIFTY`, `SENSEX` | Mandatory | - |
| exchange | Exchange of the underlying, for example `NSE_INDEX`, `BSE_INDEX`, `NSE`, `MCX` | Mandatory | - |
| expiry_date | Expiry in `DDMMMYY` form, for example `06OCT26` (no dashes) | Mandatory | - |

## Response Fields

| Field | Type | Description |
|-------|------|-------------|
| status | string | "success" or "error" |
| underlying | string | The underlying, as sent |
| underlying_ltp | number | The price the ATM strike was worked out from |
| expiry | string | The expiry, as sent |
| atm_strike | number | The ATM strike used |
| synthetic_future_price | number | ATM strike + call price - put price, rounded to two decimals |

## Formula

```
Synthetic Future Price = ATM Strike + ATM Call LTP - ATM Put LTP
```

The ATM strike is the listed strike nearest the underlying's price, chosen
the same way as in [OptionSymbol](./optionsymbol.md).

### Example Calculation

```
Spot (underlying_ltp): 22421.95
ATM strike:            22400
ATM call LTP:          156.95
ATM put LTP:           103.60

Synthetic future = 22400 + 156.95 - 103.60 = 22453.35
Basis            = 22453.35 - 22421.95   =    31.40 points
```

A positive basis is the cost of carry; near expiry the synthetic future moves
towards the spot price.

## Errors

| HTTP | Body | Cause and fix |
|------|------|---------------|
| 400 | `{"status": "error", "message": "Validation error", "errors": {...}}` | A field is missing or not valid. |
| 403 | `Invalid openalgo apikey` | The key is wrong, or no broker is logged in. |
| 404 | `No strikes found for NIFTY expiring 06-OCT-26. Please check expiry date or update master contract.` | No contracts for that expiry (for example a dashed date). |
| 404 | `Option symbol ... not found in NFO. Symbol may not exist or master contract needs update.` | The ATM call or put is not in the master contract. |
| 500 | `Could not fetch LTP for Call option: NIFTY06OCT2622400CE` | The ATM call has no price (the same for the put). |
| 400 or 500 | `Failed to fetch LTP for NIFTY. <reason>` | The underlying's quote failed. |

## Notes

- Prices come from your broker, also in analyzer mode.
- An option with a last price of 0 counts as having no price.
- [OptionGreeks](./optiongreeks.md) and [OptionChain](./optionchain.md) price
  their Greeks against this same forward.

## Use Cases

- **Arbitrage**: compare with the traded future
- **Fair value**: is the future rich or cheap?
- **Options pricing**: the forward for Greeks

---

**Back to**: [API Documentation](../README.md)
