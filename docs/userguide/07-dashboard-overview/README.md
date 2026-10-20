# 07 - Dashboard Overview

## Introduction

The **Trading Dashboard** is the page you land on after you sign in and your
broker is logged in. It shows your funds, whether the symbol list is ready,
and shortcuts to the pages you use most.

## Opening the dashboard

- After you sign in, when OpenAlgo resumes your broker session from the same
  trading day.
- After a broker login from **Select Broker**.
- Any time: **Dashboard** in the top bar, or the OpenAlgo logo at the left.
- In a browser on the same computer: `http://127.0.0.1:5000/dashboard`.

Without a broker logged in, the dashboard and the other trading pages send
you to **Select Broker**. Only **Profile** and **Server Settings** stay open,
so you can reach your account settings or free a port first.

## Layout

```
+---------------------------------------------------------------------+
| Top bar: pages | broker badge | Live/Analyze badge | switches | menu |
+---------------------------------------------------------------------+
| Trading Dashboard                      Master Contract: Ready (...) |
|                                                                     |
| Available | Collateral | Unrealized | Realized  | Utilised          |
| Balance   |            | P&L        | P&L       | Margin            |
|                                                                     |
| Quick Access                                                        |
| OpenAlgo Symbols   Live Logs   Documentation                        |
| P&L Tracker        OpenVarsity Latency Monitor                      |
+---------------------------------------------------------------------+
| Footer: version, active sessions                                    |
+---------------------------------------------------------------------+
```

The top bar is the same on every page; it is described in
[chapter 08](../08-understanding-interface/README.md).

## The funds cards

Five cards show your account as your broker reports it (or the sandbox, in
analyzer mode):

| Card | Label under the figure | What it is |
| --- | --- | --- |
| **Available Balance** | Cash Balance | Money available for new orders |
| **Collateral** | Total Collateral | Margin from pledged holdings, where your broker reports it |
| **Unrealized P&L** | Mark to Market | Profit or loss on open positions at current prices |
| **Realized P&L** | Booked P&L | Profit or loss from positions closed today |
| **Utilised Margin** | Used Margin | Margin your broker reports as in use |

P&L figures are green when positive and red when negative. Amounts use the
Indian grouping (1,00,000).

The cards show what your broker's funds call returns, the same figures as the
API's `/api/v1/funds`. Brokers differ in what they fill in: a broker that
reports no collateral shows 0.00 there.

## The Master Contract indicator

At the top right: **Master Contract:** with a coloured dot.

| Shows | Meaning |
| --- | --- |
| Checking... (grey) | Asking for the status |
| Downloading... (yellow) | The broker's instrument list is downloading after the login |
| Ready (green), with the symbol count | Symbols are loaded; search, option chains and orders by symbol work |
| Error (red) | The download failed. Hover for the reason |

On an error, open the **Master Contract** page from the profile menu and
choose **Force Download**. If it keeps failing, log in to the broker again.
See [Troubleshooting](../../user/troubleshooting.md#master-contract-errors).

## Quick Access

| Card | Opens |
| --- | --- |
| OpenAlgo Symbols | Symbol search: the OpenAlgo name of any instrument |
| Live Logs | Every API request and its answer, as it happens |
| Documentation | [docs.openalgo.in](https://docs.openalgo.in) in your browser |
| P&L Tracker | Your intraday MTM curve |
| OpenVarsity | OpenAlgo's learning site in your browser |
| Latency Monitor | How long orders and API calls take |

## How the dashboard stays current

- The funds refresh by themselves when an order is placed, filled, modified
  or cancelled, and when a position is closed. There is no polling.
- They refresh when you switch between Live and Analyze mode.
- The Master Contract indicator checks every few seconds until it shows
  "Ready".
- To refresh by hand, open **Dashboard** again from the top bar.

## Analyzer mode on the dashboard

With analyzer mode on:

- the badge in the top bar reads **Analyze Mode**, and the interface turns
  violet;
- the funds cards show the **sandbox** account (1 crore of simulated capital
  to start), not your broker's.

Switching back to Live mode shows your broker's figures again.

## When the dashboard shows a message instead

| Message | Cause | What to do |
| --- | --- | --- |
| **Broker Session Expired** | Your broker session ended (usually at about 03:00 IST) while you were signed in | Choose **Reconnect Broker** and log in again |
| **Session Expired** | Your OpenAlgo sign-in ended | Choose **Go to Login** and sign in |
| An error under the cards | The broker's funds call failed | Check your connection; open **Live Logs** for the reason |

## Common questions

**Why is my balance 0?**
The broker returned no funds yet, or the call failed. Check that the broker is
logged in (its badge is in the top bar), then open the dashboard again. In
analyzer mode you see the sandbox balance instead.

**P&L does not move with the market.**
The cards refresh on order events, not on every tick. For a live P&L curve,
open **P&L Tracker**; for live position P&L, open **Positions**.

**The Master Contract stays on "Downloading..."**
Large brokers take a while on the first download of the day. If it ends in
"Error", see above.

## A daily routine

**Before the market opens**

1. Open OpenAlgo Desktop and sign in.
2. Log in to your broker if the dashboard did not open by itself.
3. Wait for **Master Contract: Ready**.
4. Check the Live or Analyze badge is what you intend.
5. Check **Available Balance**.

**During the day**

- Watch **Positions** and the order book.
- Keep the app open and the computer awake while automation runs.

**After the close**

- Check **Positions** for anything left open.
- Review **Live Logs** or the trade book.
- Log out if you want the API and webhooks off overnight.

---

**Previous**: [06 - Broker connection](../06-broker-connection/README.md)

**Next**: [08 - Understanding the interface](../08-understanding-interface/README.md)
