# 10 - Placing Your First Order

## Introduction

This chapter places your first order, first in analyzer mode (the sandbox),
where nothing reaches your broker, then live. You can place an order three
ways: from the **Playground**, from a trading screen, or from your own code.

## Before you begin

- [ ] OpenAlgo Desktop is open and you are signed in.
- [ ] Your broker is logged in (its badge shows in the top bar).
- [ ] The dashboard shows **Master Contract: Ready**.
- [ ] You know the order basics ([chapter 03](../03-key-concepts/README.md),
      [chapter 11](../11-order-types/README.md)).

## Step 1: Switch to analyzer mode

1. Choose the mode switch in the top bar (next to the **Live Mode** badge).
2. The badge reads **Analyze Mode** and the interface turns violet. A toast
   says "Switched to Analyze mode", then reminds you that analyzer mode is for
   testing.
3. Every order now goes to the sandbox, which starts with 1 crore of
   simulated capital and fills from your broker's live prices.

The switch applies to everything at once: the app, the API, webhooks and AI
clients. See [chapter 15](../15-analyzer-mode/README.md).

## Method 1: The Playground

The Playground sends API requests from inside the app, with your API key
already filled in.

1. Open the profile menu, **API Key**, then **Open Playground**.
2. In the sidebar, under **orders**, choose **PlaceOrder**.
3. The request body appears with your key in `apikey`. Edit it:

   ```json
   {
     "apikey": "<APIKEY>",
     "strategy": "MyFirstOrder",
     "exchange": "NSE",
     "symbol": "SBIN",
     "action": "BUY",
     "product": "MIS",
     "pricetype": "MARKET",
     "quantity": "1",
     "price": "0",
     "trigger_price": "0",
     "disclosed_quantity": "0"
   }
   ```

   | Field | Value | Meaning |
   | --- | --- | --- |
   | `strategy` | `MyFirstOrder` | A label that shows in the logs |
   | `exchange` | `NSE` | National Stock Exchange, equity |
   | `symbol` | `SBIN` | State Bank of India |
   | `action` | `BUY` | Buy |
   | `product` | `MIS` | Intraday: squared off automatically near the close |
   | `pricetype` | `MARKET` | At the market price |
   | `quantity` | `1` | One share |

4. Choose **Send**.
5. The response appears on the right:

   ```json
   {
     "mode": "analyze",
     "orderid": "26100376733573",
     "status": "success"
   }
   ```

   `"mode": "analyze"` confirms the order went to the sandbox. (A live order's
   answer has no `mode` field.)

### Step 2: Check the order

1. **Orderbook**: the order is listed. A market order in the sandbox shows
   **complete** once it fills at the live price.
2. **Tradebook**: the fill, with its price.
3. **Positions**: a new SBIN MIS position of 1, with live P&L.
4. **Dashboard**: the sandbox funds show the margin used.
5. **Logs**, then **Sandbox Logs**: the request and the answer.

Congratulations: you have placed your first order.

## Method 2: From a trading screen

Several screens have an order ticket built in. Each one sends the same order
the API would, so analyzer mode applies here too.

| Screen | Where |
| --- | --- |
| Charting terminal | **Trading** in the top bar: the order dock beside the chart ([chapter 32](../32-charting-terminal/README.md)) |
| Option chain | **Tools**, **Option Chain**: buy or sell any strike |
| Holdings | Profile menu, **Holdings**: **Add** and **Exit** on each row |
| Scalping | Profile menu, **Scalping**: keyboard-driven options orders |

## Method 3: From your own code

### curl

```bash
curl -X POST http://127.0.0.1:5000/api/v1/placeorder \
  -H "Content-Type: application/json" \
  -d '{
    "apikey": "<APIKEY>",
    "strategy": "CurlTest",
    "exchange": "NSE",
    "symbol": "SBIN",
    "action": "BUY",
    "quantity": 1,
    "pricetype": "MARKET",
    "product": "MIS"
  }'
```

### Python SDK

```python
from openalgo import api

client = api(
    api_key="<APIKEY>",
    host="http://127.0.0.1:5000",
)

response = client.placeorder(
    strategy="PythonTest",
    symbol="SBIN",
    action="BUY",
    exchange="NSE",
    price_type="MARKET",
    product="MIS",
    quantity=1,
)
print(response)
# {'mode': 'analyze', 'orderid': '...', 'status': 'success'}
```

Install the SDK with `pip install openalgo`. If you changed the **App port** in
Server Settings, use it in `host`.

## Understanding the answer

### Success

```json
{"mode": "analyze", "orderid": "26100376733573", "status": "success"}
```

| Field | Meaning |
| --- | --- |
| `status` | `success`: the order was accepted |
| `orderid` | The order's id: the broker's id live, a 14-digit sandbox id in analyzer mode |
| `mode` | `analyze` in analyzer mode; absent live |

"Accepted" is not "filled". A limit order can be accepted and stay open; check
the order book.

### Errors

```json
{"message": "Symbol NOTASYMBOL not found on NSE", "mode": "analyze", "status": "error"}
```

| Message | Cause | Fix |
| --- | --- | --- |
| `Symbol X not found on NSE` | Wrong symbol or exchange, or the master contract is not loaded | Find the symbol with **Search**; wait for **Master Contract: Ready** |
| `Quantity must be in multiples of lot size 65` | F&O quantity is not a whole number of lots | Use a multiple of the lot size (Search shows it) |
| `LIMIT orders require price` | A LIMIT order with price 0 | Set `price` |
| `SL orders require trigger_price` | An SL order with no trigger | Set `trigger_price` |
| `{'strategy': ['Missing data for required field.']}` | A required field is missing | Add the field named |
| `Invalid openalgo apikey` | Wrong or regenerated key | Copy the key from the API Key page |
| `Sign in to OpenAlgo to use the API. ...` | OpenAlgo was restarted or you logged out | Sign in to OpenAlgo |
| `Insufficient funds. Required: ...` | Not enough sandbox funds for the margin | Reduce the quantity, or close positions that hold margin |

The lot size in the second row is an example: lot sizes change; check yours.
Live, your broker's own refusals (margin, market closed, a blocked
instrument) come back in the broker's words.

## Practice exercises (analyzer mode)

1. **Market buy**: `BUY 1 SBIN MARKET MIS`. Check the order book and positions.
2. **Limit buy below the market**: `BUY 1 SBIN LIMIT MIS` with `price` well
   below the current price. It stays **open**. Cancel it from the order book.
3. **Sell to close**: `SELL 1 SBIN MARKET MIS`. The position goes to 0 with a
   realized P&L.
4. **Close from Positions**: open a position, then use the close button on its
   row.

## Going live

1. Switch the mode back: the badge reads **Live Mode**.
2. Check the broker badge and that the dashboard shows your real funds.
3. Start with the smallest quantity.
4. Place the order the same way.
5. Check the order book here **and** in your broker's own app or terminal.

## Order checklist

| Check | Why |
| --- | --- |
| Live or Analyze badge | Live sends real orders |
| Symbol and exchange | `SBIN` on `NSE`, options on `NFO` |
| Quantity and lot size | F&O must be whole lots |
| Product | MIS for intraday, CNC for delivery, NRML for F&O overnight |
| Price type | MARKET fills now; LIMIT may wait |
| Strategy name | Helps you read the logs |

## Common first-order mistakes

- **Wrong exchange**: options on `NSE` instead of `NFO`.
- **Wrong lot size**: `quantity` must be a multiple of the lot size for F&O.
- **CNC for F&O**: use `NRML` (or `MIS`) for futures and options.
- **Forgetting the mode**: check the badge before every session.
- **Master contract not ready**: wait for "Ready" after the broker login.

---

**Previous**: [09 - API key management](../09-api-key-management/README.md)

**Next**: [11 - Order types](../11-order-types/README.md)
