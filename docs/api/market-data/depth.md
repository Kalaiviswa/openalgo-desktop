# Depth

Get the order book of a symbol: the best five bid and ask prices with their
quantities, plus the day's prices, volume and open interest.

## Endpoint URL

```http
POST http://127.0.0.1:5000/api/v1/depth
```

## Sample API Request

```json
{
  "apikey": "<APIKEY>",
  "symbol": "SBIN",
  "exchange": "NSE"
}
```

## Sample cURL Request

```bash
curl -X POST http://127.0.0.1:5000/api/v1/depth \
  -H 'Content-Type: application/json' \
  -d '{
  "apikey": "<APIKEY>",
  "symbol": "SBIN",
  "exchange": "NSE"
}'
```

## Sample API Response

```json
{
  "status": "success",
  "data": {
    "asks": [
      {"price": 769.6, "quantity": 767},
      {"price": 769.65, "quantity": 115},
      {"price": 769.7, "quantity": 162},
      {"price": 769.75, "quantity": 1121},
      {"price": 769.8, "quantity": 430}
    ],
    "bids": [
      {"price": 769.4, "quantity": 886},
      {"price": 769.35, "quantity": 212},
      {"price": 769.3, "quantity": 351},
      {"price": 769.25, "quantity": 343},
      {"price": 769.2, "quantity": 399}
    ],
    "high": 774.0,
    "low": 758.15,
    "ltp": 769.6,
    "ltq": 205,
    "oi": 161265750,
    "open": 760.0,
    "prev_close": 746.9,
    "totalbuyqty": 591351,
    "totalsellqty": 835701,
    "volume": 9362799
  }
}
```

With the market closed every level is `{"price": 0, "quantity": 0}`.

## Request Body

| Parameter | Description | Mandatory/Optional | Default Value |
|-----------|-------------|-------------------|---------------|
| apikey | Your OpenAlgo API key | Mandatory | - |
| symbol | Symbol in OpenAlgo format | Mandatory | - |
| exchange | One of NSE, NFO, CDS, BSE, BFO, BCD, MCX, NCDEX, NCO, NSE_INDEX, BSE_INDEX, MCX_INDEX, GLOBAL_INDEX, CRYPTO (upper case) | Mandatory | - |

## Response Fields

| Field | Type | Description |
|-------|------|-------------|
| status | string | "success" or "error" |
| data | object | Market depth data object |

### Data Object Fields

| Field | Type | Description |
|-------|------|-------------|
| asks | array | Five ask (sell) levels, best price first |
| bids | array | Five bid (buy) levels, best price first |
| high | number | Day's high |
| low | number | Day's low |
| ltp | number | Last traded price |
| ltq | number | Last traded quantity |
| oi | number | Open interest (F&O) |
| open | number | Day's open |
| prev_close | number | Previous day's close |
| totalbuyqty | number | Total buy quantity in the order book |
| totalsellqty | number | Total sell quantity in the order book |
| volume | number | Volume traded today |

### Ask/Bid Array Fields

| Field | Type | Description |
|-------|------|-------------|
| price | number | Price level |
| quantity | number | Quantity at this price |

## Understanding Market Depth

```
        BIDS (Buyers)                 ASKS (Sellers)
        --------------               ----------------
Qty     Price                        Price     Qty
886     769.40  <-- Best Bid   Best Ask -->  769.60    767
212     769.35                              769.65    115
351     769.30                              769.70    162
343     769.25                              769.75    1121
399     769.20                              769.80    430
```

## Errors

| HTTP | message | Cause and fix |
|------|---------|---------------|
| 400 | Object of field errors | A field is missing, unknown or not valid. |
| 400 | `Symbol 'XYZ' not found for exchange 'NSE'. Please verify the symbol name and ensure master contracts are downloaded.` | The symbol is not in the master contract. |
| 403 | `Invalid openalgo apikey` | The key is wrong, or no broker is logged in. |
| 500 | The broker's own message | The broker refused the request or could not be reached. |

## Notes

- In the desktop the REST answer always has exactly five levels on each side.
  Levels the broker did not send are filled with `{"price": 0, "quantity": 0}`,
  and levels beyond five are not included. For 20 or 50 levels use the
  [Depth WebSocket](../websocket-streaming/depth.md) with a broker that streams
  them.
- The REST call returns one snapshot. Use the WebSocket for continuous updates.
- Depth always comes from your broker, also in analyzer mode.
- `totalbuyqty` versus `totalsellqty` shows the balance of demand and supply;
  a tight bid-ask spread means a liquid symbol.

## Use Cases

- **Scalping strategies**: see immediate support and resistance
- **Order placement**: pick a limit price from the book
- **Liquidity analysis**: judge how easily you can enter and exit

## Related Endpoints

- [Quotes](./quotes.md) - basic quote data
- [WebSocket Depth](../websocket-streaming/depth.md) - streaming depth

---

**Back to**: [API Documentation](../README.md)
