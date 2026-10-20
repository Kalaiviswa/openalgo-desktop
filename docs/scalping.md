# Scalping terminal

A keyboard-driven terminal for fast intraday trades in options, futures and
equity, with stop-loss, target and trailing protection that OpenAlgo watches
for you. It is the web's Scalping Terminal, carried over; the protection
rules are stricter in the desktop, as described below.

Open it from the profile menu, **Scalping** (`/scalping`).

- [Choosing what to trade](#choosing-what-to-trade)
- [Placing orders](#placing-orders)
- [Stops, targets and trailing](#stops-targets-and-trailing)
- [What watches your stops](#what-watches-your-stops)
- [When a stop or target is hit](#when-a-stop-or-target-is-hit)
- [The protection rules](#the-protection-rules)
- [When the stop list cannot be read](#when-the-stop-list-cannot-be-read)
- [Books, Close All and Cancel All](#books-close-all-and-cancel-all)
- [Sandbox and Live](#sandbox-and-live)
- [Switching brokers, quitting and the next day](#switching-brokers-quitting-and-the-next-day)
- [Differences from OpenAlgo web](#differences-from-openalgo-web)
- [Known gaps](#known-gaps)
- [Developer reference](#developer-reference)

## Choosing what to trade

- **Exchange**: NSE, BSE, NFO, BFO, MCX or CDS.
- **Segment**: Equity (NSE and BSE), Options or Futures (the derivatives
  exchanges).
- **Options**: pick the **Underlying** (indices first, then stocks; type to
  filter), the **Expiry** (the nearest is chosen for you) and the **Call
  Strike** and **Put Strike** (at the money by default; for MCX and CDS from
  the current-month future's price).
- **Futures**: the nearest expiry is chosen for you. **Equity**: search the
  symbol.

Ticker cards show the call, the underlying and the put (or the one
instrument) with their last price and the day's range, live from your
broker's feed. **Charts** adds small charts (1m, 5m or 15m); they are off by
default. The header shows the last order's round trip in milliseconds and the
state of the feed (Live, Polling (REST), Connecting or Disconnected), with a
banner explaining it when the feed drops.

## Placing orders

Every entry is a **MARKET** order.

| Key | Options | Futures and equity |
| --- | --- | --- |
| Up arrow | Buy Call | Buy |
| Down arrow | Sell Call | Sell |
| Right arrow | Buy Put | Buy |
| Left arrow | Sell Put | Sell |
| F6 | Close All Positions | Close All Positions |
| F7 | Cancel All Orders | Cancel All Orders |

- **One-Click** must be on for the arrow keys and the order buttons to place
  orders; it is remembered across reloads. Arrow keys are ignored while you
  type in a field, and a held key does not repeat orders.
- **F6 and F7 always work**, even while you type and with One-Click off. On a
  Mac the function keys may control the screen or the sound instead; press Fn
  with them, or use the on-screen buttons, which always work.
- **Quantity** is in lots for derivatives (the card shows the lot size), up
  to 20 lots per click, and in shares for equity. **Product** is MIS, NRML or
  CNC as the segment allows.
- Orders placed here are your own actions in the app: they follow the
  analyzer switch (sandbox or your broker) and are never queued in the Action
  Center, whatever the order mode.

## Stops, targets and trailing

Two ways to protect a position:

- **Predefined SL and Target** in the order card: tick **SL** and/or
  **Target** and set a value in points (**Pts**) or percent. Every new entry
  gets them.
- **Per position**: **Set SL** (or **Set Call SL** and **Set Put SL**) in the
  order card, or **Risk** on a row of the Positions tab, opens "SL, Target,
  Trailing" for that position: **Stop-loss price**, **Enable trailing
  stop-loss** with a **Trailing step**, and an optional **Target price**.
  The dialog checks the direction (a long position's stop must be below the
  price, a short one's above). **Clear SL** removes it.

Trailing moves the stop only in your favour (up for a long, down for a
short) and starts only once the position is at least one point in profit.
The Positions tab shows each row's SL, TP and TSL.

Stops are kept for positions on NFO, BFO, MCX and CDS, as on the web.

## What watches your stops

OpenAlgo itself, not the page. Stops are saved in OpenAlgo's database, and a
monitor inside the app checks every price for every protected position:

- It keeps working when you leave the scalping page or close a browser tab
  showing it.
- It reads prices from your broker's live feed; each tick drives one check of
  the stops on that symbol. While the broker feed is down, stops are not
  checked; it reconnects by itself, and **Run checks** on the Diagnostics
  page shows its state ([Health monitoring](health-monitoring.md#run-checks-diagnostics)).
- It runs while a broker session is signed in: it starts when you sign in to
  your broker (or the session resumes) and stops when the session ends.
- Trailing changes are saved (at most every 1.5 seconds) and pushed to the
  page, so the stop you see is the stop being enforced.

It does not run while OpenAlgo is closed or the computer sleeps: see
[Switching brokers, quitting and the next day](#switching-brokers-quitting-and-the-next-day).

## When a stop or target is hit

1. The monitor claims the exit under the same lock that checked the price, so
   one breach sends one exit, however many ticks arrive.
2. It reads the position book for what is held now and sends a MARKET exit
   for that quantity, in whole lots and split into orders no larger than the
   exchange's freeze quantity.
3. A Live position's exit goes to your broker directly, marked as already
   decided (`force_live`), so it never waits in the Action Center and is never
   sent to the sandbox because analyzer mode was turned on meanwhile. A
   sandbox position's exit goes to the sandbox.
4. The stop is cleared only when the position is flat (see the rules below).

## The protection rules

These are deliberate departures from the web, which clears a stop as soon as
its exit is accepted. Each comes from a way a real position could otherwise
be left with nothing watching it.

- **An accepted exit is not a flat position.** Protection stays until every
  exit order has ended and the position book shows the position flat. If an
  exit is rejected, cancelled or only partly filled, the stop is still there,
  and a retry sends only the quantity that is neither filled nor still
  working.
- **A position the book does not show is not flat.** A missing row is flat
  only when the position's own exits filled all of it. A row whose quantity
  cannot be read is never flat. A zero row when every exit ended unfilled is
  not trusted either: the book is read again (twice, 300 ms apart, and a zero
  row with no exits gets 30 seconds for the broker's book to catch up). If it
  still cannot be told, the stop stays, no exit is sent, and you are alerted:
  "the broker's position book does not show this position...".
- **A book that cannot be read refuses the exit.** The position stays
  managed; the web read an unreadable book as flat and cleared the stop.
- **An exit is saved before it is sent.** If an exit order cannot be written
  to the database, it is not sent ("its exit could not be saved, so ... was
  not sent"), and the remaining parts of that exit stop. If a stored exit
  cannot be read back, no further exit is sent for that position
  automatically.
- **A decision is acted on only for the stop it was made on.** Each stop has
  a version; the monitor checks the stop's row and version before placing an
  exit and before clearing, so a stop you edited or cleared a moment earlier
  is never acted on with old values.
- **An exit goes only through the account the stop was set on.** A Live stop
  records the broker and account. If OpenAlgo is signed in to another broker
  or account, the exit is not sent there; the stop stays and you are alerted.
- **Each problem is reported once.** Alerts appear on the Health page as
  `scalping_protection` ([Health monitoring](health-monitoring.md#alerts)),
  not repeated on every tick.

An exit the broker accepted but whose order the book no longer shows is
judged like any unconfirmed order: in Live it is held until it is found or you
mark it on the Health page
([Unconfirmed orders](health-monitoring.md#unconfirmed-orders)).

## When the stop list cannot be read

Since #164, if OpenAlgo cannot read the saved stops (for example the database
is busy), the page does not show an empty list as if you had no stops. It
keeps the stops it showed last and says, in the order card:

> Saved stop-losses could not be read; the stops shown may be out of date.
> OpenAlgo keeps enforcing the ones it holds.

The first time, a message also says "Failed to load active stop-losses —
verify manually". The monitor keeps enforcing the stops it holds in memory.
Wait and refresh; if it keeps happening, restart OpenAlgo. (Behind it,
`GET /scalping/api/sl` now answers an error instead of an empty list.)

## Books, Close All and Cancel All

The Positions, Order Book and Trade Book tabs show the instruments this
terminal has traded (the scalping list), since a broker position carries no
tag saying where it came from. Each position row has **Close** (a
freeze-safe exit for that row) and **Risk**.

- **Close All Positions** (F6) closes the positions in the scalping list.
- **Cancel All Orders** (F7) cancels every open order, not only the
  scalping list's, as the API's cancel-all does.

Both work in the mode you are in: sandbox positions in analyzer mode, broker
positions in Live mode.

## Sandbox and Live

- Entries follow the analyzer switch, like every action in the app.
- A stop is saved for the mode you were in when you set it, and the list
  shows the current mode's stops.
- **The monitor watches both modes at once** and exits each position in the
  mode it was opened in: a sandbox position in the sandbox, a Live one at
  your broker. Switching modes does not leave the other mode's positions
  unprotected. The web watches only the current mode.

## Switching brokers, quitting and the next day

- **Switching brokers** is refused while Live positions are under scalping
  protection or scalping orders are not yet settled: "... still has ... 1
  position under scalping protection. They are not moved to another broker.
  First stop the strategies and protection, then switch. Nothing was
  changed." Clear the stops (or close the positions) first. Sandbox
  protection does not block a switch.
- **Quitting** asks first when Live positions are under protection: "Stops
  and targets are not checked until OpenAlgo opens again; then protection
  resumes." Positions stay at your broker either way; nothing is closed.
- **After a restart** the stops are still saved. Protection resumes when you
  sign in to OpenAlgo and your broker session resumes (or you sign in to the
  broker again).
- **The next trading day**: exit orders from before the 03:00 IST boundary are
  treated as ended when the monitor starts, and the broker session ends at
  the boundary like every other.
- **Sleep**: a laptop that sleeps checks nothing until it wakes; then the feed
  reconnects at once. Keep the computer awake while positions are protected.

## Differences from OpenAlgo web

| | OpenAlgo web | OpenAlgo Desktop |
| --- | --- | --- |
| When a stop is cleared | When its exit is accepted | When the position book shows the position flat |
| Unreadable position book | Read as flat; stop cleared | Exit refused; stop kept |
| Modes watched | The current mode only; a mode change between breach and exit skips the exit | Both modes; each position exits in its own mode |
| Live exits | Through the order path | `force_live`, never the Action Center, only through the account the stop was set on |
| Exit orders | Sent | Saved first; an exit that cannot be saved is not sent |
| An unreadable stop list | Shown as empty | Shown as possibly out of date (#164) |
| Problems | Logged | A `scalping_protection` alert on the Health page |

## Known gaps

As of this writing, the One-Click badge in the header reads "One-Click
ARMED" when One-Click is on, a word the desktop's vocabulary rules do not use
on screen; it means One-Click is on.

## Developer reference

| Piece | Where |
| --- | --- |
| Page | `src/pages/Scalping.tsx`, `src/components/scalping/` (`SetSLDialog.tsx`, `StopsUnavailable.tsx`), `src/hooks/useTrailingSL.ts`, `src/lib/scalpingPredefinedStop.ts` |
| Routes | `src-tauri/src/server/routes/scalping.rs` (`/scalping/api/*`, the web's paths and bodies) |
| Service | `src-tauri/src/scalping/service/` (resolution, entries, freeze-safe exits, the scalping list, stops) |
| Store | `src-tauri/src/scalping/store.rs`: `scalping_sl_state` (one row per symbol, exchange, product and mode, with `version`, `exit_attempt`, `broker`, `broker_account`), `scalping_tracked_symbol`; `scalping/unconfirmed.rs` for the page's own unconfirmed orders |
| Monitor | `src-tauri/src/scalping/monitor/` (`mod.rs`, `attempt.rs` for exit attempts and flatness, `exits.rs`, `reconcile.rs`) |
| Risk rules | `src-tauri/src/risk/`, pure, with the web's `test/risk/vectors.json` as the contract (`tests/fixtures/risk/`) |
| Tests | `src-tauri/tests/it/scalping.rs` (including `an_unreadable_stop_list_is_unavailable_not_empty`), `useTrailingSL.ack.test.ts`, `StopsUnavailable.test.tsx` |
