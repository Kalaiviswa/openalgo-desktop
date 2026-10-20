# WebSockets (verbose control)

The `verbose` parameter of the OpenAlgo Python SDK controls what the SDK
itself prints while it streams market data (LTP, Quote, Depth) and order
updates. It is a setting of your script, not of OpenAlgo: the SDK decides
what to print from the frames it receives, so it works against OpenAlgo
Desktop exactly as it does against OpenAlgo web. OpenAlgo Desktop has no
setting of its own for this, and needs none.

Checked against the `openalgo` package version 2.0.5.

***

### Verbose levels

| Level | Value | What the SDK prints |
| --- | --- | --- |
| Silent | `False` or `0` (default) | Nothing |
| Basic | `True` or `1` | Connection, authentication, subscription and reconnect lines, and errors |
| Debug | `2` | Everything in Basic, plus every LTP, Quote, Depth and order update, and the full server answers |

In version 2.0.5 the SDK prints its error lines from level 1 up, so silent
mode prints no errors either. Use `verbose=True` while setting a script up.

Your own callbacks run at every level; `verbose` changes only what the SDK
prints.

***

### Usage

With OpenAlgo Desktop on its default ports, `host` is all you need: the SDK
derives `ws://127.0.0.1:8765` from it.

```python
from openalgo import api

# Silent (default)
client = api(api_key="<APIKEY>", host="http://127.0.0.1:5000", verbose=False)

# Basic: connection and subscription lines
client = api(api_key="<APIKEY>", host="http://127.0.0.1:5000", verbose=True)

# Debug: every update
client = api(api_key="<APIKEY>", host="http://127.0.0.1:5000", verbose=2)
```

If you changed the **Market data port** in Server Settings, pass it as
`ws_port=` or give the full address as `ws_url="ws://127.0.0.1:<port>"`.

***

## Test example

```python
"""
Test verbose control against OpenAlgo Desktop
"""
from openalgo import api
import time

# Change this to test different levels: False, True, 1, 2
VERBOSE_LEVEL = True

client = api(
    api_key="<APIKEY>",
    host="http://127.0.0.1:5000",
    ws_url="ws://127.0.0.1:8765",
    verbose=VERBOSE_LEVEL,
)

instruments_list = [
    {"exchange": "NSE_INDEX", "symbol": "NIFTY"},
    {"exchange": "NSE", "symbol": "INFY"},
]

def on_data_received(data):
    # Your callback: runs whatever the verbose level
    print(f"MY CALLBACK: {data['symbol']} LTP: {data['data'].get('ltp')}")

print(f"\n=== Testing with verbose={VERBOSE_LEVEL} ===\n")

# Connect and subscribe
if not client.connect():
    raise SystemExit("Could not connect or authenticate; run with verbose=True to see why")
client.subscribe_quote(instruments_list, on_data_received=on_data_received)

# Poll a few times
for i in range(5):
    print(f"\n--- Poll {i+1} ---")
    quotes = client.get_quotes()
    for exch, symbols in quotes.get("quote", {}).items():
        for sym, data in symbols.items():
            print(f"  {exch}:{sym} = {data.get('ltp')}")
    time.sleep(1)

# Clean up
client.unsubscribe_quote(instruments_list)
client.disconnect()
```

***

## Expected output

The prices below are examples.

### verbose=False (silent)

```
=== Testing with verbose=False ===

MY CALLBACK: NIFTY LTP: 26008.5

--- Poll 1 ---
  NSE_INDEX:NIFTY = 26008.5
  NSE:INFY = 1531.0
```

***

### verbose=True (basic)

```
=== Testing with verbose=True ===

[WS]     Connected to ws://127.0.0.1:8765
[AUTH]   Authenticating with API key: xxxxxxxx...xxxxxxxx
[AUTH]   Success | Broker: zerodha | User: <USER_ID>
[SUB]    Subscribing NSE_INDEX:NIFTY Quote...
[SUB]    Subscribing NSE:INFY Quote...
[SUB]    NSE_INDEX:NIFTY | Mode: Unknown | Status: success
[SUB]    NSE:INFY | Mode: Unknown | Status: success
MY CALLBACK: NIFTY LTP: 26008.5

--- Poll 1 ---
  NSE_INDEX:NIFTY = 26008.5
  NSE:INFY = 1531.0
```

The SDK prints the first and last 8 characters of your API key. `User` is
your OpenAlgo username.

`Mode: Unknown` is expected. The server, web and desktop alike, names the
mode in its answer as a word (`"Quote"`), and version 2.0.5 of the SDK looks
it up as a number. The subscription itself succeeded; `Status: success` is
what matters.

***

### verbose=2 (full debug)

```
=== Testing with verbose=2 ===

[WS]     Connected to ws://127.0.0.1:8765
[AUTH]   Authenticating with API key: xxxxxxxx...xxxxxxxx
[AUTH]   Success | Broker: zerodha | User: <USER_ID>
[AUTH]   Full response: {'type': 'auth', 'status': 'success', ...}
[SUB]    Subscribing NSE_INDEX:NIFTY Quote...
[SUB]    Subscribing NSE:INFY Quote...
[SUB]    NSE_INDEX:NIFTY | Mode: Unknown | Status: success
[SUB]    NSE:INFY | Mode: Unknown | Status: success
[SUB]    Full response: {'type': 'subscribe', 'status': 'success', ...}
[QUOTE]  NSE_INDEX:NIFTY      | O: 25998.5    H: 26025.5    L: 25924.15   C: 26008.5    LTP: 26008.5
MY CALLBACK: NIFTY LTP: 26008.5
[QUOTE]  NSE:INFY             | O: 1549.0     H: 1550.6     L: 1525.9     C: 1531.0     LTP: 1531.0
MY CALLBACK: INFY LTP: 1531.0

--- Poll 1 ---
  NSE_INDEX:NIFTY = 26008.5
  NSE:INFY = 1531.0
```

The `C:` column is the frame's `close`: the previous session's close as the
broker reports it, not the current price.

***

## Log categories

| Tag | Level | Meaning |
| --- | --- | --- |
| `[WS]` | 1 | Connected, disconnected, reconnect attempts |
| `[AUTH]` | 1 (full answer at 2) | Authentication sent and its result |
| `[SUB]` | 1 (full answer at 2) | Subscriptions sent, their result, replays after a reconnect, order-update subscription |
| `[UNSUB]` | 1 | Unsubscriptions sent |
| `[LTP]` | 2 | Each LTP update |
| `[QUOTE]` | 2 | Each Quote update |
| `[DEPTH]` | 2 | Each Depth update, followed by the buy and sell ladder |
| `[ORDER]` | 2 | Each order update from `subscribe_orders()` |
| `[ERROR]` | 1 | Errors: connection failures, messages that could not be sent, subscribing before authenticating, errors raised in your callbacks |

The server's error frames (a refused key, an invalid mode) are not printed at
any level. A failure of one instrument inside a subscribe answer is: its
status is on the `[SUB]` line, and its reason in the `Full response` line at
level 2. The next section lists what a silent failure usually means.

***

## Reading the output with OpenAlgo Desktop

Run with `verbose=True` and compare what you see:

- **`[ERROR] Failed to connect to WebSocket server`.** Nothing is listening at
  that address. OpenAlgo Desktop is not running, the **Market data port** in
  Server Settings is not the one in your script, or another program holds the
  port (Server Settings then shows "Live market data is not running").
- **`[AUTH] Authenticating ...` and then nothing, and `connect()` returns
  `False`.** The desktop answered with an error the SDK does not print. With
  OpenAlgo Desktop the usual causes are:
  - the app was restarted, or you logged out of it, and nobody has signed in
    since: until you sign in, the feed checks no key (the API, webhooks and
    MCP are locked the same way). Sign in to OpenAlgo Desktop and run the
    script again;
  - the API key in the script is not the current one (it was regenerated):
    copy it again from the **API Key** page;
  - no broker is logged in: log in to your broker in the app.
- **`[SUB] ... | Status: error`.** That instrument will not stream. Run with
  `verbose=2` to see the reason in the full answer: usually the symbol is not
  in the master contract (`Token not found for ...`; check the
  [symbol format](symbol-format.md)), or your broker has no live feed in the
  desktop.
- **`[WS] Disconnected from ...` followed by `[WS] Reconnect attempt ...`.**
  The desktop closed the connection. Besides network trouble, it does so
  when you sign out of OpenAlgo, when the broker session ends (broker logout,
  the daily session end around 03:00 IST, switching brokers) or a new one
  starts, when the API key is regenerated, and when the app quits or its
  market data port changes. The SDK reconnects by itself, waiting up to a
  minute between attempts, and replays your subscriptions once it is back in
  (`[SUB] Replaying ...`). After a regenerated key it cannot get back in:
  restart the script with the new key.

What the desktop sends and why it closes a connection is described in
[WebSockets](websockets-format.md).
