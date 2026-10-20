# Sandbox P&L by Symbol

Get today's sandbox profit and loss for each symbol, with totals. Available only while analyzer mode is on.

## Endpoint URL

```http
POST http://127.0.0.1:5000/api/v1/pnl/symbols
```

## Sample API Request

```json
{
  "apikey": "<APIKEY>"
}
```

## Sample cURL Request

```bash
curl -X POST http://127.0.0.1:5000/api/v1/pnl/symbols \
  -H 'Content-Type: application/json' \
  -d '{"apikey": "<APIKEY>"}'
```

## Sample API Response

```json
{
  "data": [
    {
      "exchange": "NSE",
      "pnl": 125.5,
      "product": "MIS",
      "quantity": 10,
      "symbol": "RELIANCE",
      "today_realized_pnl": 25.5,
      "total_pnl_today": 125.5,
      "unrealized_pnl": 100.0
    }
  ],
  "mode": "analyze",
  "status": "success",
  "total_pnl": 125.5,
  "total_pnl_today": 125.5,
  "total_today_realized_pnl": 25.5,
  "total_unrealized_pnl": 100.0
}
```

## Request Body

| Parameter | Description | Mandatory/Optional | Default Value |
|-----------|-------------|-------------------|---------------|
| apikey | Your OpenAlgo API key | Mandatory | - |

## Response Fields

| Field | Type | Description |
|-------|------|-------------|
| status | string | `success` or `error` |
| data | array | One row per position, as in the sandbox position book |
| total_pnl | number | Today's P&L of the whole book: booked today plus unrealized (the same as `total_pnl_today`) |
| total_unrealized_pnl | number | Unrealized P&L of the open positions |
| total_today_realized_pnl | number | P&L booked today |
| total_pnl_today | number | Booked today plus unrealized |
| mode | string | Always `analyze` |

### Data Array Fields

| Field | Type | Description |
|-------|------|-------------|
| symbol | string | OpenAlgo symbol |
| exchange | string | Exchange code |
| product | string | MIS, CNC or NRML |
| quantity | number | Net quantity: positive long, negative short, 0 for a position closed today |
| pnl | number | Today's P&L of the position: booked today plus unrealized |
| unrealized_pnl | number | Unrealized P&L at the last price, for an open position |
| today_realized_pnl | number | P&L booked today on this position |
| total_pnl_today | number | Booked today plus unrealized |

## Analyzer Mode

The rows are the sandbox position book's: open positions, plus positions closed this session that booked a profit or loss. "Today" starts at the 03:00 IST session boundary. The rows carry no `ltp` or `average_price`; use [PositionBook](../account-services/positionbook.md) for those. An empty book answers `"data": []` with every total `0.0`.

## Errors

| HTTP | Body | When |
|------|------|------|
| 400 | `{"status": "error", "message": "This endpoint is only available in sandbox/analyzer mode"}` | Analyzer mode is off. This is checked before anything else in the request, the API key included |
| 400 | `{"status": "error", "message": {"apikey": ["Missing data for required field."]}}` | No `apikey` (validation errors here are an object) |
| 403 | `{"status": "error", "message": "Invalid API key", "mode": "analyze"}` | The key is wrong |

Errors common to every endpoint (missing or invalid API key, OpenAlgo locked, wrong method, rate limit) are listed in the [API overview](../README.md#errors-common-to-every-endpoint).

## Notes

- This endpoint needs a valid API key but no broker session.
- The invalid-key message here is `Invalid API key`, not `Invalid openalgo apikey`, as on the web.
- Rate limit: 100 requests per second per caller. See [Rate limiting](../rate-limiting.md).

---

**Back to**: [API Documentation](../README.md)
