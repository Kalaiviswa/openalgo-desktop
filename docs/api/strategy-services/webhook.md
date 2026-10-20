# Strategy Webhook

Trigger a strategy from an external alert. The URL token identifies the
strategy, so this endpoint takes no API key.

## Endpoint URL

```http
On this computer:  POST http://127.0.0.1:5000/strategy/webhook/<your_webhook_token>
Through a tunnel:  POST https://<your-tunnel-or-domain>/strategy/webhook/<your_webhook_token>
```

This endpoint is **not** under `/api/v1`, and it does **not** accept an
`apikey` field. The token in the path is the whole credential.

TradingView and other alert services send from the internet, so they can reach
the desktop only through a tunnel you run on this computer (ngrok, Cloudflare
Tunnel and the like) or a custom domain that forwards to it. The app does not
open one for you. A device on your own network can use your computer's network
address once "Allow access from other devices" is on in Server Settings.

## The Token

- It is generated when the strategy is created and looks like `oaws_` followed
  by 43 URL-safe characters.
- **It is shown exactly once**, in the app, when the strategy is created or the
  token is rotated. Copy it then.
- Only its SHA-256 digest is stored. **No endpoint returns it.** If you lose it,
  rotate it on the strategy's Webhook tab and copy the new one.
- Treat it as a credential. Anyone who can post to the URL can start or stop the
  strategy, subject to its kill switch, IP allowlist and live opt-in.
- Rotating makes the old token stop working immediately.

## Batch Strategies

A batch strategy is a multi-leg spread entered and exited as a unit. It accepts
`start` and `stop`, and nothing else.

### Sample API Request (Start)

```json
{
  "action": "start",
  "mode": "sandbox"
}
```

### Sample cURL Request (Start)

```bash
curl -X POST http://127.0.0.1:5000/strategy/webhook/oaws_your_webhook_token_here \
  -H 'Content-Type: application/json' \
  -d '{
  "action": "start",
  "mode": "sandbox"
}'
```

### Sample API Response (Start)

```json
{
  "status": "success",
  "result": "ok",
  "message": "Strategy start accepted",
  "strategy_id": 7,
  "run_id": 42
}
```

### Sample API Request (Stop)

```json
{
  "action": "stop"
}
```

### Sample cURL Request (Stop)

```bash
curl -X POST http://127.0.0.1:5000/strategy/webhook/oaws_your_webhook_token_here \
  -H 'Content-Type: application/json' \
  -d '{
  "action": "stop"
}'
```

### Sample API Response (Stop)

```json
{
  "status": "success",
  "result": "ok",
  "message": "Strategy stop accepted",
  "strategy_id": 7,
  "run_id": 42,
  "stop_pending": true,
  "exits": [
    {
      "leg_id": 1,
      "ok": true,
      "position_ref": "969bc536b1c14d15992f730c2c136d7a",
      "exit_owner": "live",
      "error": null
    }
  ]
}
```

`mode` is required on `start` and ignored on `stop`. A stop that carries a stray
`mode` is not refused for it.

`Strategy stop accepted` means the stop was handed to the engine; it does not
mean the broker is flat. `stop_pending: true` keeps the run current, subscribed
and managed until the exit fills confirm every position is flat. A stop for a
strategy that is not running answers `Strategy stop accepted` with no `run_id`.

## Signal Strategies

A signal strategy moves one leg at a time. It accepts `long_entry`,
`long_exit`, `short_entry` and `short_exit`, and nothing else. There is no
`start` and no `mode`: the first signal after the session boundary opens the
run, and the mode comes from the strategy's own live opt-in (live when live
trading is enabled on the strategy, sandbox otherwise).

The leg is named either by `leg_id` or by `symbol`, optionally narrowed by
`exchange`. `leg_id` wins when both are given.

### Sample API Request (By Leg Id)

```json
{
  "action": "long_entry",
  "leg_id": 1
}
```

### Sample API Request (By Symbol)

```json
{
  "action": "short_exit",
  "symbol": "RELIANCE",
  "exchange": "NSE"
}
```

### Sample cURL Request

```bash
curl -X POST http://127.0.0.1:5000/strategy/webhook/oaws_your_webhook_token_here \
  -H 'Content-Type: application/json' \
  -d '{
  "action": "short_exit",
  "symbol": "RELIANCE",
  "exchange": "NSE"
}'
```

### Sample API Response (Order Placed)

```json
{
  "status": "success",
  "result": "ok",
  "message": "Signal accepted",
  "strategy_id": 4,
  "run_id": 91
}
```

### Sample API Response (No-op)

```json
{
  "status": "success",
  "result": "ok",
  "message": "Signal accepted (already_long)",
  "strategy_id": 4,
  "run_id": 91
}
```

HTTP 200. The signal was understood and correctly did nothing.

## Request Body

| Parameter | Description | Mandatory/Optional | Default Value |
|-----------|-------------|-------------------|---------------|
| action | Batch: `start` or `stop`. Signal: `long_entry`, `long_exit`, `short_entry` or `short_exit`. Trimmed and lower-cased before matching | Mandatory | - |
| mode | `live` or `sandbox`. Batch `start` only. Trimmed and lower-cased before matching | Mandatory on `start`, ignored elsewhere | **None. There is no default** |
| leg_id | Which leg the signal targets. Signal strategies only | Optional | null |
| symbol | Leg symbol, used when `leg_id` is absent. Signal strategies only | Optional | null |
| exchange | Exchange for `symbol`. Signal strategies only | Optional | null |

The body must be a JSON object. A JSON array, a bare string and a number are all
refused. Extra fields are ignored, and are redacted before the audit row is
written if they look like a credential.

## Response Fields

| Field | Type | Description |
|-------|------|-------------|
| status | string | `success` or `error` |
| result | string | The outcome label, always one of the result labels below. Absent on the 413 and on the 503 while OpenAlgo is locked |
| message | string | What happened, in words |
| strategy_id | integer | The strategy the token resolved to. Absent when the token resolved to nothing |
| run_id | integer | The run the signal opened or affected. Absent when there is none |
| stop_pending | boolean | Stop responses only. `true` means the saved stop still has exposure to fill, retry or reconcile; `false` means the stop confirmed flatness |
| exits | array | Stop responses only, present with `stop_pending`; per-position exit outcomes as on [`/stop`](./stop.md) |

`status` and `result` answer different questions. A repeated delivery has
`status: "success"` because your intent was already met, and
`result: "rejected_dedupe"` because the audit trail has to show that this
delivery did nothing.

## Result Labels And Status Codes

| Result | HTTP | Cause |
|---|---|---|
| `ok` | 200 | The signal was accepted and handed to the engine. For a signal strategy this includes a no-op |
| `rejected_token` | 404 | The token is malformed, unknown, or has been rotated: `Unknown or expired webhook token` |
| `rejected_locked` | 403 | The strategy's webhook kill switch is on: `This strategy's webhook is locked` |
| `rejected_ip` | 403 | The caller's address is outside the strategy's IP allowlist: `This address is not allowed to trigger this strategy` |
| `rejected_payload` | 400 | The body is empty, not valid UTF-8, not valid JSON, or not a JSON object |
| `rejected_invalid_action` | 400 | The action is not one this strategy's kind accepts, a `start` named no valid `mode`, or the signal engine refused the signal |
| `rejected_live_disabled` | 403 | `mode: "live"` on a strategy that has not opted into live trading |
| `rejected_dedupe` | 200 | An identical start or stop was already handled within the last 60 seconds. Reported as a success |
| `rejected_cooling_off` | 409 | The strategy stopped within the last 30 seconds, so a `start` is held off: `This strategy stopped recently, try again in <n>s` |
| `rejected_engine_error` | 500 | The engine refused a batch `start` or `stop`, or failed unexpectedly. The message carries the engine's reason |
| `rate_limited` | 429 | A rate limit or the failed-attempt budget refused the request |

Two answers come before the pipeline and carry no `result`:

- **413** `{"status": "error", "message": "Payload larger than 16384 bytes"}`.
  In the desktop this is answered for any body over 16384 bytes, whether its
  size was declared or not, before anything is read into the audit trail. The
  web answers an undeclared oversized body with `rejected_payload` (400) and
  audits it.
- **503** while OpenAlgo is locked (see below).

The 429 body is:

```json
{
  "status": "error",
  "result": "rate_limited",
  "message": "Rate limit exceeded. Please slow down your requests.",
  "retry_after": 60
}
```

In the desktop it carries no `Retry-After` header; read `retry_after`.

Every request admitted to the pipeline, accepted or refused, writes one row to
the webhook audit table. The rate limits, the 413 and the 503 answer before the
pipeline, so they write no audit row. The strategy's page in the app does not
show the webhook audit yet.

## While OpenAlgo Is Locked

In the desktop, the keys OpenAlgo needs are made from your password and are not
kept on disk, so after the app starts, and after you log out, nothing is placed
until you sign in. An alert with a valid token gets 503:

```json
{
  "status": "error",
  "message": "OpenAlgo is locked, so this alert was not acted on. Alerts are refused after OpenAlgo starts and after you log out: sign in to OpenAlgo to receive them again."
}
```

The token is checked first, so only someone holding a valid token learns that
OpenAlgo is locked. Nothing counts against the sender and no audit row is
written. Sign in to the app before the market opens so your alerts are acted
on.

## Validation Order

Before the pipeline, in this order: the per-caller rate limit (this computer
and devices on your network), the 413 size check, the token's own rate limits
and the failed-attempt budget, and the 503 while locked. Then each pipeline
stage writes its own audit row and stops the request:

1. The token resolves to a strategy, or `rejected_token`
2. The strategy's webhook is not locked, or `rejected_locked`
3. The caller is inside the IP allowlist, or `rejected_ip`
4. The body is a JSON object, or `rejected_payload`
5. The action is one this strategy's kind accepts, or `rejected_invalid_action`
6. A batch `start` names a valid mode, or `rejected_invalid_action`
7. A live start is enabled for live, or `rejected_live_disabled`
8. It is not a repeat of the last identical start or stop, or `rejected_dedupe`
9. The strategy is not cooling off, or `rejected_cooling_off`
10. The engine accepts it, or `rejected_engine_error`

The kill switch outranks the allowlist, and the allowlist outranks the payload,
so a body from a refused address is never even parsed. Signal strategies branch
out after stage 5: they have no mode to check, no separate live gate, and no
repeat or cooling-off window.

## The IP Allowlist In The Desktop

- An empty or absent allowlist allows every caller, which is how a strategy is
  created. The strategy page has no allowlist editor yet.
- Entries are CIDR ranges; a bare address is read as its own `/32` or `/128`.
  One malformed entry is skipped rather than failing the whole list.
- **Only a device on your network has an address the desktop can match.** A
  caller through a tunnel, and a program on this computer, have none, so a
  non-empty allowlist refuses them with `rejected_ip`. The web reads the real
  client address from the proxy's headers; the desktop never trusts those
  headers. Through a tunnel, keep the allowlist empty and keep the URL secret:
  the token is what protects the webhook.
- **The webhook locks itself after 10 refused addresses in 10 minutes**, from
  any mix of addresses. It stays locked (`rejected_locked`) until you unlock it
  on the strategy's page, and a critical `webhook_locked` event says `Webhook
  locked after 10 refused alerts from addresses outside the allowlist. Check who
  has the webhook URL, then unlock it or rotate it.` The web has no such lock.

## Notes

- **`mode` is required on a batch `start` and is never defaulted.** A start with
  no mode, or with `paper`, `real` or an empty string, is refused with
  `rejected_invalid_action` (`'mode' must be one of live, sandbox when
  starting`). Case and surrounding spaces do not matter here: `"LIVE"` and
  `" Sandbox "` are read as `live` and `sandbox`. This is the one place the
  webhook is more forgiving than [`/start`](./start.md).
- **Live is opt-in per strategy.** `mode: "live"` is refused with
  `rejected_live_disabled` until you enable live trading on the strategy's page.
  A Live start also needs your broker signed in; without it the start answers
  `rejected_engine_error` with `Broker session is not available or has expired.
  Log in to your broker to restore it.`
- **An unknown token and a malformed one get the same answer**, so the endpoint
  does not reveal which tokens exist. A malformed token is refused on its shape
  before any lookup.
- **Guessing tokens spends a failure budget.** In the desktop, each unknown
  token counts against the sender: after 10 in a minute, that sender's unknown
  tokens are refused with `rate_limited` (429) without a lookup or an audit
  row, while a valid token from the same sender still works. A valid token sent
  from outside its allowlist counts the same way.
- **The 404 counts as a missing page.** In the desktop, an unknown token's 404
  is counted by the missing-page tracker like any other 404, so with automatic
  bans on, a device on your network that keeps sending unknown tokens can be
  banned. This computer and callers through a tunnel are never banned, so the
  address your real alerts arrive from cannot be.
- **A signal that does nothing answers 200 with a note.** A repeat `long_entry`
  on a leg already long, or an exit for a position that is not held, is a
  no-op, not a failure: reporting it as a failure invites a retry, and a retry
  on an order path is how one alert becomes two positions. The notes are
  `already_long`, `already_short`, `flip_pending`, `no_matching_position`,
  `outside_entry_window` and `outside_trading_window`, shown in the message as
  `Signal accepted (already_long)`. A signal arriving while a saved stop is in
  progress is refused instead, with `rejected_invalid_action` and `The signal
  was refused`.
- **Being refused is different from a no-op.** A signal blocked by the
  strategy's `direction`, by the leg's own accepted side, or naming a leg that
  does not exist answers `rejected_invalid_action` with the engine's message,
  for example `This strategy is long_only; a short signal is not accepted`,
  `No leg matches this signal`, or `Leg 1: cash cannot be held short overnight,
  and product CNC carries the position. Use MIS for an intraday short.`
- **A signal leg names its instrument outright, and it is checked** against the
  master contract, cash included. `NIFTY` on `NFO` is a base symbol rather than
  a contract and is refused. If the master contract has no rows at all for that
  exchange, there is nothing to check against and the leg passes.
- **Each kind refuses the other's vocabulary.** `long_entry` against a batch
  strategy and `start` against a signal strategy are both
  `rejected_invalid_action`, and the message names the actions that strategy
  accepts (`'action' must be one of start, stop`).
- **Repeat suppression, batch only.** Two identical `(strategy, action, mode)`
  deliveries inside 60 seconds are one signal (`Duplicate signal ignored,
  already handled within 60s`). A delivery whose engine call failed releases
  its claim, so a genuine retry is not swallowed.
- **Cooling off, batch `start` only.** A strategy that stopped within the last
  30 seconds refuses a `start`, so two alerts firing against each other cannot
  flip it back and forth. A stop is never held by the window, and a stop that is
  still pending does not start it.
- **Body size cap: 16384 bytes.** A TradingView alert is a few hundred bytes.
- **`action` and `mode` are trimmed and lower-cased** before matching. Nothing
  else in the payload is changed.
- **Audit rows for unknown tokens are capped at the newest 1000.** They name no
  strategy, so nothing would ever remove them otherwise.
- The endpoint never fails without an answer. An unexpected failure is logged
  and reported as `rejected_engine_error`.

## Rate Limits

- **Per caller: 100 a minute**, for this computer and for each device on your
  network, checked before the token is even looked up.
- **Per token: 100 a minute**, so one leaked token cannot flood your broker
  account however many senders replay it. The window is keyed on the token's
  digest, never the token.
- **Through a tunnel**, every caller looks the same to the desktop, so a valid
  token is limited on its own window rather than one shared with strangers.
- **Failed attempts: 10 a minute per caller.** See the notes above.

## Where The Token Must Not Appear

- **The traffic log keeps the route, not the token.** The token segment of
  `/strategy/webhook/` paths is replaced with `<redacted>` in the Traffic Logs
  and in the missing-page tracker.
- **It never appears in an audit payload.** The stored payload is the body with
  credential-looking fields and anything containing the token or an `oaws_`
  prefix replaced by `[redacted]`.
- **No API response carries it.**

It will still be in your alert service's configuration, and in any tunnel or
proxy logs outside the app. If it may have leaked, rotate it on the strategy's
page; the old token stops working immediately.

## Use Cases

- **TradingView alerts**: paste the webhook URL into an alert and send
  `{"action": "start", "mode": "sandbox"}` as the message
- **Signal-driven equity trading**: one alert per side per symbol, with the
  strategy holding several unrelated instruments
- **External schedulers**: start in the morning and stop at square-off from any
  HTTP client
- **Sandbox validation**: run the same alerts with `mode: "sandbox"` until the
  strategy is enabled for live

---

**Back to**: [Strategy Module API](./README.md), [API Documentation](../README.md)
