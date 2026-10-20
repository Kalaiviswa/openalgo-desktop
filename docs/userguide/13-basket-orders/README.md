# 13 - Basket Orders

## Introduction

A **basket order** places several orders in one request: different symbols,
sides, products or price types, all under one strategy name. A **split
order** does the opposite: it sends one large quantity as several smaller
orders.

Both work as on OpenAlgo web, with the same requests and answers.

## What a basket order is

```
One request -> /api/v1/basketorder
   BUY  1 TCS      MARKET MIS
   SELL 2 HDFCBANK MARKET MIS
   BUY  1 SBIN     LIMIT  CNC at 763.30
-> one answer with a result for each order
```

Instead of three API calls, one. Each order in the basket is still a separate
order at your broker.

## Use cases

| Use | Example |
| --- | --- |
| A portfolio entry | Buy ten stocks of a model portfolio together |
| Sector rotation | Sell one group of stocks and buy another |
| Pairs | Buy one stock and sell a related one |
| Options structures | Buy a call and sell a further call (a spread), or four legs of an iron condor |

For options structures, OpenAlgo also has the options order endpoints (by
offset from the money) and the **Strategy Builder** in **Tools**, whose
**Execute Basket Order** button sends the legs you built as a basket.

## The request

```json
{
  "apikey": "<APIKEY>",
  "strategy": "Rebalance",
  "orders": [
    {"exchange": "NSE", "symbol": "TCS", "action": "BUY", "quantity": 1, "pricetype": "MARKET", "product": "MIS"},
    {"exchange": "NSE", "symbol": "HDFCBANK", "action": "SELL", "quantity": 2, "pricetype": "MARKET", "product": "MIS"},
    {"exchange": "NSE", "symbol": "SBIN", "action": "BUY", "quantity": 1, "pricetype": "LIMIT", "product": "CNC", "price": 763.3}
  ]
}
```

| Field | Where | Meaning |
| --- | --- | --- |
| `apikey`, `strategy` | Top level, once | Your key and a label for every leg |
| `orders` | Top level | The list of orders |
| `exchange`, `symbol`, `action`, `quantity` | Each order | As in a regular order |
| `pricetype`, `product`, `price`, `trigger_price` | Each order | As in a regular order |

An empty `orders` list, or a leg with an invalid value (for example a bad
product), is refused before anything is placed, with a field error naming
the leg.

## The answer

```json
{
  "mode": "analyze",
  "status": "success",
  "results": [
    {"symbol": "TCS", "exchange": "NSE", "product": "MIS", "status": "success", "orderid": "26100392570398", "batch_order": true, "is_last_order": false},
    {"symbol": "SBIN", "exchange": "NSE", "product": "CNC", "status": "success", "orderid": "26100395374263", "batch_order": true, "is_last_order": false},
    {"symbol": "HDFCBANK", "exchange": "NSE", "product": "MIS", "status": "success", "orderid": "26100396568563", "batch_order": true, "is_last_order": true}
  ]
}
```

That answer is from analyzer mode; a live answer has no `mode`, and its
results carry `symbol`, `exchange`, `product`, `status` and `orderid`.

Notice the order of the results: **BUY legs first, then SELL legs**, each
group in the order you sent them.

## How a basket is placed

- **BUY before SELL.** Buy legs go first, then sell legs, as on OpenAlgo web.
  Brokers that give a margin benefit for hedged positions see the protective
  long legs first.
- **Live**: legs are sent in batches of 10 at a time, with a one-second pause
  between batches.
- **Analyzer mode**: the sandbox places the legs one after another, priced
  from one quote request for the whole basket.
- **No all-or-nothing.** Each leg succeeds or fails on its own. Nothing is
  rolled back.

### Partial success

The top-level `status` is `success` whenever the basket was processed, even if
some legs failed. Read each result:

```json
{
  "status": "success",
  "results": [
    {"symbol": "ITC", "exchange": "NSE", "product": "MIS", "status": "success", "orderid": "26100330880794"},
    {"symbol": "NOTASYMBOL", "status": "error", "message": "Symbol NOTASYMBOL not found on NSE"}
  ]
}
```

If a leg fails, the others stay placed. Decide yourself whether to retry the
failed leg or close the ones that went through.

### When the broker does not answer

In the desktop, a live leg whose placement got no definite answer from the
broker (for example the connection dropped after the order was sent) is
reported as an error with "The broker did not confirm whether this order was
placed. It may have been placed: check the order book before placing it
again." Check your order book before sending that leg again; OpenAlgo itself
never sends it twice.

## Python

```python
from openalgo import api

client = api(api_key="<APIKEY>", host="http://127.0.0.1:5000")

basket = [
    {"symbol": "TCS", "exchange": "NSE", "action": "BUY",
     "quantity": 1, "pricetype": "MARKET", "product": "MIS"},
    {"symbol": "HDFCBANK", "exchange": "NSE", "action": "SELL",
     "quantity": 2, "pricetype": "MARKET", "product": "MIS"},
]

response = client.basketorder(orders=basket)

for leg in response.get("results", []):
    if leg["status"] != "success":
        print("Failed:", leg["symbol"], leg.get("message"))
```

## Order types in a basket

Each leg has its own price type.

- **MARKET legs** fill at once and are the simplest for structures that must
  go in together.
- **LIMIT legs** may wait. A basket of limits can leave you half in.
- **Mixed** is allowed: for example, the long leg at market and the short leg
  at a limit.

## Split orders

A split order sends one large quantity as several orders of `splitsize`:

```json
{
  "apikey": "<APIKEY>",
  "strategy": "Accumulate",
  "exchange": "NSE",
  "symbol": "SBIN",
  "action": "BUY",
  "quantity": 10,
  "splitsize": 3,
  "pricetype": "MARKET",
  "product": "MIS"
}
```

Answer: three orders of 3 and one of 1.

```json
{
  "mode": "analyze",
  "status": "success",
  "split_size": 3,
  "total_quantity": 10,
  "results": [
    {"order_num": 1, "orderid": "26100319045249", "quantity": 3, "status": "success"},
    {"order_num": 2, "orderid": "26100322216224", "quantity": 3, "status": "success"},
    {"order_num": 3, "orderid": "26100323761475", "quantity": 3, "status": "success"},
    {"order_num": 4, "orderid": "26100325112150", "quantity": 1, "status": "success"}
  ]
}
```

- `splitsize` is required and must be a positive whole number.
- A split makes at most 100 orders.
- Live, the pieces go one after another, about a tenth of a second apart.
- In the desktop, if one piece gets no definite answer from the broker, the
  rest are not sent: they are reported as "Not sent: an earlier order of this
  split has no definite answer from the broker and may have been placed.
  Check the order book before sending the rest."

Use a split for quantities above an exchange's freeze limit, or to avoid
moving the market with one large order.

## Limits and good practice

| Topic | What to know |
| --- | --- |
| Rate limit | A basket or split is one request, but each leg counts as an order at your broker |
| Margin | Each leg needs margin when it is placed; with brokers that give a hedge benefit, a SELL leg placed after its hedge may need less |
| Semi-Auto mode | The whole basket waits in the Action Center as one item |
| Analyzer mode | Every leg goes to the sandbox |

- Test every new basket in analyzer mode first.
- Check each result's `status`; do not trust the top-level `status` alone.
- Keep baskets to what must go together; unrelated orders are easier to
  manage one by one.

## Basket or individual orders?

| | Individual orders | Basket |
| --- | --- | --- |
| API calls | One per order | One |
| Sequencing | You control it | BUY first, then SELL |
| Failure handling | Per call | Per leg, inside one answer |
| All-or-nothing | No | No |

API reference: [basketorder](../../api/order-management/basketorder.md),
[splitorder](../../api/order-management/splitorder.md).

---

**Previous**: [12 - Smart orders](../12-smart-orders/README.md)

**Next**: [14 - Positions and holdings](../14-positions-holdings/README.md)
