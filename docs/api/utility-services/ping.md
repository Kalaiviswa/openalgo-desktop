# Ping

Check that your API key works and that a broker session is live. It answers with the name of the broker you are logged in to.

## Endpoint URL

```http
POST http://127.0.0.1:5000/api/v1/ping
```

## Sample API Request

```json
{
  "apikey": "<APIKEY>"
}
```

## Sample cURL Request

```bash
curl -X POST http://127.0.0.1:5000/api/v1/ping \
  -H 'Content-Type: application/json' \
  -d '{"apikey": "<APIKEY>"}'
```

## Sample API Response

```json
{
  "data": {
    "broker": "zerodha",
    "message": "pong"
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
| data.message | string | Always `pong` |
| data.broker | string | The broker you are logged in to, in lowercase (`zerodha`, `angel`, `fyers` and so on) |

## Errors

Validation errors on this endpoint carry `message` as an object:

```json
{
  "message": {"foo": ["Unknown field."]},
  "status": "error"
}
```

| HTTP | `message` | When |
|------|-----------|------|
| 400 | `{"apikey": ["Missing data for required field."]}` | No `apikey` in the body, for example when it is sent only in the `X-API-KEY` header |
| 403 | `Invalid openalgo apikey` | The key is wrong, or no broker is logged in |

Errors common to every endpoint (missing or invalid API key, OpenAlgo locked, wrong method, rate limit) are listed in the [API overview](../README.md#errors-common-to-every-endpoint).

## Notes

- Ping is not a check that the app is running. It needs a valid key and a live broker session; without a broker session it answers 403, like every endpoint that reaches your broker.
- Ping does not contact the broker. A 200 means OpenAlgo holds a session for it, not that the broker's servers are reachable at that moment.
- The answer is the same in analyzer mode.
- After OpenAlgo starts, and after you log out, ping answers 503 until you sign in again. See [Before You Call the API](../README.md#before-you-call-the-api).
- Rate limit: 100 requests per second per caller. See [Rate limiting](../rate-limiting.md).

---

**Back to**: [API Documentation](../README.md)
