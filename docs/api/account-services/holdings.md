# Holdings

Get your delivery holdings (CNC) with P&L and portfolio totals.

## Endpoint URL

```http
POST http://127.0.0.1:5000/api/v1/holdings
```

## Sample API Request

```json
{
  "apikey": "<APIKEY>"
}
```

## Sample cURL Request

```bash
curl -X POST http://127.0.0.1:5000/api/v1/holdings \
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
    "holdings": [
      {
        "symbol": "RELIANCE",
        "exchange": "NSE",
        "product": "CNC",
        "quantity": 1,
        "average_price": 1342.0,
        "ltp": 1193.0,
        "pnl": -149.0,
        "pnlpercent": -11.1
      },
      {
        "symbol": "CANBK",
        "exchange": "NSE",
        "product": "CNC",
        "quantity": 5,
        "average_price": 102.7,
        "ltp": 88.9,
        "pnl": -69.0,
        "pnlpercent": -13.44
      }
    ],
    "statistics": {
      "totalholdingvalue": 1637.5,
      "totalinvvalue": 1855.5,
      "totalprofitandloss": -218.0,
      "totalpnlpercentage": -11.75
    }
  }
}
```

## Sample API Response (Analyzer Mode, Nothing Settled Yet)

```json
{
  "data": {
    "holdings": [],
    "statistics": {
      "totalholdingvalue": 0.0,
      "totalinvvalue": 0.0,
      "totalpnlpercentage": 0.0,
      "totalprofitandloss": 0.0
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
| data.holdings | array | Holdings |
| data.statistics | object | Portfolio totals |
| mode | string | `analyze` in analyzer mode; absent in live mode |

### Holding Object Fields

| Field | Type | Description |
|-------|------|-------------|
| symbol | string | OpenAlgo symbol |
| exchange | string | NSE or BSE |
| product | string | CNC |
| quantity | number | Shares held |
| average_price | number | Average buy price |
| ltp | number | Last traded price |
| pnl | number | Profit or loss, two decimals |
| pnlpercent | number | Profit or loss in percent, two decimals; 0 when the average or the last price is missing |
| current_value, settlement_date | | Analyzer mode only: market value, and the day the shares settled (`YYYY-MM-DD`) |

### Statistics Object Fields

| Field | Type | Description |
|-------|------|-------------|
| totalholdingvalue | number | Market value of all holdings |
| totalinvvalue | number | What they cost |
| totalprofitandloss | number | Total profit or loss |
| totalpnlpercentage | number | Total profit or loss in percent |

The totals are your broker's own figures when it reports them, otherwise they are computed from the rows. All are rounded to two decimals.

## Analyzer Mode

Sandbox CNC buys become holdings on the next calendar day (T+1): at 00:00 IST they move from the position book to holdings, weekends and market holidays included. A CNC buy made today is not in holdings yet. See [Sandbox mode](../../user/sandbox.md).

## Errors

Validation errors on this endpoint carry `message` as an object, for example `{"apikey": ["Missing data for required field."]}`.

| HTTP | `message` | When |
|------|-----------|------|
| 500 | The broker's reason, or `Could not reach the broker. Check your internet connection and try again.` | The broker did not return the holdings |
| 501 | `... is not available for your broker.` | Your broker offers no holdings call |

Errors common to every endpoint (missing or invalid API key, OpenAlgo locked, wrong method, rate limit) are listed in the [API overview](../README.md#errors-common-to-every-endpoint).

## Notes

- Holdings are delivery shares, carried across days; intraday and F&O positions are in the [position book](./positionbook.md).
- Rows carry OpenAlgo symbols.
- Delta Exchange has no delivery holdings: its holdings list is always empty, and spot balances appear in the position book.
- Rate limit: 100 requests per second per caller. See [Rate limiting](../rate-limiting.md).

---

**Back to**: [API Documentation](../README.md)
