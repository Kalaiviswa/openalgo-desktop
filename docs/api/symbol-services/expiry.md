# Expiry

List the expiry dates of an underlying's futures or options that have not expired yet, nearest first.

## Endpoint URL

```http
POST http://127.0.0.1:5000/api/v1/expiry
```

## Sample API Request (Options)

```json
{
  "apikey": "<APIKEY>",
  "symbol": "NIFTY",
  "exchange": "NFO",
  "instrumenttype": "options"
}
```

## Sample cURL Request

```bash
curl -X POST http://127.0.0.1:5000/api/v1/expiry \
  -H 'Content-Type: application/json' \
  -d '{
  "apikey": "<APIKEY>",
  "symbol": "NIFTY",
  "exchange": "NFO",
  "instrumenttype": "options"
}'
```

## Sample API Response (Options)

```json
{
  "data": [
    "06-OCT-26",
    "13-OCT-26",
    "19-OCT-26",
    "27-OCT-26",
    "03-NOV-26",
    "23-NOV-26",
    "29-DEC-26",
    "30-MAR-27",
    "29-JUN-27",
    "28-SEP-27",
    "28-DEC-27",
    "27-JUN-28",
    "26-DEC-28",
    "26-JUN-29",
    "24-DEC-29",
    "25-JUN-30",
    "31-DEC-30",
    "24-JUN-31"
  ],
  "message": "Found 18 expiry dates for NIFTY options in NFO",
  "status": "success"
}
```

## Sample API Request (Futures)

```json
{
  "apikey": "<APIKEY>",
  "symbol": "CRUDEOIL",
  "exchange": "MCX",
  "instrumenttype": "futures"
}
```

## Sample API Response (Futures)

```json
{
  "data": [
    "19-OCT-26",
    "19-NOV-26",
    "18-DEC-26",
    "19-JAN-27",
    "19-FEB-27",
    "19-MAR-27",
    "21-JUN-27",
    "20-SEP-27",
    "17-DEC-27",
    "20-MAR-28"
  ],
  "message": "Found 10 expiry dates for CRUDEOIL futures in MCX",
  "status": "success"
}
```

## Request Body

| Parameter | Description | Mandatory/Optional | Default Value |
|-----------|-------------|-------------------|---------------|
| apikey | Your OpenAlgo API key | Mandatory | - |
| symbol | Underlying (`NIFTY`, `BANKNIFTY`, `RELIANCE`, `CRUDEOIL`); any case | Mandatory | - |
| exchange | NFO, BFO, MCX, CDS, NCO, BCD, NCDEX or CRYPTO (case-sensitive) | Mandatory | - |
| instrumenttype | `futures` or `options` (lowercase) | Mandatory | - |

## Response Fields

| Field | Type | Description |
|-------|------|-------------|
| status | string | `success` or `error` |
| message | string | `Found N expiry dates for <SYMBOL> <instrumenttype> in <EXCHANGE>` |
| data | array | Expiry dates as `DD-MMM-YY` strings, nearest first |

## Sample API Response (Nothing Found)

```json
{
  "data": [],
  "message": "No expiry dates found for FOO options in NFO",
  "status": "success"
}
```

## Errors

Validation errors on this endpoint carry `message` as an object:

```json
{
  "message": {"instrumenttype": ["Must be one of: futures, options."]},
  "status": "error"
}
```

| HTTP | `message` | When |
|------|-----------|------|
| 400 | `{"exchange": ["Must be one of: NFO, BFO, MCX, CDS, NCO, BCD, NCDEX, CRYPTO."]}` | Not a derivatives exchange (NSE, for example) |
| 400 | `Symbol parameter is required and cannot be empty` | `symbol` is empty or only spaces |
| 403 | `Invalid openalgo apikey` | The key is wrong |

Errors common to every endpoint (missing or invalid API key, OpenAlgo locked, wrong method, rate limit) are listed in the [API overview](../README.md#errors-common-to-every-endpoint).

## Notes

- **The format is `DD-MMM-YY`** (`06-OCT-26`), not the `DDMMMYY` that symbols and the options endpoints use. Remove the dashes before you build a symbol or call [OptionSymbol](../options-services/optionsymbol.md) or [OptionChain](../options-services/optionchain.md): `06-OCT-26` becomes `06OCT26`. A date sent with its dashes is not found.
- Dates before today (IST) are left out. An expiry falling today stays in the list for the whole day, also after the market closes.
- A contract counts for the underlying when its OpenAlgo symbol starts with the underlying followed by a date, so `NIFTY` does not pick up `NIFTYNXT50` contracts.
- Weekly and monthly expiries are both listed where the exchange has them.
- Expiry needs a valid API key but no broker session. It reads the master contract already downloaded; with none downloaded it answers with the "No expiry dates found" message and an empty list.
- Rate limit: 100 requests per second per caller. See [Rate limiting](../rate-limiting.md).

## In the desktop

- In the desktop, an underlying whose contracts in the master contract have all expired gets the "No expiry dates found" message. The web answers `Found 0 expiry dates for ...` in that case. Both send an empty `data` list with status 200.
- In the desktop, "today" is the date in India (IST), whatever your computer's time zone. The web uses the date of the server it runs on.

---

**Back to**: [API Documentation](../README.md)
