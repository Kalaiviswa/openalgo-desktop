# TradeBook

Get today's executed trades: one row per fill.

## Endpoint URL

```http
POST http://127.0.0.1:5000/api/v1/tradebook
```

## Sample API Request

```json
{
  "apikey": "<APIKEY>"
}
```

## Sample cURL Request

```bash
curl -X POST http://127.0.0.1:5000/api/v1/tradebook \
  -H 'Content-Type: application/json' \
  -d '{
  "apikey": "<APIKEY>"
}'
```

## Sample API Response

```json
{
  "status": "success",
  "data": [
    {
      "action": "BUY",
      "symbol": "RELIANCE",
      "exchange": "NSE",
      "orderid": "250408000989443",
      "tradeid": "1234567",
      "product": "MIS",
      "quantity": 1,
      "average_price": 1180.1,
      "timestamp": "2025-04-08 13:58:03",
      "trade_value": 1180.1
    }
  ]
}
```

## Sample API Response (Analyzer Mode)

```json
{
  "data": [
    {
      "action": "BUY",
      "average_price": 103.6,
      "exchange": "NFO",
      "orderid": "26100378391337",
      "price": 103.6,
      "product": "NRML",
      "quantity": 65,
      "strategy": "fixtures",
      "symbol": "NIFTY06OCT2622400PE",
      "timestamp": "2026-10-03 09:43:52",
      "trade_value": 6734.0,
      "tradeid": "TRADE-20261003-094352-ec080f4a"
    }
  ],
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
| data | array | Today's trades |
| mode | string | `analyze` in analyzer mode; absent in live mode |

### Trade Object Fields

| Field | Type | Description |
|-------|------|-------------|
| orderid | string | The order this fill belongs to |
| tradeid | string | The fill's own id (in analyzer mode `TRADE-YYYYMMDD-HHMMSS-<8 hex>`) |
| symbol | string | OpenAlgo symbol |
| exchange | string | Exchange code |
| action | string | BUY or SELL |
| quantity | number | Filled quantity (fractional only on CRYPTO) |
| average_price | number | Fill price, two decimals |
| trade_value | number | Value of the fill, two decimals |
| product | string | MIS, CNC or NRML |
| timestamp | string | Fill time as your broker reports it; `YYYY-MM-DD HH:MM:SS` IST in analyzer mode |
| order_tag | string | Kotak only: the broker's order tag |
| price, strategy | | Analyzer mode only |

## Analyzer Mode

The sandbox lists its own fills with `"mode": "analyze"`. Sandbox orders fill in full, so there is one trade per filled order. See [Sandbox mode](../../user/sandbox.md).

## Errors

Validation errors on this endpoint carry `message` as an object, for example `{"apikey": ["Missing data for required field."]}`.

| HTTP | `message` | When |
|------|-----------|------|
| 500 | The broker's reason, or `Could not reach the broker. Check your internet connection and try again.` | The broker did not return the trade book |
| 501 | `... is not available for your broker.` | Your broker offers no trade book call |

Errors common to every endpoint (missing or invalid API key, OpenAlgo locked, wrong method, rate limit) are listed in the [API overview](../README.md#errors-common-to-every-endpoint).

## Notes

- Only executed trades are listed; pending orders are in the [order book](./orderbook.md).
- A live order can have several fills, each its own row.
- Rows carry OpenAlgo symbols.
- Rate limit: 100 requests per second per caller. See [Rate limiting](../rate-limiting.md).

---

**Back to**: [API Documentation](../README.md)
