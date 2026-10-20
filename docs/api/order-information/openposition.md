# OpenPosition

Get the net quantity you hold in one symbol, for one exchange and product.

## Endpoint URL

```http
POST http://127.0.0.1:5000/api/v1/openposition
```

## Sample API Request

```json
{
  "apikey": "<APIKEY>",
  "strategy": "Test Strategy",
  "symbol": "YESBANK",
  "exchange": "NSE",
  "product": "MIS"
}
```

## Sample cURL Request

```bash
curl -X POST http://127.0.0.1:5000/api/v1/openposition \
  -H 'Content-Type: application/json' \
  -d '{
  "apikey": "<APIKEY>",
  "strategy": "Test Strategy",
  "symbol": "YESBANK",
  "exchange": "NSE",
  "product": "MIS"
}'
```

## Sample API Response

```json
{
  "quantity": -10,
  "status": "success"
}
```

## Sample API Response (No Position)

```json
{
  "quantity": 0,
  "status": "success"
}
```

## Sample API Response (Analyzer Mode)

```json
{
  "mode": "analyze",
  "quantity": 5,
  "status": "success"
}
```

## Request Body

| Parameter | Description | Mandatory/Optional | Default Value |
|-----------|-------------|-------------------|---------------|
| apikey | Your OpenAlgo API key | Mandatory | - |
| strategy | Strategy name | Mandatory | - |
| symbol | OpenAlgo symbol | Mandatory | - |
| exchange | Exchange code, as in the position book | Mandatory | - |
| product | MIS, CNC or NRML | Mandatory | - |

## Response Fields

The answer is flat: there is no `data` object.

| Field | Type | Description |
|-------|------|-------------|
| status | string | `success` or `error` |
| quantity | number | Net quantity: positive long, negative short, 0 flat or not found. A whole number, except a CRYPTO position on Delta Exchange |
| mode | string | `analyze` in analyzer mode; absent in live mode |

## Analyzer Mode

The sandbox answers from its own positions with `"mode": "analyze"`, and every answer is recorded in the sandbox log, a flat position included, as on the web.

## Errors

Validation errors on this endpoint carry `message` as a string that spells out a Python dictionary, with `"mode": "analyze"` added in analyzer mode:

```json
{
  "message": "{'product': ['Must be one of: MIS, NRML, CNC.']}",
  "status": "error"
}
```

| HTTP | `message` | When |
|------|-----------|------|
| 500 | The broker's reason, or `Could not reach the broker. Check your internet connection and try again.` | The broker did not return the positions |

Errors common to every endpoint (missing or invalid API key, OpenAlgo locked, wrong method, rate limit) are listed in the [API overview](../README.md#errors-common-to-every-endpoint).

## Notes

- The position is looked up in the [position book](../account-services/positionbook.md) by symbol, exchange and product together. A position held under another product counts as 0 here.
- `exchange` is not checked against the list of exchanges; an unknown one simply finds nothing.
- `strategy` is required, as in the web's code; the web docs list it as optional.
- Use it with [PlaceSmartOrder](../order-management/placesmartorder.md) for position-aware orders.
- Rate limit: 100 requests per second per caller. See [Rate limiting](../rate-limiting.md).

## In the desktop

- In the desktop, `quantity` is a JSON number for every broker. The web passes on whatever the broker's position book holds, a string for some brokers (`"-10"` in the web docs). A client that parses it as a number works with both.

---

**Back to**: [API Documentation](../README.md)
