# AnalyzerStatus

Find out whether analyzer mode is on, and how many requests the sandbox has logged.

## Endpoint URL

```http
POST http://127.0.0.1:5000/api/v1/analyzer
```

## Sample API Request

```json
{
  "apikey": "<APIKEY>"
}
```

## Sample cURL Request

```bash
curl -X POST http://127.0.0.1:5000/api/v1/analyzer \
  -H 'Content-Type: application/json' \
  -d '{
  "apikey": "<APIKEY>"
}'
```

## Sample API Response (Analyzer Mode On)

```json
{
  "data": {
    "analyze_mode": true,
    "mode": "analyze",
    "total_logs": 2
  },
  "status": "success"
}
```

## Sample API Response (Live Mode)

```json
{
  "data": {
    "analyze_mode": false,
    "mode": "live",
    "total_logs": 83
  },
  "status": "success"
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
| data | object | The analyzer status |

### Data Object Fields

| Field | Type | Description |
|-------|------|-------------|
| analyze_mode | boolean | `true` while analyzer mode is on |
| mode | string | `analyze` or `live` |
| total_logs | number | Requests recorded in the sandbox log (**Logs**, then **Sandbox Logs**) |

The body has no top-level `mode` field, in either mode.

## Analyzer Mode

Analyzer mode is one setting for the whole app: the API, webhooks, MCP clients and the app's own order pages all follow it. While it is on, orders and account requests go to the sandbox, never to your broker. See [Sandbox mode](../../user/sandbox.md).

| | Live mode | Analyzer mode |
|---|---|---|
| Orders reach your broker | Yes | No |
| Funds | Your broker account | Sandbox capital, 1 crore by default |
| Order ids | Your broker's | 14-digit sandbox ids |
| Answers | The web's shapes | The same shapes, with `"mode": "analyze"` |

## Errors

Validation errors on this endpoint carry `message` as a string that spells out a Python dictionary:

```json
{
  "message": "{'apikey': ['Missing data for required field.']}",
  "status": "error"
}
```

| HTTP | `message` | When |
|------|-----------|------|
| 403 | `Invalid openalgo apikey` | The key is wrong, or no broker is logged in |
| 500 | `An unexpected error occurred` | The setting could not be read |

Errors common to every endpoint (missing or invalid API key, OpenAlgo locked, wrong method, rate limit) are listed in the [API overview](../README.md#errors-common-to-every-endpoint).

## Notes

- Check the mode before you run anything that places orders.
- This endpoint needs a live broker session, as on the web, even though it only reads a setting.
- Use [AnalyzerToggle](./analyzertoggle.md) to switch modes, or the Live/Analyze switch in the app's top bar.
- Rate limit: 100 requests per second per caller. See [Rate limiting](../rate-limiting.md).

## In the desktop

- In the desktop, a failed GTT modify in analyzer mode is written to the sandbox log and counted in `total_logs`. A web defect keeps those failures out of the web's log, so for the same requests the web's count can be lower.

---

**Back to**: [API Documentation](../README.md)
