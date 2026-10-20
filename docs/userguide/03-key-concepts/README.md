# 03 - Key Concepts

## Introduction

These are the words you will meet in OpenAlgo Desktop and in this guide. Each
has a short explanation and what it means in the app.

## Core concepts

### 1. API

An API is how one program asks another to do something. TradingView, your
Python script or Amibroker asks OpenAlgo Desktop to place an order; OpenAlgo
Desktop asks your broker.

```
TradingView -> "BUY 10 SBIN" -> OpenAlgo Desktop (/api/v1/placeorder) -> your broker
```

OpenAlgo Desktop's API is at `http://127.0.0.1:5000/api/v1/` and answers
exactly like OpenAlgo web's.

### 2. API key

Your API key proves that a request comes from you. Every `/api/v1` request
carries it in the JSON body as `apikey`. It is a 64-character code created
with your account; find it on the **API Key** page.

- Keep it secret: anyone with it can trade your account while OpenAlgo
  Desktop is running and you are signed in.
- If it leaks, choose **Regenerate** on the API Key page and update your
  tools.

See [chapter 09](../09-api-key-management/README.md).

### 3. Webhook

A webhook is an address another service calls when something happens, like a
doorbell. TradingView calls it when an alert fires; OpenAlgo Desktop receives
the alert and places the order.

```
http://127.0.0.1:5000/api/v1/placesmartorder        (this computer only)
https://<your-tunnel-address>/api/v1/placesmartorder (reachable from the internet)
```

TradingView, Chartink and GoCharting send webhooks from their own servers on
the internet, so they need an address that reaches your computer. See
[chapter 16](../16-tradingview-integration/README.md).

### 4. Broker session

When you log in to your broker through OpenAlgo Desktop, the broker gives it
a session (an access token) that lets it trade for you.

- Indian brokers end these sessions every night. OpenAlgo Desktop ends the
  broker session at about **03:00 IST**; after that you log in to your broker
  again from **Select Broker**.
- Delta Exchange trades around the clock and its session stays connected
  across 03:00 IST.

### 5. Symbol

OpenAlgo uses one way of writing each instrument, whatever your broker calls
it.

| Instrument | OpenAlgo symbol | Exchange |
| --- | --- | --- |
| Reliance shares | `RELIANCE` | `NSE` |
| State Bank of India shares | `SBIN` | `NSE` |
| The NIFTY 50 index | `NIFTY` | `NSE_INDEX` |
| A NIFTY future | `NIFTY28OCT26FUT` | `NFO` |
| A NIFTY call option, strike 25000 | `NIFTY28OCT2625000CE` | `NFO` |

Futures are `[underlying][expiry]FUT` and options
`[underlying][expiry][strike][CE or PE]`, with the expiry written as
`DDMMMYY`. The contracts above show the format; use a current expiry. Search
for any symbol from the **Search** entry in the profile menu.

### 6. Exchange codes

| Code | Market |
| --- | --- |
| `NSE`, `BSE` | Equity |
| `NFO`, `BFO` | Futures and options on NSE and BSE |
| `CDS`, `BCD` | Currency derivatives |
| `MCX`, `NCDEX` | Commodities |
| `NSE_INDEX`, `BSE_INDEX` | Indices: quotes, charts, and the underlying of an options order |
| `CRYPTO` | Delta Exchange |

## Order concepts

### 7. Price types

| Price type | Meaning |
| --- | --- |
| `MARKET` | Buy or sell now at the market price |
| `LIMIT` | Buy or sell only at your price or better |
| `SL` | Stop-loss limit: when the trigger price trades, a limit order is placed |
| `SL-M` | Stop-loss market: when the trigger price trades, a market order is placed |

```
SBIN trades at 625.

MARKET BUY    buy now, at about 625
LIMIT BUY 620 buy only at 620 or lower
SL-M SELL     trigger 600: sell at market once SBIN trades at 600
```

See [chapter 11](../11-order-types/README.md).

### 8. Product types

| Product | Use it for |
| --- | --- |
| `CNC` | Delivery: shares you keep in your demat account |
| `MIS` | Intraday: closed the same day, squared off automatically near the close |
| `NRML` | Futures and options you carry overnight |

### 9. Actions

An order's action is `BUY` or `SELL`. To go short, `SELL` with no position;
to cover, `BUY`. There are no separate short or cover actions.

## OpenAlgo concepts

### 10. Analyzer mode (sandbox)

A switch that sends every order to a simulated account instead of your
broker. The account starts with 1 crore of capital and fills orders from
your broker's live prices.

```
Analyzer mode on   -> orders go to the sandbox
Analyzer mode off  -> orders go to your broker (Live mode)
```

See [chapter 15](../15-analyzer-mode/README.md).

### 11. Order mode and the Action Center

Set on the **API Key** page:

- **Auto**: orders from the API and webhooks are placed at once.
- **Semi-Auto**: they wait in the **Action Center** until you approve them.

### 12. Strategy name

Every API order carries a `strategy` name, a label you choose, such as
`MA_Crossover`. It shows in the logs so you can tell which system sent which
order. It does not separate positions: your broker holds one position per
symbol, exchange and product, whatever strategy name the orders carried.

### 13. Smart order

An order that says where you want to end up, not what to do. You send the
position you want (`position_size`); OpenAlgo reads your current position and
places the difference.

```
You hold 100 SBIN (long).
Smart order with position_size -100 (short 100).
OpenAlgo sells 200: 100 to close the long, 100 to open the short.
```

See [chapter 12](../12-smart-orders/README.md).

### 14. Basket and split orders

- **Basket order**: several orders, often on different symbols, in one
  request.
- **Split order**: one large quantity sent as several smaller orders of a
  size you choose.

See [chapter 13](../13-basket-orders/README.md).

### 15. Master contract

Your broker's list of every instrument it trades, downloaded after each
broker login and mapped to OpenAlgo symbols. Until it has loaded, symbol
search, option chains and orders by symbol do not work. The **Master
Contract** indicator on the dashboard shows its progress.

## Sign-in concepts

### 16. Your OpenAlgo account and password

You create one account in the app the first time you open it. Your password
does two jobs: it signs you in, and it unlocks the key that encrypts your
saved broker keys, your API key and your two-factor secret.

After OpenAlgo Desktop starts, and after you log out, the API, the market data
feed, webhooks and AI clients stay off until you sign in. See
[chapter 05](../05-first-time-setup/README.md).

### 17. Two sessions

| Session | What it is | When it ends |
| --- | --- | --- |
| OpenAlgo sign-in | You, signed in to the app | When you log out, quit the app, or at about 03:00 IST |
| Broker session | OpenAlgo logged in to your broker | When you log out, switch brokers, or at about 03:00 IST |

After you sign in, OpenAlgo resumes your broker session if it is still from
the same trading day; otherwise it sends you to **Select Broker**.

### 18. Two-factor authentication (TOTP)

An optional 6-digit code from an authenticator app (Google Authenticator,
Authy and others), asked for after your password. Turn it on in **Profile**,
**TOTP** tab. See [chapter 28](../28-two-factor-auth/README.md).

## Market data concepts

### 19. LTP

Last traded price: the price of the most recent trade.

### 20. OHLC

Open, high, low and close: the first, highest, lowest and last price of a
period.

### 21. Market depth

The best buy and sell orders waiting at each price, up to 5 levels (more with
some brokers).

```
   BUY                    SELL
Qty    Price     |   Price    Qty
500    624.00    |   626.00   800
1000   623.95    |   626.05   1200
```

## Quick reference

| Term | In one line |
| --- | --- |
| API | How tools ask OpenAlgo Desktop to act |
| API key | Your secret for the API, sent as `apikey` |
| Webhook | An address another service calls with an alert |
| Broker session | OpenAlgo's login at your broker, ends about 03:00 IST |
| Symbol | `SBIN`, `NIFTY28OCT26FUT`: one format for every broker |
| MARKET, LIMIT, SL, SL-M | The four price types |
| CNC, MIS, NRML | Delivery, intraday, overnight F&O |
| Analyzer mode | The sandbox: simulated orders, live prices |
| Semi-Auto | Orders wait in the Action Center |
| Smart order | Send the position you want; OpenAlgo places the difference |
| Master contract | Your broker's instrument list |
| TOTP | The 6-digit authenticator code |

---

**Previous**: [02 - Why build with OpenAlgo](../02-why-build-with-openalgo/README.md)

**Next**: [03 - System requirements](../03-system-requirements/README.md)
