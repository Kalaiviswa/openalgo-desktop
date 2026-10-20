# Holidays

Get the market holidays of a year, with the exchanges each holiday closes and
any session that still runs that day (for example MCX's evening session, or
Muhurat trading).

A valid API key is enough; no broker needs to be logged in.

## Endpoint URL

```http
POST http://127.0.0.1:5000/api/v1/market/holidays
```

## Sample API Request

```json
{
  "apikey": "<APIKEY>",
  "year": 2026
}
```

## Sample cURL Request

```bash
curl -X POST http://127.0.0.1:5000/api/v1/market/holidays \
  -H 'Content-Type: application/json' \
  -d '{
  "apikey": "<APIKEY>",
  "year": 2026
}'
```

## Sample API Response

Trimmed to four of the seventeen holidays of 2026.

```json
{
  "status": "success",
  "year": 2026,
  "timezone": "Asia/Kolkata",
  "data": [
    {
      "date": "2026-01-15",
      "description": "Municipal Corporation Election - Maharashtra",
      "holiday_type": "TRADING_HOLIDAY",
      "closed_exchanges": ["NSE", "BSE", "NFO", "BFO", "CDS", "BCD"],
      "open_exchanges": [
        {
          "exchange": "MCX",
          "start_time": 1768476600000,
          "end_time": 1768501500000
        }
      ]
    },
    {
      "date": "2026-01-26",
      "description": "Republic Day",
      "holiday_type": "TRADING_HOLIDAY",
      "closed_exchanges": ["NSE", "BSE", "NFO", "BFO", "CDS", "BCD", "MCX"],
      "open_exchanges": []
    },
    {
      "date": "2026-11-08",
      "description": "Diwali Laxmi Pujan (Muhurat Trading)",
      "holiday_type": "SPECIAL_SESSION",
      "closed_exchanges": [],
      "open_exchanges": [
        {"exchange": "NSE", "start_time": 1794141000000, "end_time": 1794145500000},
        {"exchange": "BSE", "start_time": 1794141000000, "end_time": 1794145500000},
        {"exchange": "NFO", "start_time": 1794141000000, "end_time": 1794145500000},
        {"exchange": "BFO", "start_time": 1794141000000, "end_time": 1794145500000},
        {"exchange": "CDS", "start_time": 1794141000000, "end_time": 1794145500000},
        {"exchange": "BCD", "start_time": 1794141000000, "end_time": 1794145500000},
        {"exchange": "MCX", "start_time": 1794141000000, "end_time": 1794163500000}
      ]
    },
    {
      "date": "2026-12-25",
      "description": "Christmas",
      "holiday_type": "TRADING_HOLIDAY",
      "closed_exchanges": ["NSE", "BSE", "NFO", "BFO", "CDS", "BCD", "MCX"],
      "open_exchanges": []
    }
  ]
}
```

On 2026-01-15, `1768476600000` is 17:00 IST and `1768501500000` is 23:55 IST:
MCX trades its evening session only.

## Request Body

| Parameter | Description | Mandatory/Optional | Default Value |
|-----------|-------------|-------------------|---------------|
| apikey | Your OpenAlgo API key | Mandatory | - |
| year | Year, 2020 to 2050 | Optional | The current year (IST) |

## Response Fields

| Field | Type | Description |
|-------|------|-------------|
| status | string | "success" or "error" |
| year | number | The year listed |
| timezone | string | Always `Asia/Kolkata` |
| data | array | Holidays in date order |

### Holiday Object Fields

| Field | Type | Description |
|-------|------|-------------|
| date | string | The day, `YYYY-MM-DD` |
| description | string | The holiday's name |
| holiday_type | string | See below |
| closed_exchanges | array | Exchanges closed all day |
| open_exchanges | array | Sessions that run that day |

### Open Exchanges Object Fields

| Field | Type | Description |
|-------|------|-------------|
| exchange | string | Exchange code |
| start_time | number | Session start, epoch **milliseconds** |
| end_time | number | Session end, epoch **milliseconds** |

## Holiday Types

| Type | Description |
|------|-------------|
| TRADING_HOLIDAY | The listed exchanges do not trade |
| SETTLEMENT_HOLIDAY | No settlement; trading may still run |
| SPECIAL_SESSION | A special session on a non-trading day, such as Muhurat trading |

## Errors

| HTTP | message | Cause and fix |
|------|---------|---------------|
| 400 | `{"year": ["Must be greater than or equal to 2020 and less than or equal to 2050."]}` | Year outside 2020 to 2050. |
| 400 | `{"year": ["Not a valid integer."]}` | `year` is not a whole number. |
| 400 | `{"<field>": ["Unknown field."]}` | A field other than `apikey` and `year`. |
| 403 | `Invalid openalgo apikey` | The key is wrong. |
| 500 | `An unexpected error occurred` | The calendar could not be read. |

## Notes

- The desktop ships the 2025 and 2026 holiday lists of OpenAlgo web. A year
  with nothing listed answers a success with an empty `data`. You can add and
  remove holidays on **Admin > Market Holidays**; this endpoint answers from
  that list.
- In the desktop the API key is checked: a wrong key gets 403. OpenAlgo web
  answers this endpoint without checking the key, so a client that works there
  with its real key works here unchanged.
- Times are epoch milliseconds, unlike [History](../market-data/history.md),
  which uses seconds.

## Use Cases

- **Calendar planning**: know trading days in advance
- **Strategy scheduling**: skip or adjust strategies on holidays
- **Risk management**: plan for thin-liquidity days

## Related

- [Timings](./timings.md) - the trading sessions of one date

---

**Back to**: [API Documentation](../README.md)
