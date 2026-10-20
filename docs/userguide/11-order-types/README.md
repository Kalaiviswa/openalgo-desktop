# 11 - Order Types

## Introduction

Every OpenAlgo order has a **price type**, a **product** and an **action**.
This chapter explains each price type with an example and the request that
places it, then products and quantities.

The fields and values are the same as OpenAlgo web's, and the same for every
broker.

## Overview

| Price type | `pricetype` | Needs `price` | Needs `trigger_price` | Fills |
| --- | --- | --- | --- | --- |
| Market | `MARKET` | No | No | Now, at the market price |
| Limit | `LIMIT` | Yes | No | At your price or better, if the market gets there |
| Stop-loss limit | `SL` | Yes | Yes | When the trigger trades, as a limit order at your price |
| Stop-loss market | `SL-M` | No | Yes | When the trigger trades, as a market order |

If you leave `pricetype` out, it is `MARKET`; if you leave `product` out, it
is `MIS`.

## Market order (`MARKET`)

**What it does**: buys or sells at once, at the best price available.

**Example**: SBIN trades at 625. A market BUY of 10 fills now, at about 625.

**Use it when** getting in or out matters more than the exact price: exits,
liquid stocks, fast moves.

| Pros | Cons |
| --- | --- |
| Fills at once in a liquid market | The price can be worse than the last trade (slippage) |
| Simple | Risky in thin or fast markets |

```json
{
  "apikey": "<APIKEY>",
  "strategy": "Example",
  "exchange": "NSE",
  "symbol": "SBIN",
  "action": "BUY",
  "quantity": 10,
  "pricetype": "MARKET",
  "product": "MIS"
}
```

## Limit order (`LIMIT`)

**What it does**: buys at your price or lower, or sells at your price or
higher. It waits in the order book until the market reaches your price, or
until you cancel it.

**Example**: SBIN trades at 625. A LIMIT BUY at 620 waits; if SBIN falls to
620, it fills at 620 or better.

**Use it when** the price matters more than filling now: entries at a level,
targets, illiquid instruments.

| Pros | Cons |
| --- | --- |
| You control the price | It may never fill |
| No slippage | It can miss a fast move |

```json
{
  "apikey": "<APIKEY>",
  "strategy": "Example",
  "exchange": "NSE",
  "symbol": "SBIN",
  "action": "BUY",
  "quantity": 10,
  "pricetype": "LIMIT",
  "price": 620,
  "product": "MIS"
}
```

A LIMIT order with no price, or price 0, is refused: "LIMIT orders require
price".

## Stop-loss limit order (`SL`)

**What it does**: waits until the market trades at your **trigger price**,
then becomes a limit order at your **price**.

**Example**: you hold SBIN bought at 625 and want out if it falls to 610.
SELL SL with trigger 610 and price 608: when SBIN trades at 610, a limit sell
at 608 is placed. It fills at 608 or better, if the market is there.

**Use it for** stop-losses where you want a worst price, and for breakout
entries (a BUY SL above the market).

### Trigger and price

| Order | Trigger | Price | Example |
| --- | --- | --- | --- |
| BUY SL (breakout entry, or stop on a short) | Above the market | At or above the trigger | Trigger 630, price 632 |
| SELL SL (stop on a long) | Below the market | At or below the trigger | Trigger 610, price 608 |

The gap between trigger and price gives the order room to fill in a fast
move. With no gap, it may trigger and not fill.

```json
{
  "apikey": "<APIKEY>",
  "strategy": "Example",
  "exchange": "NSE",
  "symbol": "SBIN",
  "action": "SELL",
  "quantity": 10,
  "pricetype": "SL",
  "price": 608,
  "trigger_price": 610,
  "product": "MIS"
}
```

An SL order with no trigger price is refused: "SL orders require
trigger_price".

## Stop-loss market order (`SL-M`)

**What it does**: waits until the market trades at your trigger price, then
becomes a market order.

**Example**: SELL SL-M with trigger 610 on your SBIN long: when SBIN trades
at 610, a market sell is placed and fills at whatever price is available.

**Use it when** getting out matters more than the price.

### SL or SL-M?

| | SL | SL-M |
| --- | --- | --- |
| After the trigger | Limit order at your price | Market order |
| Fills | Only at your price or better | Almost always |
| Risk | May not fill in a gap or fast move | Price can be well past the trigger |

```json
{
  "apikey": "<APIKEY>",
  "strategy": "Example",
  "exchange": "NSE",
  "symbol": "SBIN",
  "action": "SELL",
  "quantity": 10,
  "pricetype": "SL-M",
  "trigger_price": 610,
  "product": "MIS"
}
```

Some brokers and exchanges restrict SL-M orders on certain segments (for
example F&O). If your broker refuses one, use SL with a wider gap.

## How each type fills in analyzer mode

In analyzer mode the sandbox fills orders from your broker's live prices:

| Type | Sandbox fill |
| --- | --- |
| MARKET | At the ask when buying and the bid when selling, or at the last price when the quote has no bid or ask |
| LIMIT already at or through the market | At the last traded price |
| LIMIT resting | At your limit price, once the market trades through it |
| SL, SL-M | When the trigger trades, at the last traded price (an SL only within its limit) |

Orders fill in full or not at all. See
[Sandbox mode](../../user/sandbox.md#how-orders-fill).

## Products

| Product | `product` | Use it for | Held |
| --- | --- | --- | --- |
| Intraday | `MIS` | Day trades, with your broker's intraday leverage | Squared off automatically near the close |
| Delivery | `CNC` | Shares you buy to keep | In your demat account |
| Normal | `NRML` | Futures and options carried overnight | Until expiry or you close it |

- Use `CNC` for equity delivery only; for F&O use `NRML` or `MIS`.
- Your broker squares off MIS positions at its own cut-off time. In analyzer
  mode the sandbox does it at the times on the **Sandbox** page (15:15 IST for
  NSE, BSE, NFO and BFO by default).
- Selling CNC shares you do not hold is refused.

## Actions

`action` is `BUY` or `SELL` (upper or lower case). There is no separate SHORT
or COVER: sell with no position to go short, buy to cover.

## Quantities

- **Equity**: any whole number of shares.
- **F&O and commodities**: a whole number of lots. "Quantity must be in
  multiples of lot size 65" means the lot is 65. **Search** shows each
  contract's lot size.
- **Crypto (Delta Exchange)**: quantities are in contracts.
- **Fractional quantities** are accepted only on the `CRYPTO` exchange. In
  the desktop they are sent to Delta Exchange live; in analyzer mode, and on
  every other exchange, orders take whole numbers.
- **Very large quantities**: in the desktop a quantity above 2,147,483,647 is
  refused before anything is sent ("Quantity is larger than any exchange
  accepts."), since brokers take a 32-bit quantity.
- **Disclosed quantity** (`disclosed_quantity`, optional): how much of the
  order the exchange shows at a time, where your broker supports it.

## Validity

The OpenAlgo order has no validity field: orders go to your broker as day
orders, and an open order that has not filled by the end of the trading day
is cancelled. For an order that waits across days, use a GTT order
([API reference](../../api/order-management/placegttorder.md)), where your
broker supports it.

## Price type reference

| Field | MARKET | LIMIT | SL | SL-M |
| --- | --- | --- | --- | --- |
| `price` | 0 or omitted | Required | Required | 0 or omitted |
| `trigger_price` | 0 or omitted | 0 or omitted | Required | Required |

## Common mistakes

**An SL order triggers at once.** The trigger is already on the wrong side of
the market (a SELL SL trigger above the last price, or a BUY SL trigger below
it). Put the trigger beyond the current price in the direction you are
protecting.

**A limit order never fills.** The price is too far from the market. Check
the LTP and the depth, or use MARKET.

**Wrong product.** CNC for F&O, or MIS for a position you meant to keep
overnight. Check before placing.

**Lot size.** F&O quantities must be whole lots; the lot size changes from
time to time.

## Quick decision guide

```
Do you need to fill now?
  Yes -> MARKET
  No  -> Is this a stop-loss or a breakout entry?
           Yes -> Must it fill at any price?
                    Yes -> SL-M
                    No  -> SL
           No  -> LIMIT
```

---

**Previous**: [10 - Placing your first order](../10-placing-first-order/README.md)

**Next**: [12 - Smart orders](../12-smart-orders/README.md)
