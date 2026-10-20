# OrderBook

Get today's orders with a summary count: placed, open, completed, cancelled and rejected.

## Endpoint URL

```http
POST http://127.0.0.1:5000/api/v1/orderbook
```

## Sample API Request

```json
{
  "apikey": "<APIKEY>"
}
```

## Sample cURL Request

```bash
curl -X POST http://127.0.0.1:5000/api/v1/orderbook \
  -H 'Content-Type: application/json' \
  -d '{
  "apikey": "<APIKEY>"
}'
```

## Sample API Response

```json
{
  "status": "success",
  "data": {
    "orders": [
      {
        "action": "BUY",
        "symbol": "RELIANCE",
        "exchange": "NSE",
        "orderid": "250408000989443",
        "product": "MIS",
        "quantity": 1,
        "price": 0.0,
        "pricetype": "MARKET",
        "order_status": "complete",
        "trigger_price": 0.0,
        "timestamp": "2025-04-08 13:58:03"
      },
      {
        "action": "BUY",
        "symbol": "YESBANK",
        "exchange": "NSE",
        "orderid": "250408001002736",
        "product": "MIS",
        "quantity": 1,
        "price": 16.5,
        "pricetype": "LIMIT",
        "order_status": "cancelled",
        "trigger_price": 0.0,
        "timestamp": "2025-04-08 14:13:45"
      }
    ],
    "statistics": {
      "total_buy_orders": 2,
      "total_sell_orders": 0,
      "total_completed_orders": 1,
      "total_open_orders": 0,
      "total_rejected_orders": 0
    }
  }
}
```

## Sample API Response (Analyzer Mode)

```json
{
  "data": {
    "orders": [
      {
        "action": "BUY",
        "average_price": 103.6,
        "exchange": "NFO",
        "filled_quantity": 65,
        "order_status": "complete",
        "orderid": "26100378391337",
        "pending_quantity": 0,
        "price": 103.6,
        "pricetype": "MARKET",
        "product": "NRML",
        "quantity": 65,
        "rejection_reason": "",
        "strategy": "fixtures",
        "symbol": "NIFTY06OCT2622400PE",
        "timestamp": "2026-10-03 09:43:52",
        "trigger_price": 0.0
      }
    ],
    "statistics": {
      "total_buy_orders": 25,
      "total_completed_orders": 28,
      "total_open_orders": 3,
      "total_rejected_orders": 0,
      "total_sell_orders": 9,
      "total_trigger_pending_orders": 2
    }
  },
  "mode": "analyze",
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
| data.orders | array | Today's orders |
| data.statistics | object | Counts over those orders |
| mode | string | `analyze` in analyzer mode; absent in live mode |

### Order Object Fields

| Field | Type | Description |
|-------|------|-------------|
| orderid | string | Order id |
| symbol | string | OpenAlgo symbol |
| exchange | string | Exchange code |
| action | string | BUY or SELL |
| quantity | number | Order quantity (fractional only on CRYPTO) |
| price | number | Limit price, two decimals; 0 for MARKET orders |
| trigger_price | number | Trigger price, two decimals |
| pricetype | string | MARKET, LIMIT, SL or SL-M |
| product | string | MIS, CNC or NRML |
| order_status | string | `open`, `trigger pending`, `complete`, `cancelled`, `rejected` (other broker states in lowercase) |
| timestamp | string | Order time as your broker reports it; `YYYY-MM-DD HH:MM:SS` IST in analyzer mode |
| order_tag | string | Kotak only: the broker's order tag |
| average_price, filled_quantity, pending_quantity, rejection_reason, strategy | | Analyzer mode only |

### Statistics Object Fields

| Field | Type | Description |
|-------|------|-------------|
| total_buy_orders | number | BUY orders |
| total_sell_orders | number | SELL orders |
| total_completed_orders | number | Orders with status `complete` |
| total_open_orders | number | Orders with status `open` |
| total_rejected_orders | number | Orders with status `rejected` |
| total_trigger_pending_orders | number | Analyzer mode only: orders with status `trigger pending` |

## Analyzer Mode

The sandbox lists its own orders, newest first, with the extra fields above and `"mode": "analyze"`. SL and SL-M orders wait as `trigger pending` until their trigger trades. See [Sandbox mode](../../user/sandbox.md).

## Errors

Validation errors on this endpoint carry `message` as an object, for example `{"apikey": ["Missing data for required field."]}`.

| HTTP | `message` | When |
|------|-----------|------|
| 500 | The broker's reason, or `Could not reach the broker. Check your internet connection and try again.` | The broker did not return the order book |
| 501 | `... is not available for your broker.` | Your broker offers no order book call |

Errors common to every endpoint (missing or invalid API key, OpenAlgo locked, wrong method, rate limit) are listed in the [API overview](../README.md#errors-common-to-every-endpoint).

## Notes

- The book covers the current trading day.
- Every row carries the OpenAlgo symbol, never the broker's own trading symbol, so it can be passed straight back to `/modifyorder`, `/cancelorder` or `/orderstatus`.
- Zerodha's stop orders that wait for their trigger appear as `open` in the live book, as on the web.
- Rate limit: 100 requests per second per caller. See [Rate limiting](../rate-limiting.md).

## In the desktop

- In the desktop, `quantity` and the statistics are JSON numbers for every broker. Some of the web's broker adapters return `quantity` as a string (`"1"`) or the counts as decimals (`2.0`). A client that parses them as numbers works with both.

---

**Back to**: [API Documentation](../README.md)
