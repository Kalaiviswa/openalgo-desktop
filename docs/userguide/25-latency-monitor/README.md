# 25 - Latency Monitor

## Introduction

The Latency Monitor shows how long each API call and each order took inside
OpenAlgo Desktop and at your broker. Use it to see whether a slow fill came
from your broker, from OpenAlgo, or from somewhere before the request reached
OpenAlgo.

Open **Logs**, then **Latency Monitor** (`/logs/latency`). It is also on the
**Admin** page as **Latency Dashboard**. The page title is **Order Latency
Monitor**.

## What is measured

Every call to `/api/v1` is recorded: orders from your programs, alerts and AI
clients, and data calls such as quotes and history. Orders you send from the
scalping terminal, and the close-position and close-all buttons in the app,
are recorded too, as on the web.

What the time covers: from the moment the request reaches OpenAlgo to the
moment the answer leaves it. It does not include the time the request took
to reach your computer (from TradingView's servers through a tunnel, for
example), or the time after the broker accepted the order (the exchange
match).

For an order, the time is split in two:

| Part | Meaning |
| --- | --- |
| **Broker API Call** (broker confirmation) | Time spent waiting for your broker's servers: network round trip, the broker's checks and its hand-off to the exchange |
| **Platform Processing** (platform overhead) | The rest: API key check, validation, symbol lookup, logging and the answer |

In analyzer mode an order never reaches the broker, so its time is all
platform processing. For a data call, the handler's time is shown as the
broker part.

## The page

- **Total Orders Tracked**: every recorded call, all time (data calls
  included, as on the web).
- **Success Rate**: calls that did not end in an error, with the number that
  failed.
- **Average Confirmation Time**: the average total time.
- **Fast Orders**: the share under 150 ms (the target is 95%).
- **Performance Levels**: Excellent under 150 ms, Good 150 to 250 ms,
  Acceptable 250 to 400 ms, Slow over 400 ms.
- **Latency Distribution**: how the recent calls fall into those levels.
- **Broker Performance Comparison**: per broker, the average, the median
  (P50), the worst 1% (P99), the share of fast orders and the count.
  Percentiles cover the last 30 days.
- **Recent Orders**: the latest 100 calls with time (IST), order id, broker,
  symbol, type (PLACE, SMART, MODIFY, CANCEL, CLOSE, BASKET, QUOTES, HISTORY
  and so on), latency and status. The button at the end of a row opens **Order
  Latency Breakdown** with the two parts above.

The page refreshes every 30 seconds; **Refresh** reloads it at once.

## Export

**Export to CSV** downloads every recorded call as `latency_logs.csv`, newest
first, with these columns:

| Column | Content |
| --- | --- |
| Date & Time (IST) | When the call arrived |
| Broker | The broker logged in at the time |
| Order ID | The order id, or the request id, or `unknown` |
| Symbol | The symbol in the request, when there was one |
| Order Type | PLACE, SMART, MODIFY, CANCEL, QUOTES and so on |
| Broker Confirmation (ms) | The broker part |
| Platform Overhead (ms) | The OpenAlgo part |
| Total Latency (ms) | Both |
| Status | SUCCESS or FAILED |
| Error (if any) | The error message for a failed call |

In the app window the file is saved like any download; in a browser it is an
ordinary download. The file is built a page at a time, so a large log
downloads without holding up trading.

## What is kept

The latency log lives in `logs.db` in your data folder.

- **Orders** (place, smart, modify, cancel, close, cancel all, basket, split,
  options, multi-leg and GTT) are kept up to the newest 200,000 rows.
- **Data calls** (quotes, history, funds and the rest) are kept for 7 days.

Old rows are removed every ten minutes. Nothing here holds a request body or
your API key: only the symbol from the request and the order id or message
from the answer.

## Reading the numbers

- **Broker part high**: your broker or your internet connection is slow.
  Compare with other times of day, and with the
  [Health page and diagnostics](../29-troubleshooting/README.md#check-the-health-page-and-run-diagnostics).
- **Platform part high**: the computer is busy (another heavy program, or the
  computer just woke from sleep). One slow call after a long idle period is
  normal.
- **Fills arrive late but latency is low**: the delay is before OpenAlgo (the
  alert service, the tunnel) or after the broker (the exchange). This page
  cannot see either.

---

**Previous**: [24 - PnL Tracker](../24-pnl-tracker/README.md)

**Next**: [26 - Traffic Logs](../26-traffic-logs/README.md)
