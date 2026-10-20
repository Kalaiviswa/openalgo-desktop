# Rate Limiting

OpenAlgo Desktop limits how fast a caller may use the API, the same way OpenAlgo web does: a moving window per caller, kept in memory, answered with HTTP 429 when the window is full. It also paces the orders it sends to your broker, so a burst from your scripts never breaks your broker's own limits.

The limits are fixed in the app. There is no `.env` file and no setting to change them.

## API Rate Limits

A caller is one network address: every program on this computer counts as one caller (127.0.0.1), and each device on your network is its own caller. Callers that come through a tunnel (ngrok, Cloudflare Tunnel) share one identity, so behind a tunnel a valid API key gets a window of its own.

| Group | Limit | Endpoints |
|-------|-------|-----------|
| Orders | 10 per second | `/placeorder`, `/modifyorder`, `/cancelorder`, `/optionsorder`, `/optionsmultiorder`, `/placegttorder`, `/modifygttorder`, `/cancelgttorder`, and every `/strategy/*` endpoint |
| Smart orders | 10 per second | `/placesmartorder` |
| Telegram | 30 per minute | `/telegram/*`, except the two below |
| Telegram broadcast | 5 per minute | `/telegram/broadcast` |
| General | 100 per second | Every other `/api/v1` endpoint: market data, account books, symbols, options analytics, `/cancelallorder`, `/closeposition`, `/basketorder`, `/splitorder`, `/margin`, `/telegram/webhook`, `/whatsapp/notify` and the rest |

All paths are under `http://127.0.0.1:5000/api/v1`.

A basket, split or options multi-order counts as one request here, however many orders it places.

## What Happens When a Limit Is Exceeded

The request is refused with HTTP 429 and the limit in the body, as Flask-Limiter answers on the web:

```json
{
  "message": "10 per 1 second"
}
```

The body has no `status` field, and no `Retry-After` or `X-RateLimit-*` header is sent. A refused request is not counted, so a client that waits a moment and tries again gets through once the window has room. The other limits answer `"100 per 1 second"`, `"30 per 1 minute"` and `"5 per 1 minute"`.

## Order Pacing at the Broker

The API limits above decide whether OpenAlgo accepts a request. Separately, OpenAlgo sends orders to your broker no faster than the broker allows, one after another in the order they arrived.

- Brokers that publish an order rate are paced at it (for example Zerodha at 10 orders a second, Angel at 20).
- Brokers that publish none (Samco, Tradejini, HDFC Sky, HDFC Securities, Pocketful, 5paisa) are paced at 10 orders a second, the exchanges' threshold above which a retail API client's algorithm must be registered.
- Orders have their own queue, apart from quotes and history, so a burst of market data requests never holds an exit back.

What a client sees: a burst of orders is queued, so the later ones take a little longer to answer. An order whose turn would come more than 10 seconds later is not sent at all. It is answered at once with HTTP 400:

```json
{
  "status": "error",
  "message": "OpenAlgo is pacing requests to stay within your broker's rate limit, and this one would have had to wait too long for its turn. Try again in a few seconds."
}
```

Nothing reached the broker, so it is safe to send that order again.

Some brokers also count requests per minute or per day (Fyers counts orders, market data and account calls separately, by plan). A request refused for one of those limits gets the broker's own reason.

## Invalid API Keys

Invalid API keys are counted per caller: 10 a minute. A caller that keeps sending wrong keys still gets the same 403 answer, and a device on your network that keeps doing it can be blocked for a while. Programs on this computer are never blocked this way, and a valid key is never refused because of someone else's mistakes. See [Your API client says the API key is invalid](../user/troubleshooting.md#your-api-client-says-the-api-key-is-invalid).

## Webhook Limits

| Endpoint | Limit |
|----------|-------|
| `/strategy/webhook/<token>` | 100 per minute per caller |
| `/chartink/webhook/<webhook_id>` | 100 per minute per caller |
| Wrong webhook addresses | 10 per minute per caller; further wrong ones are refused at once |

## Sign-in Limits

Signing in to OpenAlgo, and broker sign-ins, are limited to 5 a minute and 25 an hour per address. After five wrong passwords or authenticator codes from the same place, each further try waits 30 seconds, doubling up to 5 minutes. Mistakes made elsewhere never slow down signing in on this computer.

## WebSocket Limits

The market data feed on `ws://127.0.0.1:8765` has its own limits, apart from the HTTP ones:

- A client may hold up to 3000 subscriptions.
- A message larger than 1 MiB closes the connection with code 1009.
- A client that sends thousands of messages a second, or megabytes of them, is closed with code 1008 ("too many messages"). Subscribing all 3000 instruments, in one message or one per message, stays far inside it.
- A client that stops reading what it is sent is closed with code 1008 ("client too slow").

See the [WebSocket streaming](./websocket-streaming/ltp.md) pages.

## A Resource Guard

Every caller is also limited to 1000 requests a second across the whole app. Past it, only requests with an invalid credential (or, for pages, without a signed-in session) are refused, with HTTP 429 and `"Too many requests. Please slow down and try again."`. No real client comes near it.

## Recommendations

- Space requests out in your code, or use a client-side rate limiter.
- On a 429, wait a second and try again; back off further if it repeats.
- Use `/basketorder`, `/splitorder` or `/optionsmultiorder` to send many orders in one request.
- Use `/multiquotes` instead of many `/quotes` calls, and the WebSocket feed for live prices instead of polling.

## In the desktop

- In the desktop, one window counts every endpoint of a group together: `/placeorder` and `/cancelorder` share one 10-per-second window per caller, and `/quotes` and `/depth` share the 100-per-second one. The web counts each endpoint in a window of its own.
- In the desktop, the `/strategy/*` endpoints are in the orders group (10 per second). The web applies its general limit to them (100 per second with its sample settings).
- In the desktop, `/optiongreeks` and `/whatsapp/notify` are in the general group (100 per second). The web limits them to 30 per minute each.
- In the desktop, the limits cannot be changed. The web reads them from `.env` (`API_RATE_LIMIT`, `ORDER_RATE_LIMIT`, `SMART_ORDER_RATE_LIMIT`, `WEBHOOK_RATE_LIMIT`).
- In the desktop, orders are paced at the broker for every broker, including those the web does not pace (Samco, Tradejini, HDFC Sky, HDFC Securities, Pocketful, 5paisa), and an order that would wait more than 10 seconds is refused with HTTP 400. The web bounds the wait only on a multi-threaded server and answers 429 there.
- In the desktop, the WebSocket feed limits the messages each client may send; the web reads messages without a limit.
- In the desktop, the Chartink strategy management pages (create, configure, delete) have no request limit of their own; the web limits them to 200 per minute. They need the signed-in session either way. The web's Flow webhook limit has no counterpart: Flow is not in the desktop.

---

**Back to**: [API Documentation](./README.md)
