# MultiQuotes

Get quotes for several symbols in one call. The answer lists the quotes under
`results`, not `data`.

## Endpoint URL

```http
POST http://127.0.0.1:5000/api/v1/multiquotes
```

## Sample API Request

```json
{
  "apikey": "<APIKEY>",
  "symbols": [
    {"symbol": "RELIANCE", "exchange": "NSE"},
    {"symbol": "NIFTY27OCT26FUT", "exchange": "NFO"},
    {"symbol": "FOOBARBAZ", "exchange": "NSE"}
  ]
}
```

## Sample cURL Request

```bash
curl -X POST http://127.0.0.1:5000/api/v1/multiquotes \
  -H 'Content-Type: application/json' \
  -d '{
  "apikey": "<APIKEY>",
  "symbols": [
    {"symbol": "RELIANCE", "exchange": "NSE"},
    {"symbol": "NIFTY27OCT26FUT", "exchange": "NFO"},
    {"symbol": "FOOBARBAZ", "exchange": "NSE"}
  ]
}'
```

## Sample API Response

Recorded with the market closed. `FOOBARBAZ` is not a real symbol, so it comes
back with an `error` instead of `data`, and it is listed first.

```json
{
  "status": "success",
  "results": [
    {
      "symbol": "FOOBARBAZ",
      "exchange": "NSE",
      "error": "Symbol 'FOOBARBAZ' not found for exchange 'NSE'. Please verify the symbol name and ensure master contracts are downloaded."
    },
    {
      "symbol": "RELIANCE",
      "exchange": "NSE",
      "data": {
        "ask": 0,
        "ask_qty": 0,
        "bid": 0,
        "bid_qty": 0,
        "high": 1183.9,
        "low": 1160.8,
        "ltp": 1167.7,
        "oi": 0,
        "open": 1180.1,
        "prev_close": 1187,
        "volume": 0
      }
    },
    {
      "symbol": "NIFTY27OCT26FUT",
      "exchange": "NFO",
      "data": {
        "ask": 0,
        "ask_qty": 0,
        "bid": 0,
        "bid_qty": 0,
        "high": 22702.4,
        "low": 22321,
        "ltp": 22520,
        "oi": 19680440,
        "open": 22630,
        "prev_close": 22707.3,
        "volume": 0
      }
    }
  ]
}
```

## Request Body

| Parameter | Description | Mandatory/Optional | Default Value |
|-----------|-------------|-------------------|---------------|
| apikey | Your OpenAlgo API key | Mandatory | - |
| symbols | Array of symbol objects, at least one | Mandatory | - |

### Symbol Object Fields

| Field | Description |
|-------|-------------|
| symbol | Symbol in OpenAlgo format |
| exchange | One of NSE, NFO, CDS, BSE, BFO, BCD, MCX, NCDEX, NCO, NSE_INDEX, BSE_INDEX, MCX_INDEX, GLOBAL_INDEX, CRYPTO (upper case) |

## Response Fields

| Field | Type | Description |
|-------|------|-------------|
| status | string | "success" when at least one symbol was known |
| results | array | One entry per symbol |

### Results Array Fields

| Field | Type | Description |
|-------|------|-------------|
| symbol | string | Symbol as sent |
| exchange | string | Exchange as sent |
| data | object | The quote, with the same eleven fields as [Quotes](./quotes.md). Absent when `error` is present. |
| error | string | Why this symbol has no quote: not in the master contract, or the broker sent none |

## Errors

| HTTP | Body | Cause and fix |
|------|------|---------------|
| 400 | `{"status": "error", "message": {"symbols": ["Shorter than minimum length 1."]}}` | `symbols` is empty. |
| 400 | `{"status": "error", "message": "<first symbol's error>", "invalid_symbols": [{"symbol", "exchange", "error"}, ...]}` | None of the symbols is in the master contract. |
| 403 | `Invalid openalgo apikey` | The key is wrong, or no broker is logged in. |
| 500 | The broker's own message | The broker refused the batch or could not be reached. |

A field error inside one symbol object (a missing `exchange`, an unknown
exchange) refuses the whole request with 400, as on OpenAlgo web.

## Notes

- Symbols that are not in the master contract come first, in the order sent;
  the broker's quotes follow in the order sent. Match results by `symbol` and
  `exchange`, not by position.
- One unknown symbol does not fail the call: the overall `status` stays
  "success" and only that entry has `error`.
- OpenAlgo sends the list to your broker in the batch sizes it accepts, or one
  quote at a time for a broker with no batch quote call. The desktop sets no
  fixed maximum; the whole request body is limited to 1 MiB.
- Quotes always come from your broker, also in analyzer mode.

## Use Cases

- **Watchlist updates**: refresh every symbol of a watchlist at once
- **Portfolio valuation**: the LTP of every holding
- **Multi-symbol strategies**: watch correlated symbols together

---

**Back to**: [API Documentation](../README.md)
