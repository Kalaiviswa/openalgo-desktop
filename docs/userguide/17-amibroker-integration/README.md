# 17 - Amibroker Integration

## Introduction

Amibroker sends orders to OpenAlgo Desktop the same way it sends them to
OpenAlgo web: an AFL formula posts a JSON request to the REST API. The desktop
accepts the same requests and answers with the same responses, so an AFL that
works with OpenAlgo web works here once you change the API key (and the base
URL, if your web instance ran somewhere else).

```
Amibroker AFL  --->  http://127.0.0.1:5000/api/v1/...  --->  OpenAlgo Desktop  --->  your broker
 (signal)             (JSON with your API key)               (or the sandbox, in analyzer mode)
```

There is no Amibroker page in the app. Amibroker talks to the API only.

## What you need

| Item | Where |
| --- | --- |
| Amibroker 6.20 or later | The AFL below uses `InternetSetHeaders` and `InternetPostRequest` |
| OpenAlgo Desktop open and signed in | The API is off until you sign in after OpenAlgo starts |
| Your broker logged in | Select Broker. Orders need a live broker session |
| The API key | **API Key** page (profile menu). The desktop has its own key; a web key does not work here |

## Where Amibroker runs

Amibroker is a Windows program.

- **On the same Windows PC as OpenAlgo Desktop.** Use
  `http://127.0.0.1:5000`. Nothing else to set up.
- **On another computer** (for example Amibroker in a Windows virtual machine
  while OpenAlgo Desktop runs on a Mac or Linux machine). Open **Server
  Settings** in the profile menu, turn on **Allow access from other devices**,
  save and restart OpenAlgo. Then use `http://<that computer's IP address>:5000`
  in the AFL, for example `http://192.168.1.20:5000`. Your firewall may ask
  whether to allow OpenAlgo Desktop; allow it on private networks only. This
  traffic is not encrypted, so use it only on a network you trust. Details:
  [Troubleshooting: another device, a firewall, or a tunnel](../../user/troubleshooting.md#another-device-a-firewall-or-a-tunnel).

If you changed the **App port** in Server Settings, use that port instead of
5000.

## Two rules the API enforces

1. **The API key goes in the JSON body**, as `"apikey"`. A key sent only in a
   header is not read, exactly as on the web.
2. **The request must say it is JSON**: send the header
   `Content-Type: application/json`. A request without it is refused before it
   is read.

## A helper function for AFL

Put this near the top of your AFL. It posts one request and returns the
response text.

```afl
// Post a JSON body to an OpenAlgo endpoint and return the response text.
function OpenAlgoPost(baseUrl, endpoint, body)
{
    InternetSetHeaders("Content-Type: application/json\r\n");
    ih = InternetPostRequest(baseUrl + "/api/v1/" + endpoint, body);
    response = "";
    if (ih)
    {
        line = InternetReadString(ih);
        while (line != "")
        {
            response = response + line;
            line = InternetReadString(ih);
        }
        InternetClose(ih);
    }
    else
    {
        response = "No response from OpenAlgo";
    }
    return response;
}
```

## Placing an order

```afl
_SECTION_BEGIN("OpenAlgo");
apiKey   = ParamStr("API Key", "<APIKEY>");
baseUrl  = ParamStr("OpenAlgo URL", "http://127.0.0.1:5000");
strategy = ParamStr("Strategy", "Amibroker");
exchange = ParamList("Exchange", "NSE|NFO|BSE|BFO|MCX|CDS");
product  = ParamList("Product", "MIS|CNC|NRML");
qty      = Param("Quantity", 1, 1, 100000, 1);
liveOn   = ParamToggle("Send orders", "No|Yes", 0);
_SECTION_END();

function PlaceOrder(action)
{
    global apiKey, baseUrl, strategy, exchange, product, qty;
    body = "{"
        + "\"apikey\":\"" + apiKey + "\","
        + "\"strategy\":\"" + strategy + "\","
        + "\"symbol\":\"" + Name() + "\","
        + "\"exchange\":\"" + exchange + "\","
        + "\"action\":\"" + action + "\","
        + "\"quantity\":\"" + NumToStr(qty, 1.0, False) + "\","
        + "\"pricetype\":\"MARKET\","
        + "\"product\":\"" + product + "\""
        + "}";
    response = OpenAlgoPost(baseUrl, "placeorder", body);
    _TRACE("OpenAlgo placeorder: " + response);
    return response;
}

Buy  = Cross(MA(C, 9), MA(C, 21));
Sell = Cross(MA(C, 21), MA(C, 9));

// Send once per new signal, on the last bar only.
lastSignal = Nz(StaticVarGet(Name() + "_oa_last"));
if (liveOn AND LastValue(Buy) AND lastSignal != 1)
{
    PlaceOrder("BUY");
    StaticVarSet(Name() + "_oa_last", 1);
}
if (liveOn AND LastValue(Sell) AND lastSignal != -1)
{
    PlaceOrder("SELL");
    StaticVarSet(Name() + "_oa_last", -1);
}
```

`Name()` must be the OpenAlgo symbol (for example `SBIN`, or
`NIFTY30JAN25FUT` on NFO). If your Amibroker data feed names symbols
differently, map them in the AFL before you send. See the
[Symbol Format Guide](../symbol-format/README.md).

A successful answer looks like this:

```json
{"orderid": "250408000989443", "status": "success"}
```

## Position-sized orders (smart orders)

`placesmartorder` takes a target position instead of a fresh quantity:
OpenAlgo compares `position_size` with the open position for that symbol and
strategy and sends only the difference. This is the usual choice for a
reversal system.

```afl
function SmartOrder(action, targetSize)
{
    global apiKey, baseUrl, strategy, exchange, product, qty;
    body = "{"
        + "\"apikey\":\"" + apiKey + "\","
        + "\"strategy\":\"" + strategy + "\","
        + "\"symbol\":\"" + Name() + "\","
        + "\"exchange\":\"" + exchange + "\","
        + "\"action\":\"" + action + "\","
        + "\"quantity\":\"" + NumToStr(qty, 1.0, False) + "\","
        + "\"position_size\":\"" + NumToStr(targetSize, 1.0, False) + "\","
        + "\"pricetype\":\"MARKET\","
        + "\"product\":\"" + product + "\""
        + "}";
    return OpenAlgoPost(baseUrl, "placesmartorder", body);
}

// Long: target +qty. Short: target -qty. Flat: target 0.
// SmartOrder("BUY", qty);   SmartOrder("SELL", -qty);   SmartOrder("SELL", 0);
```

More on how smart orders size: [12 - Smart Orders](../12-smart-orders/README.md).

## Running it during market hours

1. Open **Analysis**, load the AFL and choose your watchlist.
2. Turn on **Auto-repeat** and pick an interval (for example 1 minute).
3. Run **Scan** or **Explore**.

Keep OpenAlgo Desktop open and the computer awake for as long as the scan
runs. OpenAlgo trades only while it is open, and after a restart the API stays
off until you sign in again.

## Test in analyzer mode first

Turn on analyzer mode with the mode switch in the top bar (the badge reads
**Analyze Mode**). Every order from Amibroker then goes to the sandbox, priced
from your broker's live data, and the response has the same shape as a live
one. Check **Logs**, **Sandbox Logs** to see each request. Switch back to
**Live Mode** only when the orders look right. See
[15 - Analyzer Mode](../15-analyzer-mode/README.md) and
[Sandbox mode](../../user/sandbox.md).

If **Semi-Auto** is selected on the API Key page, Amibroker's orders wait in
the [Action Center](../22-action-center/README.md) for your approval instead
of going straight to the broker.

## Limits

- 100 API requests a second per caller; order placement 10 a second. A scan
  that fires many orders at once should pace them.
- A quantity must be a whole number, except for crypto orders going to the
  live broker.

## Troubleshooting

| What you see | Cause and fix |
| --- | --- |
| `No response from OpenAlgo` | OpenAlgo Desktop is closed, the address or port is wrong, or (from another computer) access from other devices is off or a firewall blocks it. |
| `Sign in to OpenAlgo to use the API...` | OpenAlgo restarted or you logged out. Sign in to OpenAlgo; the key itself is fine. |
| `Invalid openalgo apikey` | Wrong key, a key from OpenAlgo web, or a regenerated key not yet copied into the AFL. The same answer comes back when no broker is logged in (after 03:00 IST, for example): log in to your broker. |
| An error about missing fields | The JSON is malformed or the `Content-Type` header is missing. `_TRACE` the body and check every quote and comma. |
| Symbol not found | `Name()` is not an OpenAlgo symbol, or the master contract has not loaded. Check the symbol on the **Search** page. |
| Duplicate orders | The AFL sent the same signal more than once. Keep the static-variable check above. |

---

**Previous**: [16 - TradingView Integration](../16-tradingview-integration/README.md)

**Next**: [18 - ChartInk Integration](../18-chartink-integration/README.md)
