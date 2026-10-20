# 12 - Smart Orders

## Introduction

A regular order says what to do: "buy 100". A **smart order** says where you
want to end up: "be long 100". OpenAlgo reads your current position and
places only the order needed to get there. This makes automated systems
simpler and safer, because an alert sent twice, or a reversal, cannot double
your position by accident.

The endpoint is `/api/v1/placesmartorder`. It behaves exactly as on OpenAlgo
web.

## The problem smart orders solve

**With regular orders**, your system must remember its position:

```
Signal: go long.   You are flat.        Send BUY 100.
Signal: go short.  You are long 100.    Send SELL 200 (100 to close, 100 to open).
Signal: go long.   You are short 100.   Send BUY 200.
```

If the system's memory is wrong (a restart, a missed fill, a manual trade),
the orders are wrong.

**With smart orders**, you send the target and OpenAlgo works out the rest:

```
Signal: go long.   Send position_size 100.    OpenAlgo buys what is missing.
Signal: go short.  Send position_size -100.   OpenAlgo sells what is needed.
Signal: exit.      Send position_size 0.      OpenAlgo closes whatever is open.
```

## How OpenAlgo decides

OpenAlgo reads your open position in that **symbol, exchange and product**
(from your broker, or from the sandbox in analyzer mode), compares it with
`position_size`, and acts:

| Current position | `position_size` | What OpenAlgo does |
| --- | --- | --- |
| 0 | 100 | BUY 100 |
| 0 | -100 | SELL 100 |
| 50 | 100 | BUY 50 |
| 150 | 100 | SELL 50 |
| 100 | -100 | SELL 200 |
| -100 | 100 | BUY 200 |
| 100 | 0 | SELL 100 (close) |
| -100 | 0 | BUY 100 (close) |
| 100 | 100 | Nothing: "Positions Already Matched. No Action needed." |
| 0 | 0, with `quantity` above 0 | Places the order as sent (`action` and `quantity` from your request) |

When the position already matches and `quantity` is 0, the answer is "No
OpenPosition Found. Not placing Exit order." Both no-action answers have
`"status": "success"`, so a repeated alert is harmless.

Apart from the last row, the order's side and size come from the difference;
the `action` and `quantity` you send do not change them. Send them anyway: the
API requires both.

### The position is the account's, not the strategy's

OpenAlgo reads the net position your broker holds in that symbol, exchange and
product. The `strategy` name is a label for the logs; it does not keep
positions apart. If two systems trade the same symbol and product, a smart
order from one sees the other's position too. Give them different symbols or
products, or use regular orders.

## The request

```json
{
  "apikey": "<APIKEY>",
  "strategy": "Trend",
  "exchange": "NSE",
  "symbol": "INFY",
  "action": "BUY",
  "quantity": 5,
  "position_size": 15,
  "pricetype": "MARKET",
  "product": "MIS"
}
```

| Field | Required | Meaning |
| --- | --- | --- |
| `apikey` | Yes | Your API key |
| `strategy` | Yes | A label for the logs |
| `exchange`, `symbol` | Yes | The instrument |
| `action` | Yes | `BUY` or `SELL` |
| `quantity` | Yes | 0 or more |
| `position_size` | Yes | The position you want: positive long, negative short, 0 flat |
| `pricetype` | No | `MARKET` (default), `LIMIT`, `SL`, `SL-M` |
| `product` | No | `MIS` (default), `CNC`, `NRML` |
| `price`, `trigger_price` | For LIMIT and stop orders | As in a regular order |

### Answers

Placed (analyzer mode shown; live answers have no `mode`):

```json
{"mode": "analyze", "orderid": "26100319444548", "status": "success"}
```

Nothing to do:

```json
{"message": "Positions Already Matched. No Action needed.", "mode": "analyze", "status": "success"}
```

Missing `position_size`:

```json
{"message": "{'position_size': ['Missing data for required field.']}", "mode": "analyze", "status": "error"}
```

## Examples

### 1. Going long from flat

Position 0. Send `"action": "BUY", "quantity": 10, "position_size": 10`.
OpenAlgo buys 10. Position: +10.

### 2. Adding

Position +10. Send `"action": "BUY", "quantity": 5, "position_size": 15`.
OpenAlgo buys 5. Position: +15.

### 3. Partial exit

Position +15. Send `"action": "SELL", "quantity": 10, "position_size": 5`.
OpenAlgo sells 10. Position: +5.

### 4. Closing

Position +5. Send `"action": "SELL", "quantity": 0, "position_size": 0`.
OpenAlgo sells 5. Position: 0.

### 5. Reversing

Position +100. Send `"action": "SELL", "quantity": 100, "position_size": -100`.
OpenAlgo sells 200. Position: -100.

## Python

```python
from openalgo import api

client = api(api_key="<APIKEY>", host="http://127.0.0.1:5000")

def go_to(position):
    return client.placesmartorder(
        strategy="Reversal",
        symbol="INFY",
        action="BUY" if position >= 0 else "SELL",
        exchange="NSE",
        price_type="MARKET",
        product="MIS",
        quantity=abs(position),
        position_size=position,
    )

print(go_to(10))    # long 10
print(go_to(-10))   # reverse to short 10: sells 20
print(go_to(0))     # flat
```

## TradingView

A strategy alert can send TradingView's own position as the target:

```json
{
  "apikey": "<APIKEY>",
  "strategy": "TradingView Strategy",
  "symbol": "SBIN",
  "exchange": "NSE",
  "action": "{{strategy.order.action}}",
  "product": "MIS",
  "pricetype": "MARKET",
  "quantity": "{{strategy.order.contracts}}",
  "position_size": "{{strategy.position_size}}"
}
```

The **TradingView** page under **Platforms** builds this message for you
(the **Strategy Alert** tab). See
[chapter 16](../16-tradingview-integration/README.md).

## Smart order or regular order?

| | Regular order | Smart order |
| --- | --- | --- |
| You send | What to do | Where to end up |
| Position tracking | Your system | OpenAlgo |
| Same alert twice | Doubles the position | Second one does nothing |
| Reversal | You compute the size | Automatic |
| Several systems on one symbol | Independent | They share one position |

## Things to know

- **Same product.** The position is read for the product you send. A smart
  order with `MIS` does not see a `NRML` position in the same contract.
- **Symbol and exchange must match** the position exactly (`SBIN` on `NSE`
  is not `SBIN` on `BSE`).
- **Arriving together.** Two smart orders for the same position that arrive
  at the same moment are handled one after the other, so both cannot act on
  the same old position.
- **Rate limit.** 10 smart orders per second, as on OpenAlgo web.
- **Semi-Auto mode.** Smart orders wait in the Action Center for approval.
- **Analyzer mode.** The sandbox position is used, and the sandbox places the
  order.

## Troubleshooting

| Answer | Meaning | Check |
| --- | --- | --- |
| "Positions Already Matched. No Action needed." | You already hold `position_size` | Nothing to do |
| "No OpenPosition Found. Not placing Exit order." | `quantity` was 0 and the position already matches | Nothing to do |
| An unexpected size | Another system or a manual trade changed the position | Check **Positions**; remember positions are per symbol, exchange and product, not per strategy |
| `Symbol X not found on NSE` | Wrong symbol or exchange, or the master contract is not loaded | **Search**, and **Master Contract: Ready** |
| "Order queued for approval in Action Center" | Semi-Auto mode | Approve it in **Action Center**, or switch to Auto on the API Key page |

API reference: [placesmartorder](../../api/order-management/placesmartorder.md).

---

**Previous**: [11 - Order types](../11-order-types/README.md)

**Next**: [13 - Basket orders](../13-basket-orders/README.md)
