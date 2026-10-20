# LTP (WebSocket)

Stream the last traded price of symbols as it changes: mode 1, the smallest
frames.

Connecting, authenticating, acknowledgements and errors are described in
[WebSocket Streaming](./README.md).

## WebSocket URL

```
ws://127.0.0.1:8765
```

## WebSocket Request

After [authenticating](./README.md#authenticate):

```json
{
  "action": "subscribe",
  "mode": "LTP",
  "symbols": [
    {"exchange": "NSE", "symbol": "RELIANCE"},
    {"exchange": "NSE_INDEX", "symbol": "NIFTY"}
  ]
}
```

`"mode": 1` is the same. Without `mode` a subscription is Quote, not LTP.

## Sample Response

```json
{
  "type": "market_data",
  "symbol": "RELIANCE",
  "exchange": "NSE",
  "mode": 1,
  "data": {
    "symbol": "RELIANCE",
    "exchange": "NSE",
    "mode": "ltp",
    "ltp": 1167.7,
    "ltt": 1791000285768,
    "timestamp": 1791000285768
  },
  "broker": "zerodha"
}
```

## Unsubscribe from LTP

```json
{
  "action": "unsubscribe",
  "mode": "LTP",
  "symbols": [
    {"exchange": "NSE", "symbol": "RELIANCE"}
  ]
}
```

The acknowledgement lists each symbol under `successful` with `"mode": "LTP"`,
or under `failed` with `"mode": null` when the mode was not valid. See
[Unsubscribe](./README.md#unsubscribe).

## Python SDK Example

```python
from openalgo import api
import time

client = api(
    api_key="<APIKEY>",
    host="http://127.0.0.1:5000",
    ws_url="ws://127.0.0.1:8765"
)

instruments = [
    {"exchange": "NSE", "symbol": "RELIANCE"},
    {"exchange": "NSE", "symbol": "INFY"}
]

def on_ltp(data):
    print("LTP Update Received:")
    print(data)

client.connect()
client.subscribe_ltp(instruments, on_data_received=on_ltp)

try:
    time.sleep(60)
finally:
    client.unsubscribe_ltp(instruments)
    client.disconnect()
```

## Message Fields

### Subscribe/Unsubscribe Message

| Field | Type | Description |
|-------|------|-------------|
| action | string | `subscribe` or `unsubscribe` |
| mode | integer or string | `1` or `LTP` in any case |
| symbols | array | Symbol objects. A single `symbol` and `exchange` at the top level also works. |
| request_id | any | Optional, echoed in the acknowledgement |

### Symbol Object

| Field | Type | Description |
|-------|------|-------------|
| exchange | string | Exchange code (NSE, BSE, NFO, NSE_INDEX ...) |
| symbol | string | Symbol in OpenAlgo format |

### Market Data Message

| Field | Type | Description |
|-------|------|-------------|
| type | string | Always `market_data` |
| symbol | string | Symbol |
| exchange | string | Exchange code |
| mode | integer | `1` |
| data | object | LTP data |
| broker | string | The broker that sent the price |

### Data Object

| Field | Type | Description |
|-------|------|-------------|
| symbol | string | Symbol |
| exchange | string | Exchange code |
| mode | string | `ltp` |
| ltp | number | Last traded price |
| ltt | number | Last trade time, epoch milliseconds (the update time when the broker sends none) |
| timestamp | number | Update time, epoch milliseconds |

## Notes

- A frame is sent for every price update the broker streams.
- If the same connection also holds the symbol in Quote or Depth mode, each
  update brings one frame per mode, so you also get these mode-1 frames
  alongside the richer ones.
- In the desktop `ltt` is always present in LTP data. OpenAlgo web's Zerodha
  frames leave it out of LTP frames that come with a Quote or Depth
  subscription.
- Use LTP for price displays, alerts and simple signals.

## Related

- [Quote WebSocket](./quote.md) - OHLC and volume as well
- [Depth WebSocket](./depth.md) - the order book

---

**Back to**: [API Documentation](../README.md)
