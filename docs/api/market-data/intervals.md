# Intervals

Get the history intervals your logged-in broker offers, grouped by unit.

## Endpoint URL

```http
POST http://127.0.0.1:5000/api/v1/intervals
```

## Sample API Request

```json
{
  "apikey": "<APIKEY>"
}
```

## Sample cURL Request

```bash
curl -X POST http://127.0.0.1:5000/api/v1/intervals \
  -H 'Content-Type: application/json' \
  -d '{
  "apikey": "<APIKEY>"
}'
```

## Sample API Response

Recorded with Zerodha.

```json
{
  "status": "success",
  "data": {
    "days": ["D"],
    "hours": ["1h"],
    "minutes": ["1m", "3m", "5m", "10m", "15m", "30m"],
    "months": [],
    "seconds": [],
    "weeks": []
  }
}
```

## Request Body

| Parameter | Description | Mandatory/Optional | Default Value |
|-----------|-------------|-------------------|---------------|
| apikey | Your OpenAlgo API key | Mandatory | - |

## Response Fields

| Field | Type | Description |
|-------|------|-------------|
| status | string | "success" or "error" |
| data | object | Intervals by unit |

### Data Object Fields

| Field | Type | Description |
|-------|------|-------------|
| seconds | array | Second intervals, for example `1s` |
| minutes | array | Minute intervals, for example `1m`, `5m`, `15m` |
| hours | array | Hour intervals, for example `1h`, `2h` |
| days | array | `D` when daily candles are offered |
| weeks | array | `W` when weekly candles are offered |
| months | array | `M` when monthly candles are offered |

Each list is sorted by its number (`1m`, `3m`, `5m`, `10m` ...).

## Common Interval Values

| Interval | Description |
|----------|-------------|
| 1m | 1 minute |
| 3m | 3 minutes |
| 5m | 5 minutes |
| 10m | 10 minutes |
| 15m | 15 minutes |
| 30m | 30 minutes |
| 1h | 1 hour |
| D | Daily |
| W | Weekly |
| M | Monthly |

## Errors

| HTTP | message | Cause and fix |
|------|---------|---------------|
| 400 | `{"apikey": ["Missing data for required field."]}` | No `apikey` in the body. |
| 403 | `Invalid openalgo apikey` | The key is wrong, or no broker is logged in. |

## Notes

- The list depends on the broker that is logged in now, and only intervals
  that [History](./history.md) accepts are listed.
- The custom chart intervals you add in the `/trading` terminal or Historify
  (for example `7m` or `2D`) are not listed here: the app builds them from
  these standard intervals.
- Check this list before asking [History](./history.md) for an interval with
  `source: "api"`.

---

**Back to**: [API Documentation](../README.md)
