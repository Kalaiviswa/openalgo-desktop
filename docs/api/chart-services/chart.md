# Chart Preferences

Read and save chart workspace preferences: a set of named values (theme,
layout and so on) kept for your account, for charting clients to store their
settings in.

A valid API key is enough for both calls; no broker needs to be logged in.

## Read Preferences

```http
GET http://127.0.0.1:5000/api/v1/chart?apikey=<APIKEY>
```

```bash
curl --get 'http://127.0.0.1:5000/api/v1/chart' \
  --data-urlencode 'apikey=<APIKEY>'
```

### Response

```json
{
  "status": "success",
  "data": {
    "tv_theme": "dark",
    "tv_chart_layout": "{\"interval\":\"15m\"}"
  }
}
```

With nothing saved yet, `data` is `{}`.

## Save Preferences

```http
POST http://127.0.0.1:5000/api/v1/chart
```

```bash
curl -X POST 'http://127.0.0.1:5000/api/v1/chart' \
  -H 'Content-Type: application/json' \
  -d '{
    "apikey": "<APIKEY>",
    "tv_theme": "dark",
    "tv_chart_layout": "{\"interval\":\"15m\"}"
  }'
```

### Response

```json
{
  "status": "success",
  "message": "Preferences updated successfully"
}
```

Every key in the body except `apikey` is a preference. A save changes only
the keys it sends; the others stay as they were.

### Limits

- At most 50 preferences in one request.
- A preference name of at most 50 characters.
- A value of at most 1 MB as JSON. The whole request body is limited to
  1 MiB in the desktop, so in practice one large value must stay a little
  under that.
- At least one preference besides `apikey`.

### How values are stored

Values are stored as text, as on OpenAlgo web:

| Sent | Read back |
|------|-----------|
| `"dark"` | `"dark"` |
| `5` | `"5"` |
| `true` / `false` | `"1"` / `"0"` |
| `null` | `null` |
| `{"interval": "15m"}` | `"{\"interval\":\"15m\"}"` |

Send structured settings as a JSON string, and parse it after reading.

In the desktop an object or a list is saved as its JSON text. OpenAlgo web
fails to save one.

## Errors

| Call | HTTP | message | Cause |
|------|------|---------|-------|
| GET | 400 | `Missing apikey parameter` | No `apikey` in the query string. |
| GET, POST | 403 | `Invalid openalgo apikey` | The key is wrong. |
| POST | 400 | `No data provided` | Empty body. |
| POST | 400 | `{"apikey": ["Missing data for required field."]}` | No `apikey` in the body. |
| POST | 400 | `Too many preference keys (max 50)` | More than 50 preferences. |
| POST | 400 | `Preference key too long: <first 20 characters>... (max 50 chars)` | A name over 50 characters. |
| POST | 400 | `Preference value too large for key: <name> (max 1MB)` | A value over 1 MB. |
| POST | 400 | `No preferences provided to update` | Only `apikey` was sent. |
| POST | 413 | JSON error | The request body is over 1 MiB. |
| GET | 500 | `Server Error` | The preferences could not be read. |
| POST | 500 | `Failed to update preferences` | The preferences could not be saved. |

The size limits are checked before the API key, as on OpenAlgo web.

---

**Back to**: [API Documentation](../README.md)
