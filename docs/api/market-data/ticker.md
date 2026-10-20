# Ticker

Get historical candles with a GET request, as JSON or as plain-text lines that
charting tools such as Amibroker can read directly.

## Endpoint URL

```http
GET http://127.0.0.1:5000/api/v1/ticker/<EXCHANGE>:<SYMBOL>?apikey=<APIKEY>&interval=D&from=YYYY-MM-DD&to=YYYY-MM-DD&format=json
```

## Sample Request

```bash
curl --get 'http://127.0.0.1:5000/api/v1/ticker/NSE:RELIANCE' \
  --data-urlencode 'apikey=<APIKEY>' \
  --data-urlencode 'interval=D' \
  --data-urlencode 'from=2026-09-01' \
  --data-urlencode 'to=2026-10-02' \
  --data-urlencode 'format=json'
```

## Query Parameters

| Parameter | Required | Description |
|---|---|---|
| `apikey` | Yes | Your OpenAlgo API key |
| `interval` | No | A [History](./history.md) interval; default `D` |
| `from` | Yes | Start date, `YYYY-MM-DD` |
| `to` | Yes | End date, `YYYY-MM-DD` |
| `format` | No | `json` or `txt`; default `json` |

The path is `EXCHANGE:SYMBOL`, for example `NSE:RELIANCE` or
`NFO:NIFTY27OCT26FUT`.

## JSON Response

The same candle shape as [History](./history.md), with `timestamp` in epoch
seconds.

```json
{
  "status": "success",
  "data": [
    {
      "close": 1309.0,
      "high": 1311.8,
      "low": 1280.0,
      "oi": 0,
      "open": 1285.0,
      "timestamp": 1788220800,
      "volume": 12617528
    }
  ]
}
```

## Text Response (`format=txt`)

Plain text (`Content-Type: text/plain`), one candle per line, no header line,
times in IST.

Daily (`interval=D`): `EXCHANGE:SYMBOL,YYYY-MM-DD,open,high,low,close,volume`

```text
NSE:RELIANCE,2026-09-01,1285.0,1311.8,1280.0,1309.0,12617528
NSE:RELIANCE,2026-09-02,1298.0,1321.9,1293.1,1313.1,13497841
```

Intraday: the time follows the date.

```text
NSE:RELIANCE,2026-10-01,09:15:00,1180.1,1182.5,1177.0,1180.2,653284
NSE:RELIANCE,2026-10-01,09:20:00,1180.2,1180.4,1177.9,1178.8,342956
```

## Date Range Limits

Before asking the broker, the start date is moved forward when the range is
too long:

| Interval | Longest range |
|----------|---------------|
| `D`, `W`, `M` | 3650 days before `to` |
| Every other interval | 30 days before `to` |

Use [History](./history.md) for longer intraday ranges.

## Errors

JSON format:

| HTTP | Body | Cause and fix |
|------|------|---------------|
| 400 | `{"status": "error", "message": {"end_date": ["Field may not be null."], "start_date": ["Field may not be null."]}}` | `from` or `to` is missing. The message names them `start_date` and `end_date`, as on OpenAlgo web. |
| 400 | `{"status": "error", "message": {"interval": ["Must be one of: ..."]}}` | The interval is not a History interval. |
| 403 | `{"status": "error", "message": "Invalid openalgo apikey"}` | The key is wrong, or no broker is logged in. |
| 500 | `{"status": "error", "message": "<broker's message>"}` | The broker refused the request or could not be reached. |

Text format: the same cases answer plain text with the same HTTP status:

| HTTP | Text |
|------|------|
| 400 | `Invalid request: check apikey, interval, from and to (YYYY-MM-DD)` |
| 403 | `Invalid openalgo apikey` |
| 500 | The broker's message |

## Notes

- In the desktop a path without an exchange (`/api/v1/ticker/SBIN`) is read as
  `NSE:SBIN`. OpenAlgo web answers such a path with `NSE:RELIANCE` data
  whatever symbol you named. Always send the exchange.
- In the desktop a text-format error comes back as text with its own status
  (403 for a wrong key). OpenAlgo web answers every text-format error with a
  500 Internal Server Error.
- The symbol is not checked against the master contract up front, as on
  OpenAlgo web; an unknown symbol fails when the broker request is made, with
  500.
- The ticker always reads from your broker. For candles stored in Historify use
  [History](./history.md) with `source: "db"`.
- `adjusted` and `sort`, which some clients send, are ignored.

---

**Back to**: [API Documentation](../README.md)
