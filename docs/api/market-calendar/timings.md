# Timings

Get the trading sessions of every exchange on one date, as epoch
milliseconds. Holidays, weekends and special sessions are taken into account.

A valid API key is enough; no broker needs to be logged in.

## Endpoint URL

```http
POST http://127.0.0.1:5000/api/v1/market/timings
```

## Sample API Request

```json
{
  "apikey": "<APIKEY>",
  "date": "2026-10-01"
}
```

## Sample cURL Request

```bash
curl -X POST http://127.0.0.1:5000/api/v1/market/timings \
  -H 'Content-Type: application/json' \
  -d '{
  "apikey": "<APIKEY>",
  "date": "2026-10-01"
}'
```

## Sample API Response

A regular weekday with the standard hours:

```json
{
  "status": "success",
  "data": [
    {"exchange": "NSE", "start_time": 1790826300000, "end_time": 1790848800000},
    {"exchange": "BSE", "start_time": 1790826300000, "end_time": 1790848800000},
    {"exchange": "NFO", "start_time": 1790826300000, "end_time": 1790849400000},
    {"exchange": "BFO", "start_time": 1790826300000, "end_time": 1790849400000},
    {"exchange": "MCX", "start_time": 1790825400000, "end_time": 1790879100000},
    {"exchange": "BCD", "start_time": 1790825400000, "end_time": 1790854200000},
    {"exchange": "CDS", "start_time": 1790825400000, "end_time": 1790854200000},
    {"exchange": "NCO", "start_time": 1790825400000, "end_time": 1790879100000},
    {"exchange": "CRYPTO", "start_time": 1790793000000, "end_time": 1790879399000}
  ]
}
```

A Saturday (2026-10-03): only CRYPTO trades.

```json
{
  "status": "success",
  "data": [
    {"exchange": "CRYPTO", "start_time": 1790965800000, "end_time": 1791052199000}
  ]
}
```

## Request Body

| Parameter | Description | Mandatory/Optional | Default Value |
|-----------|-------------|-------------------|---------------|
| apikey | Your OpenAlgo API key | Mandatory | - |
| date | The date, `YYYY-MM-DD`, from 2020-01-01 to 2050-12-31 | Mandatory | - |

## Response Fields

| Field | Type | Description |
|-------|------|-------------|
| status | string | "success" or "error" |
| data | array | One entry per exchange that trades that day |

### Timing Object Fields

| Field | Type | Description |
|-------|------|-------------|
| exchange | string | Exchange code |
| start_time | number | Session start, epoch **milliseconds** |
| end_time | number | Session end, epoch **milliseconds** |

## Standard Trading Hours (IST)

The hours a regular weekday starts from. They follow OpenAlgo web's defaults.

| Exchange | Open | Close |
|----------|------|-------|
| NSE | 09:15 | 15:30 |
| BSE | 09:15 | 15:30 |
| NFO | 09:15 | 15:40 |
| BFO | 09:15 | 15:40 |
| CDS | 09:00 | 17:00 |
| BCD | 09:00 | 17:00 |
| MCX | 09:00 | 23:55 |
| NCO | 09:00 | 23:55 |
| CRYPTO | 00:00 | 23:59:59 |

You can change an exchange's hours on **Admin > Market Timings**; this
endpoint answers with your hours. An install from an earlier desktop build
that still had the old defaults (MCX to 23:30, NFO to 15:30) was moved to
these, unless you had changed them yourself.

## What a date returns

| Date | Answer |
|------|--------|
| A weekday | Every exchange with its hours |
| Saturday or Sunday | CRYPTO only |
| A trading holiday | Only the sessions the holiday keeps open (for example MCX's evening session); an empty list when everything is closed |
| A settlement holiday | Every exchange with its regular hours |
| A special session (Muhurat trading) | Only the special session times |

## Errors

| HTTP | message | Cause and fix |
|------|---------|---------------|
| 400 | `{"date": ["Missing data for required field."]}` | No `date`. |
| 400 | `{"<field>": ["Unknown field."]}` | A field other than `apikey` and `date`. |
| 400 | `Invalid date format. Use YYYY-MM-DD` | The date is in another form, such as `03/10/2026`. |
| 400 | `Date must be between 2020-01-01 and 2050-12-31` | The date is out of range. |
| 403 | `Invalid openalgo apikey` | The key is wrong. |
| 500 | `An unexpected error occurred` | The calendar could not be read. |

## Notes

- In the desktop the API key is checked first, so a wrong key gets 403 even
  when the date is also wrong. OpenAlgo web answers this endpoint without
  checking the key.
- Holidays come from the list on **Admin > Market Holidays**; see
  [Holidays](./holidays.md).

## Converting Epoch to Readable Time

**JavaScript:**
```javascript
const date = new Date(1790826300000);
console.log(date.toLocaleString('en-IN', { timeZone: 'Asia/Kolkata' }));
// 1/10/2026, 9:15:00 am
```

**Python:**
```python
from datetime import datetime
from zoneinfo import ZoneInfo

dt = datetime.fromtimestamp(1790826300000 / 1000, ZoneInfo("Asia/Kolkata"))
print(dt.strftime('%Y-%m-%d %H:%M:%S %Z'))
# 2026-10-01 09:15:00 IST
```

---

**Back to**: [API Documentation](../README.md)
