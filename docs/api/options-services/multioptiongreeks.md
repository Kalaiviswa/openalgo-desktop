# MultiOptionGreeks

Work out implied volatility and Greeks for 1 to 50 options in one call. Each
option gets its own entry; one failing option does not fail the others.

## Endpoint URL

```http
POST http://127.0.0.1:5000/api/v1/multioptiongreeks
```

## Sample API Request

```json
{
  "apikey": "<APIKEY>",
  "symbols": [
    {"symbol": "NIFTY06OCT2622400CE", "exchange": "NFO"},
    {"symbol": "NIFTY01JAN2010000CE", "exchange": "NFO"}
  ]
}
```

## Sample cURL Request

```bash
curl -X POST http://127.0.0.1:5000/api/v1/multioptiongreeks \
  -H 'Content-Type: application/json' \
  -d '{
  "apikey": "<APIKEY>",
  "symbols": [
    {"symbol": "NIFTY06OCT2622400CE", "exchange": "NFO"},
    {"symbol": "NIFTY01JAN2010000CE", "exchange": "NFO"}
  ]
}'
```

## Sample API Response

The second symbol does not exist, so the batch is `partial`. The HTTP status
is still 200.

```json
{
  "status": "partial",
  "message": "1 option Greeks calculation(s) failed: Option LTP not available",
  "data": [
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
      "interest_rate": 0,
      "implied_volatility": 15.23,
      "greeks": {
        "delta": 0.5686,
        "gamma": 0.001219,
        "theta": -19.5214,
        "vega": 8.3232,
        "rho": -0.013961
      }
    },
    {
      "status": "error",
      "symbol": "NIFTY01JAN2010000CE",
      "exchange": "NFO",
      "message": "Option LTP not available"
    }
  ],
  "summary": {"total": 2, "success": 1, "failed": 1}
}
```

## Request Body

| Parameter | Description | Mandatory/Optional | Default Value |
|-----------|-------------|-------------------|---------------|
| apikey | Your OpenAlgo API key | Mandatory | - |
| symbols | 1 to 50 option objects | Mandatory | - |
| interest_rate | Annual risk-free rate in percent, 0 to 100, for every option | Optional | 0 |
| expiry_time | Expiry time `HH:MM` (IST) for every option, instead of the exchange's cut-off | Optional | Exchange cut-off |

### Option Object Fields

| Field | Description | Mandatory/Optional |
|-------|-------------|-------------------|
| symbol | Option symbol, for example `NIFTY06OCT2622400CE` | Mandatory |
| exchange | `NFO`, `BFO`, `MCX`, `CDS`, `NCO`, `BCD`, `NCDEX` or `CRYPTO` | Mandatory |
| underlying_symbol | Symbol to quote for this option's underlying | Optional |
| underlying_exchange | Exchange of `underlying_symbol` | Optional |

There is no per-request `forward_price` here; use
[OptionGreeks](./optiongreeks.md) for that.

## Response Fields

| Field | Type | Description |
|-------|------|-------------|
| status | string | `success` (all worked), `partial` (some failed) or `error` (all failed) |
| message | string | Present only when something failed: the count and up to three distinct reasons |
| data | array | One entry per option, in the order sent |
| summary | object | `total`, `success` and `failed` counts |

Each successful entry has the same fields as an
[OptionGreeks](./optiongreeks.md) answer. A failed entry has `status:
"error"`, `symbol`, `exchange` and `message`.

## Expired options

An option whose expiry has passed is a success entry with `days_to_expiry: 0`,
`implied_volatility: 0`, all Greeks `0`, `time_value: 0.0`, its
`intrinsic_value`, and `note: "Option has expired - Greeks are no longer
applicable"`. It does not count as a failure.

## Errors

The batch answers HTTP 200 whatever happens to the individual options, even
when every one fails (`status: "error"`, `message: "All option Greeks
calculations failed: ..."`). The whole request is refused only for these:

| HTTP | Body | Cause and fix |
|------|------|---------------|
| 400 | `{"status": "error", "message": "Validation failed", "errors": {"symbols": ["Length must be between 1 and 50."]}}` | A field is missing or not valid, or more than 50 options. |
| 401 | `Invalid openalgo apikey` | The key is wrong, or no broker is logged in. 401 as on OpenAlgo web. |
| 500 | `Failed to get option Greeks: Invalid expiry_time format: ...` | `expiry_time` is not `HH:MM`. |

## Notes

- All option prices are fetched in one batch quote, and each underlying's
  forward is worked out once per expiry and shared by its options. The forward
  is the synthetic future of that expiry where it can be worked out, else the
  underlying's LTP, as in [OptionGreeks](./optiongreeks.md).
- An option missing from the master contract, or quoted at 0, fails with
  "Option LTP not available".
- Prices come from your broker, also in analyzer mode.
- For a whole chain, [OptionChain](./optionchain.md) with `with_greeks` is not
  limited to 50 options and costs no extra broker calls.

---

**Back to**: [API Documentation](../README.md)
