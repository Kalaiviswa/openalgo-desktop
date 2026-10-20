# CancelAllOrder

Cancel every working order in one request: open orders and trigger-pending stop orders.

## Endpoint URL

```http
POST http://127.0.0.1:5000/api/v1/cancelallorder
```

## Sample API Request

```json
{
  "apikey": "<APIKEY>",
  "strategy": "Python"
}
```

## Sample cURL Request

```bash
curl -X POST http://127.0.0.1:5000/api/v1/cancelallorder \
  -H 'Content-Type: application/json' \
  -d '{
  "apikey": "<APIKEY>",
  "strategy": "Python"
}'
```

## Sample API Response

```json
{
  "status": "success",
  "message": "Canceled 5 orders. Failed to cancel 0 orders.",
  "canceled_orders": [
    "250408001042620",
    "250408001042667",
    "250408001042642",
    "250408001043015",
    "250408001043386"
  ],
  "failed_cancellations": []
}
```

## Sample API Response (Partial Success)

In live mode, `failed_cancellations` lists the ids of the orders the broker did not cancel:

```json
{
  "status": "success",
  "message": "Canceled 3 orders. Failed to cancel 2 orders.",
  "canceled_orders": [
    "250408001042620",
    "250408001042667",
    "250408001042642"
  ],
  "failed_cancellations": [
    "250408001043015",
    "250408001043386"
  ]
}
```

## Sample API Response (Analyzer Mode)

```json
{
  "canceled_orders": ["26100395374263", "26100341808353", "26100334197687"],
  "failed_cancellations": [],
  "message": "Canceled 3 orders. Failed to cancel 0 orders.",
  "mode": "analyze",
  "status": "success"
}
```

## Sample API Response (Nothing To Cancel, Analyzer Mode)

```json
{
  "canceled_orders": [],
  "failed_cancellations": [],
  "message": "No open orders to cancel",
  "mode": "analyze",
  "status": "success"
}
```

## Request Body

| Parameter | Description | Mandatory/Optional | Default Value |
|-----------|-------------|-------------------|---------------|
| apikey | Your OpenAlgo API key (1 to 256 characters) | Mandatory | - |
| strategy | Strategy name | Mandatory | - |

`strategy` is required by the schema, as on the web (the web page lists it as optional). It does not limit which orders are cancelled: every working order is. Any other field is refused as `Unknown field.`

## Response Fields

| Field | Type | Description |
|-------|------|-------------|
| status | string | `success` or `error` |
| message | string | A summary: how many were cancelled and how many were not |
| canceled_orders | array of string | Ids of the orders cancelled |
| failed_cancellations | array | Orders not cancelled: order ids in live mode; objects in analyzer mode (below) |
| mode | string | `analyze` when analyzer mode is on; absent in live mode |

### Failed Cancellations in Analyzer Mode

| Field | Type | Description |
|-------|------|-------------|
| orderid | string | The order not cancelled |
| message | string | Why it was not cancelled |

## Analyzer Mode

With analyzer mode on, the sandbox cancels every `open` and `trigger pending` order placed since the 03:00 IST session start, releases their margin, and the answer carries `"mode": "analyze"`. With nothing to cancel it answers `No open orders to cancel` with two empty lists. See [Sandbox mode](../../user/sandbox.md).

## Errors

Validation errors carry `message` as a string that spells out a Python dictionary. In analyzer mode the body also carries `"mode": "analyze"`:

```json
{
  "message": "{'strategy': ['Missing data for required field.']}",
  "mode": "analyze",
  "status": "error"
}
```

| HTTP | `message` | When |
|------|-----------|------|
| 400 | `{'strategy': ['Missing data for required field.']}` | `strategy` is missing |
| 400 | The broker's own reason, for example `Zerodha did not return your order book, so no order was cancelled. Check your orders and try again.` | The broker refused, or its order book could not be read |
| 403 | `Cancel all orders operation is not allowed in Semi-Auto mode. Please switch to Auto mode to cancel orders.` | The API key's order mode is Semi-Auto (live mode only) |
| 500 | `Failed to cancel all orders due to internal error` | No usable answer from the broker |

Errors common to every endpoint (missing or invalid API key, OpenAlgo locked, wrong method, rate limit) are listed in the [API overview](../README.md#errors-common-to-every-endpoint).

## Notes

- Cancels all working orders: open limit orders and trigger-pending SL and SL-M orders (and AMO orders where the broker supports them).
- Filled orders, and orders the exchange is already processing, cannot be cancelled.
- The answer is a success even when some orders could not be cancelled; check `failed_cancellations`.
- Cancel all is never queued in the Action Center. In Semi-Auto it is refused in live mode and allowed in analyzer mode, as on the web.
- Rate limit: 100 requests per second per caller, the general API limit. Each cancel the broker receives also takes its turn in the broker pacer. See [Rate limiting](../rate-limiting.md).

## In the desktop

- In the desktop, analyzer mode cancels trigger-pending SL and SL-M orders too. The web's sandbox looks for the status `trigger_pending` while orders are stored as `trigger pending`, so it leaves them working.
- In the desktop, when the broker's order book cannot be read, the answer is an error that says nothing was cancelled. For several brokers the web answers success with nothing cancelled.
- In the desktop, if the order mode (Auto or Semi-Auto) cannot be read, a live cancel all is refused with 500 and nothing is sent; the web treats an unreadable mode as Auto.

---

**Back to**: [API Documentation](../README.md)
