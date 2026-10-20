# 22 - Action Center

## Introduction

The Action Center holds orders for your approval. With the order mode set to
**Semi-Auto**, a new order from a program that uses your API key is not sent
to the broker: it waits in the Action Center until you approve or reject it.

```
Auto mode       signal ---> order sent at once
Semi-Auto mode  signal ---> Action Center (pending) ---> you approve ---> order sent
                                                    \--> you reject  ---> nothing sent
```

Use it while you are building trust in a new strategy, or whenever you want a
person to look at every order before it reaches the market.

## Turning Semi-Auto on

1. Open the **API Key** page (profile menu).
2. Under **Order Execution Mode**, choose **Semi-Auto**. The note below the
   buttons then reads "Current mode: Semi-Auto - All orders will be queued in
   Action Center for approval".
3. To go back, choose **Auto Mode**.

The Action Center page has a **Toggle Order Mode** button that takes you to
the same place.

## What waits and what does not

Semi-Auto holds every **new order sent with your API key**. That includes
some order windows inside the app, because they place orders through the same
API, exactly as on the web.

| Source | In Semi-Auto |
| --- | --- |
| Programs using your API key: Python scripts, Amibroker, Excel, TradingView or GoCharting alerts sent to `/api/v1`, AI clients (MCP) | New orders wait for approval |
| Chartink strategies | New orders wait for approval (they use the API path) |
| New orders from the app's order windows: the Place Order dialog, the Strategy Builder's basket, Arbitrage, and Buy and Sell on the charting terminal | Wait for approval. On the chart the order is reported as "placeorder returned no orderid"; it is not lost, it is waiting in the Action Center |
| A Live OpenScript run started from the charting terminal | Its orders wait for approval, and the run follows each one (below) |
| Close, cancel and modify buttons on the app's pages (Positions, Order Book, the terminal's bottom dock), and the scalping terminal | Sent at once. They are you acting in the app |
| The strategy module's Live runs (the **Strategies** page) | Sent at once. A stop-loss exit that waits for a person is not a stop loss |

Orders that wait: place order, smart order, basket, split, options order,
options multi-order and GTT placement.

For an API client, Semi-Auto **refuses** modify, cancel, cancel all, close
position, GTT modify and GTT cancel, with a message such as "Cancel order
operation is not allowed in Semi-Auto mode. Please switch to Auto mode to
cancel orders." A queued cancel could reach a market that has moved on. Do
these with the app's own buttons (Order Book, Positions, the terminal's dock),
or switch to Auto mode. Dragging or cancelling an order line on the chart
itself goes through the API and is refused the same way. In analyzer mode
these calls are not refused.

Read-only calls (order book, positions, funds and so on) are never affected.

### If OpenAlgo cannot read the order mode

If the order mode cannot be read, OpenAlgo refuses the API client's order
instead of sending it, answering: "OpenAlgo could not read whether your orders
wait for approval in the Action Center (Semi-Auto), so nothing was sent. Try
again in a moment; if it keeps happening, restart OpenAlgo." The same applies
to the calls Semi-Auto refuses. The app's close, cancel and modify buttons,
scalping, and protective exits are not held up by it.

In the desktop, an unreadable order mode refuses the order. OpenAlgo web
sends it at once instead.

## The Action Center page

Open **Action Center** from the profile menu (it sits just after API Key).

- **Tabs**: **Pending** (with a count), **Approved**, **Rejected** and **All
  Orders**.
- **Counters**: Pending Approval, Buy Orders, Sell Orders, Approved, Rejected.
- **Orders table**: strategy (with the kind of order under it), symbol,
  exchange, action, quantity, price, order type, product and when it arrived.
  An options multi-order whose legs differ shows MULTI as its action; open a
  basket or multi-leg order to see each leg.
- The arrow at the start of a row's actions opens **Order Details**: every
  field of the original request (your API key is removed before the order is
  stored).

While the page is open, a new pending order shows a notice ("New Order
Queued") and plays a sound; the list refreshes by itself. Turn the notice and
sound on or off in **Profile**, **Alerts** (category **Action Center**). The
notice appears only on this page, so keep it open while you expect orders.

## Approving and rejecting

- **Approve** (the green tick on a row): the order is sent the moment you
  approve it. The answer names the broker's order id, or says "Order approved
  but execution failed" with the broker's reason.
- **Approve All (n)**: approves and sends every pending order, one after the
  other, and reports how many were sent and how many failed.
- **Reject** (the red cross): the order is never sent. It moves to Rejected.
- **Delete** (the bin, on an approved or rejected row): removes the row from
  the list after you confirm. A pending order cannot be deleted; reject it.

There is no edit. If an order needs a different price or quantity, reject it
and send a corrected one from your program, or place it yourself.

### Where an approved order goes

- **Analyzer mode at the moment you approve** decides sandbox or live, as on
  the web. Check the mode badge before you approve.
- An order a **Live OpenScript run** queued always goes to the live broker,
  and only through the broker account the run was started on. If OpenAlgo is
  now connected to another broker or account, the order is refused and nothing
  is sent; the row is marked rejected with the reason.

### Approved once, sent once

Approving is safe to repeat. An order is sent at most once even if you click
twice or approve it on two screens at the same time ("This order was already
approved or rejected" or "This order is already being sent to your broker.
Check the order book before approving it again").

A row may show:

- **Sending**: OpenAlgo has started sending it and has no answer yet. A large
  split or basket can take a while.
- **Status unclear**, with "This order may still be sending": there is still
  no broker answer after two minutes. Sending may still be running, or an
  interruption may have left the outcome unknown. Check your broker's order
  book and refresh. Do not place the order again while its status is unclear;
  OpenAlgo never sends it again by itself.

## Semi-Auto with an OpenScript Live run

When a Live OpenScript run places an order in Semi-Auto, the run receives the
pending order instead of a broker order, and then follows the Action Center's
record of it on every refresh:

| In the Action Center | What the run does |
| --- | --- |
| Still pending, or approved and not yet sent | Waits |
| Approved and placed (the broker gave an order id) | Tracks that order like one it sent itself: counts its fills and cancels it on Stop |
| Rejected by you, or approved but refused (by OpenAlgo or the broker) | Records it as rejected and tells you on its page |
| Approved and sent with no definite answer, or the record is gone | Marks it unconfirmed; OpenAlgo settles it from the broker's order book |

OpenAlgo web records such an order with no order id and never follows it;
following it is a desktop addition.

## Good practice

- Approve quickly. A pending MARKET order fills at the price when you approve
  it, not when the signal fired.
- Check the **Live Mode** or **Analyze Mode** badge before **Approve All**.
- Use **Rejected** to review what you turned down and why your strategy sent
  it.

## Troubleshooting

| What you see | Cause and fix |
| --- | --- |
| Orders are not appearing | The mode is Auto; or the order came from a close, cancel or modify button, scalping, or the strategy module, which never wait. |
| "No orders found" on Pending | Nothing is waiting. The empty list links to the API Key page if Semi-Auto is off. |
| An API client says modify or cancel "is not allowed in Semi-Auto mode" | Expected. Do it in the app, or switch to Auto. |
| "Order approved but execution failed" | The broker refused it, or no broker is logged in ("Authentication failed"). Read the reason, fix it, then send a new order. |
| A row stays at **Status unclear** | Check the broker's order book. OpenAlgo's order reconciler also settles it from the book. |
| An OpenScript order was refused after approval | OpenAlgo is connected to a different broker or account from the one the run started on. Sign in to that account. |

---

**Previous**: [21 - Flow Visual Builder](../21-flow-visual-builder/README.md)

**Next**: [23 - Telegram Bot](../23-telegram-bot/README.md)
