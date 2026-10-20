# Search

Search the master contract for instruments by symbol, name, broker symbol, token or strike.

## Endpoint URL

```http
POST http://127.0.0.1:5000/api/v1/search
```

## Sample API Request

```json
{
  "apikey": "<APIKEY>",
  "query": "RELIANCE",
  "exchange": "NSE"
}
```

## Sample cURL Request

```bash
curl -X POST http://127.0.0.1:5000/api/v1/search \
  -H 'Content-Type: application/json' \
  -d '{
  "apikey": "<APIKEY>",
  "query": "RELIANCE",
  "exchange": "NSE"
}'
```

## Sample API Response

```json
{
  "data": [
    {
      "brexchange": "NSE",
      "brsymbol": "RELIANCE",
      "exchange": "NSE",
      "expiry": "",
      "freeze_qty": 0,
      "instrumenttype": "EQ",
      "lotsize": 1,
      "name": "RELIANCE INDUSTRIES",
      "strike": 0.0,
      "symbol": "RELIANCE",
      "tick_size": 0.1,
      "token": "738561::::2885"
    },
    {
      "brexchange": "NSE",
      "brsymbol": "RIIL",
      "exchange": "NSE",
      "expiry": "",
      "freeze_qty": 0,
      "instrumenttype": "EQ",
      "lotsize": 1,
      "name": "RELIANCE INDUSTRIAL INFRA",
      "strike": 0.0,
      "symbol": "RIIL",
      "tick_size": 0.05,
      "token": "745473::::2912"
    }
  ],
  "message": "Found 7 matching symbols",
  "status": "success"
}
```

The list is shortened here. The broker symbol, token and `name` depend on your broker's master contract (this was recorded with Zerodha).

## Sample API Response (No Match)

```json
{
  "data": [],
  "message": "No matching symbols found",
  "status": "success"
}
```

## Request Body

| Parameter | Description | Mandatory/Optional | Default Value |
|-----------|-------------|-------------------|---------------|
| apikey | Your OpenAlgo API key | Mandatory | - |
| query | What to look for. Several words narrow the search: every word must match | Mandatory | - |
| exchange | Search only this exchange: NSE, NFO, CDS, BSE, BFO, BCD, MCX, NCDEX, NCO, NSE_INDEX, BSE_INDEX, MCX_INDEX, GLOBAL_INDEX or CRYPTO | Optional | every exchange |

## Response Fields

| Field | Type | Description |
|-------|------|-------------|
| status | string | `success` or `error` |
| message | string | `Found N matching symbols`, or `No matching symbols found` |
| data | array | Matching instruments, best match first |

### Data Array Fields

Each row has the fields of [Symbol](./symbol.md), without `id`: `symbol`, `brsymbol`, `name`, `exchange`, `brexchange`, `token`, `expiry` (`DD-MMM-YY`), `strike`, `lotsize`, `instrumenttype`, `tick_size` and `freeze_qty`.

## How Matching Works

- Matching ignores case.
- Each word of the query must appear in the symbol, the broker symbol, the name or the token. A number also matches an option's strike exactly.
- Results are ranked: an exact symbol first, then symbols that start with the query, then symbols that contain every word, then the rest. Within a rank, shorter symbols come first, then alphabetical order.
- At most 500 results are returned. Make the query more specific to reach the rest.

| Query | Finds |
|-------|-------|
| `RELIANCE` | Every instrument whose symbol, name or broker symbol contains RELIANCE |
| `NIFTY 22400` | NIFTY contracts with 22400 in the symbol or as the strike |
| `NIFTY 22400 CE` | NIFTY 22400 calls |
| `NIFTY OCT FUT` | NIFTY October futures |

## Errors

Validation errors on this endpoint carry `message` as an object:

```json
{
  "message": {"query": ["Missing data for required field."]},
  "status": "error"
}
```

| HTTP | `message` | When |
|------|-----------|------|
| 400 | `Query parameter is required and cannot be empty` | `query` is empty or only spaces |
| 400 | `{"exchange": ["Must be one of: NSE, NFO, ..."]}` | An unknown exchange |
| 403 | `Invalid openalgo apikey` | The key is wrong |

Errors common to every endpoint (missing or invalid API key, OpenAlgo locked, wrong method, rate limit) are listed in the [API overview](../README.md#errors-common-to-every-endpoint).

## Notes

- Search needs a valid API key but no broker session. It reads the master contract already downloaded; with none downloaded it finds nothing.
- With `exchange` set to an exchange that has no instruments in your master contract (for example CRYPTO with an equity broker), the whole master contract is searched, as on the web.
- Rate limit: 100 requests per second per caller. See [Rate limiting](../rate-limiting.md).

---

**Back to**: [API Documentation](../README.md)
