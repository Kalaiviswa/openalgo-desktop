# Sandbox mode (analyzer mode)

Sandbox mode sends every order from the app, the API, webhooks and MCP
clients to a simulated account instead of your broker. Use it to check that a
strategy, webhook or API client sends what you expect before you trade live.
Turn it on with the mode switch in the top bar; the badge shows **Analyze
Mode** while it is on.

The sandbox is an order and account simulator that prices fills from your
broker's live data. It is not an exchange simulator and not a backtester, so
its results are a check of your order flow, not a forecast of live results.

## How orders fill

- **Market orders** fill at the ask when buying and the bid when selling, or
  at the last traded price when the quote has no bid or ask.
- **Limit orders** that are already marketable fill at the last traded price.
  Resting limit orders fill at their limit price once the price trades
  through it.
- **Stop orders** (SL and SL-M) wait until the trigger price trades, then fill
  at the last traded price (an SL only within its limit).
- Prices come from the live feed for every symbol the sandbox needs. When a
  symbol's feed has been quiet for 30 seconds, the sandbox asks your broker
  for a quote every few seconds (the **Order Check Interval** setting) and
  subscribes to that symbol again.

## Limits of the simulation

- **Orders fill in full or not at all.** There are no partial fills, no queue
  position and no market impact. A fill uses the price your broker reports at
  that moment; depth beyond the best bid and ask is not considered.
- **P&L is gross.** Brokerage, exchange charges, taxes (STT, GST, stamp duty)
  and other costs are not deducted from sandbox P&L or funds.
- **Quantities are in contracts.** For a crypto contract the contract value
  (for example 0.001 BTC per BTCUSD contract) applies to its margin, trade
  value and P&L, so 10 BTCUSD contracts at 60,000 have a value of 600.
- **Capital is in rupees; crypto prices are in US dollars.** The sandbox does
  not convert between them: crypto margin and P&L are counted as the same
  number of rupees.
- **CNC buys settle on the next calendar day.** At 00:00 IST, CNC positions
  bought the day before move to holdings, weekends and market holidays
  included.
- **Fills depend on your broker's prices.** With no broker connected, or no
  usable price for a symbol (none at all, one outside the day's high and low,
  or one older than 30 seconds), orders wait instead of filling. Market data
  entitlements and outages at your broker affect the sandbox the same way.
- **Margin is a simple leverage rule** (the leverage settings on the Sandbox
  page), not your broker's SPAN and exposure margin.

## Closing positions

A close sends a MARKET order for the open quantity. When there is no usable
price, that order waits and fills as soon as a price arrives. Close all
positions then reports it as waiting, for example "Closed 0 positions
(1 close order waiting for a price)", and asking again does not place a
second close. The automatic MIS square-off works the same way.

An exit placed against an open position (a stop or a target) blocks no
margin while the position covers it. If another order has already closed the
position when the exit fills, the exit opens a new position and the sandbox
blocks that position's margin at that moment. If your sandbox funds cannot
cover it, the exit is rejected with the reason "Insufficient funds".

## Times and calendars

- MIS positions are squared off at the exchange times on the Sandbox page
  (NSE, BSE, NFO and BFO at 15:15 IST by default). Crypto has no square-off
  and trades at any hour, every day.
- Today's realized P&L resets at 03:00 IST.
- The daily P&L snapshot in My P&L is taken at 23:59 IST on trading days, and
  every day, weekends included, when you held or traded crypto that day. If
  the app was closed at 23:59, that snapshot is filled in when the app is
  started the following day.

## Where to find it

- **Sandbox** (profile menu): capital, leverage, square-off times, and **My
  P&L History**.
- **Logs**, then **Sandbox Logs**: every request made while analyzer mode was
  on.

The sandbox keeps its data in its own database, apart from live trading. A
reset on the Sandbox page clears sandbox orders, trades, positions, holdings,
GTT orders and P&L history, restores the starting capital and puts the
settings back to their defaults.
