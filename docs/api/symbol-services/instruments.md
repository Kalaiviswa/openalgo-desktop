# Instruments

Download the master contract of one exchange, as JSON or as a CSV file.

## Endpoint URL

```http
GET http://127.0.0.1:5000/api/v1/instruments?apikey=<APIKEY>&exchange=NSE&format=json
```

This is a GET endpoint: the parameters go in the query string, not in a JSON body. A POST is answered 404.

## Sample API Request (JSON)

```bash
curl --get 'http://127.0.0.1:5000/api/v1/instruments' \
  --data-urlencode 'apikey=<APIKEY>' \
  --data-urlencode 'exchange=NFO' \
  --data-urlencode 'format=json'
```

## Sample API Response (JSON)

```json
{
  "data": [
    {
      "brexchange": "NFO",
      "brsymbol": "NIFTY26OCTFUT",
      "exchange": "NFO",
      "expiry": "27-OCT-26",
      "instrumenttype": "FUT",
      "lotsize": 65,
      "name": "NIFTY",
      "strike": 0.0,
      "symbol": "NIFTY27OCT26FUT",
      "tick_size": 0.1,
      "token": "12468226::::48704"
    },
    {
      "brexchange": "NFO",
      "brsymbol": "NIFTY26NOVFUT",
      "exchange": "NFO",
      "expiry": "23-NOV-26",
      "instrumenttype": "FUT",
      "lotsize": 65,
      "name": "NIFTY",
      "strike": 0.0,
      "symbol": "NIFTY23NOV26FUT",
      "tick_size": 0.1,
      "token": "15736578::::61471"
    }
  ],
  "message": "Found 34348 instruments",
  "status": "success"
}
```

The list is shortened here; the real answer has every instrument of the exchange. Symbols, tokens and row counts depend on your broker's master contract (this was recorded with Zerodha).

## Sample API Request (CSV)

```bash
curl --get 'http://127.0.0.1:5000/api/v1/instruments' \
  --data-urlencode 'apikey=<APIKEY>' \
  --data-urlencode 'exchange=NSE' \
  --data-urlencode 'format=csv' \
  --output instruments_NSE.csv
```

## Sample API Response (CSV)

```text
symbol,brsymbol,name,exchange,brexchange,token,expiry,strike,lotsize,instrumenttype,tick_size
GOLDSTAR-SM,GOLDSTAR-SM,GOLDSTAR POWER,NSE,NSE,257::::1,,0.0,11250,EQ,0.05
```

The CSV answer has `Content-Type: text/csv` and `Content-Disposition: attachment; filename=instruments_NSE.csv`. Lines end with CRLF. The first line is the header.

## Query Parameters

| Parameter | Description | Mandatory/Optional | Default Value |
|-----------|-------------|-------------------|---------------|
| apikey | Your OpenAlgo API key | Mandatory | - |
| exchange | NSE, NFO, CDS, BSE, BFO, BCD, MCX, NCDEX, NCO, NSE_INDEX, BSE_INDEX, MCX_INDEX, GLOBAL_INDEX or CRYPTO (case-sensitive) | Mandatory | - |
| format | `json` or `csv` (any case) | Optional | `json` |

## Response Fields (JSON)

| Field | Type | Description |
|-------|------|-------------|
| status | string | `success` or `error` |
| message | string | `Found N instruments`, or `No instruments found` |
| data | array | The instruments |

### Data Array Fields

Each row has the fields of [Symbol](./symbol.md) without `id` and `freeze_qty`: `symbol`, `brsymbol`, `name`, `exchange`, `brexchange`, `token`, `expiry` (`DD-MMM-YY`), `strike`, `lotsize`, `instrumenttype`, `tick_size`. The CSV columns are the same, in the order of its header.

## Sample API Response (Nothing Found)

```json
{
  "data": [],
  "message": "No instruments found",
  "status": "success"
}
```

This is the answer when your master contract has no instruments on that exchange, or none has been downloaded yet. It is JSON for `format=csv` too.

## Errors

Validation errors carry `message` as an object in JSON:

```json
{
  "message": {"exchange": ["Field may not be null."]},
  "status": "error"
}
```

With `format=csv`, a validation error is plain text (`Content-Type: text/plain`) instead, for example `{'exchange': ['Field may not be null.']}`.

| HTTP | `message` | When |
|------|-----------|------|
| 400 | `{"exchange": ["Field may not be null."]}` | No `exchange` |
| 400 | `{"exchange": ["Must be one of: NSE, NFO, ..."]}` | An unknown exchange |
| 400 | `{"format": ["Must be one of: json, csv."]}` | Another format |
| 400 | `{"apikey": ["Field may not be null."]}` | No `apikey` |
| 403 | `Invalid openalgo apikey` (JSON) | The key is wrong |
| 404 | `Not found` | A POST instead of a GET |

Errors common to every endpoint (missing or invalid API key, OpenAlgo locked, wrong method, rate limit) are listed in the [API overview](../README.md#errors-common-to-every-endpoint).

## Notes

- `exchange` is required in practice. The web's docs describe it as optional (all exchanges, `instruments_all.csv`), but the web answers the same 400 without it, and so does the desktop. Ask for each exchange you need.
- Instruments needs a valid API key but no broker session. It reads the master contract already downloaded.
- F&O contracts are listed and expire all the time. Fetch the list again each trading day rather than keeping an old copy.
- Rate limit: 100 requests per second per caller. See [Rate limiting](../rate-limiting.md).

## In the desktop

- In the desktop, an invalid key with `format=csv` is answered in JSON, `{"status": "error", "message": "Invalid openalgo apikey"}`, with status 403. The web answers the same status with the plain text `Invalid openalgo apikey`.

---

**Back to**: [API Documentation](../README.md)
