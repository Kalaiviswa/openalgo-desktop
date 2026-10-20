# Telegram REST API

The Telegram namespace at `/api/v1/telegram/` manages OpenAlgo Desktop's
built-in Telegram bot: its settings, starting and stopping it, the linked
Telegram users, a broadcast to them, command statistics, notification
preferences, and a direct notification to one user.

In the desktop the bot is part of the app (it talks to the Telegram Bot API
directly; no Python). It **polls** Telegram for messages, so it needs only an
outgoing internet connection. You do not need a tunnel or a public address for
Telegram, even when your desktop is not reachable from the internet.

Setting up the bot (BotFather token, Start, linking your Telegram account with
`/link <api_key> <host_url>` in a private chat with the bot) is done on the
**Telegram** page of the app. For a full walk-through of what to expect, see
[Telegram and WhatsApp: live test plan](../../port/messaging-manual-test.md).

## Authentication

| Request | Where the key is read from |
|---|---|
| GET | the `X-API-KEY` header, or the `apikey` query parameter |
| POST | `apikey` in the JSON body, or the `X-API-KEY` header |
| POST `/webhook` | no API key; Telegram's secret header (see [Webhook](#post-apiv1telegramwebhook)) |

- A missing or wrong key answers **401**:
  `{"status": "error", "message": "Invalid or missing API key"}`. This is the
  web's behaviour for this namespace. The order and data endpoints differ:
  they read the key from the body only and answer a wrong key with 403.
- A valid key is enough. No broker login is needed.
- After OpenAlgo Desktop starts, and after you log out, every `/api/v1` route
  answers **503** until you sign in again, because the keys that check API keys
  are protected by your password. The Telegram bot is also stopped then; if it
  was started, it starts again by itself when you sign in. See
  [How the keys are protected](../../port/no-keychain.md).

## Endpoint inventory

| Method | Path | What the desktop does |
|---|---|---|
| GET | `/api/v1/telegram/config` | Returns the bot settings with the token shortened |
| POST | `/api/v1/telegram/config` | Saves the token, the broadcast switch and the rate-limit setting |
| POST | `/api/v1/telegram/start` | Checks the saved token with Telegram and starts polling |
| POST | `/api/v1/telegram/stop` | Stops the bot |
| GET | `/api/v1/telegram/users` | Lists linked Telegram users, optionally filtered |
| POST | `/api/v1/telegram/broadcast` | Sends a message to every linked user with notifications on |
| POST | `/api/v1/telegram/notify` | Sends one message to the Telegram user linked to an OpenAlgo username |
| GET | `/api/v1/telegram/stats` | Command statistics for the last 1 to 365 days |
| GET | `/api/v1/telegram/preferences` | Reads a linked user's notification preferences |
| POST | `/api/v1/telegram/preferences` | Updates a linked user's notification preferences |
| POST | `/api/v1/telegram/webhook` | Checks Telegram's secret header and acknowledges the update |

## GET /api/v1/telegram/config

```bash
curl 'http://127.0.0.1:5000/api/v1/telegram/config' \
  -H 'X-API-KEY: <APIKEY>'
```

```json
{
  "status": "success",
  "data": {
    "bot_token": "1234567890...",
    "token": "1234567890...",
    "is_active": true,
    "bot_username": "my_openalgo_bot",
    "max_message_length": 4096,
    "rate_limit_per_minute": 30,
    "broadcast_enabled": true,
    "created_at": "Wed, 07 Oct 2026 10:00:00 GMT",
    "updated_at": "Wed, 07 Oct 2026 10:05:00 GMT"
  }
}
```

- The token shows its first 10 characters followed by `...`.
- `is_active` is `true` while the bot is started. It is saved, so it survives
  a restart.
- `created_at` and `updated_at` are HTTP dates in GMT, or `null` before the
  settings were first saved.

In the desktop, both `bot_token` and `token` are shortened. The web shortens
`bot_token` but returns the whole token under `token`.

Errors: 500 `Failed to get bot configuration`.

## POST /api/v1/telegram/config

```bash
curl -X POST 'http://127.0.0.1:5000/api/v1/telegram/config' \
  -H 'Content-Type: application/json' \
  -d '{
    "apikey": "<APIKEY>",
    "token": "<BOT_TOKEN_FROM_BOTFATHER>",
    "broadcast_enabled": true,
    "rate_limit_per_minute": 30
  }'
```

```json
{"status": "success", "message": "Bot configuration updated"}
```

| Field | Type | Notes |
|---|---|---|
| `token` | string or null | The whole token from BotFather. `null` or `""` clears it. |
| `broadcast_enabled` | boolean | Turns [broadcast](#post-apiv1telegrambroadcast) on or off. |
| `rate_limit_per_minute` | integer, 1 to 120 | Stored and shown; it does not throttle sends (the same on the web). |
| `webhook_url`, `polling_mode` | any | Accepted and ignored, as on the web. The bot always polls. |

Fields you leave out keep their saved value.

| Status | Message |
|---|---|
| 400 | `rate_limit_per_minute must be between 1 and 120` |
| 400 | `rate_limit_per_minute must be an integer` |
| 400 | `Send the whole bot token from BotFather, not the shortened one the settings show.` |
| 400 | `The bot token must be text.` |
| 500 | `Failed to update bot configuration` |

In the desktop, a token ending in `...` (the shortened form `GET config`
shows) or a token that is not text is refused, so it cannot overwrite the real
one. The token is stored encrypted.

## POST /api/v1/telegram/start

The body is optional. With no body, send the key in `X-API-KEY`.

```bash
curl -X POST 'http://127.0.0.1:5000/api/v1/telegram/start' \
  -H 'X-API-KEY: <APIKEY>'
```

```json
{"status": "success", "message": "Bot started successfully"}
```

Start checks the saved token with Telegram, then starts polling and waits up
to 5 seconds for the first poll. When Telegram cannot be reached for the
check, the token is kept and checked again as the bot connects.

| Status | Message |
|---|---|
| 400 | `Bot token not configured` |
| 500 | `Telegram did not accept this bot token. Copy it again from BotFather and save it.` |
| 500 | `Bot is already running` |
| 500 | `The bot is still starting or stopping. Check its status in a few seconds.` |
| 500 | `Bot failed to start within timeout` |

The web has a separate "webhook mode" in which start only initialises the bot.
The desktop has none: start always polls, and it removes any webhook set on the
bot so Telegram delivers updates to the poll.

## POST /api/v1/telegram/stop

```bash
curl -X POST 'http://127.0.0.1:5000/api/v1/telegram/stop' \
  -H 'X-API-KEY: <APIKEY>'
```

```json
{"status": "success", "message": "Bot stopped successfully"}
```

Errors: 500 `Bot is not running`.

Stopping saves `is_active: false` before the bot stops. From then on, order
alerts are not sent and [`/notify`](#post-apiv1telegramnotify) answers 409.
A broadcast, and the Test message and Broadcast buttons on the Telegram page,
still send while the bot is stopped (the same on the web).

## GET /api/v1/telegram/users

Optional filters: `broker` (the broker that was connected when the user
linked) and `notifications_enabled` (`true` or `false`).

```bash
curl 'http://127.0.0.1:5000/api/v1/telegram/users?notifications_enabled=true' \
  -H 'X-API-KEY: <APIKEY>'
```

```json
{
  "status": "success",
  "count": 1,
  "data": [
    {
      "id": 1,
      "telegram_id": 111111111,
      "openalgo_username": "<USER_ID>",
      "first_name": "Rajesh",
      "last_name": null,
      "telegram_username": "rajesh_trades",
      "broker": "zerodha",
      "notifications_enabled": true,
      "created_at": "Wed, 07 Oct 2026 10:00:00 GMT",
      "last_command_at": "Wed, 07 Oct 2026 11:15:00 GMT"
    }
  ]
}
```

Only active links are listed. Errors: 500 `Failed to get users`.

## POST /api/v1/telegram/broadcast

```bash
curl -X POST 'http://127.0.0.1:5000/api/v1/telegram/broadcast' \
  -H 'Content-Type: application/json' \
  -d '{
    "apikey": "<APIKEY>",
    "message": "Markets open in 15 minutes",
    "filters": {"broker": "zerodha"}
  }'
```

```json
{
  "status": "success",
  "message": "Broadcast sent to 1 users, failed for 0 users",
  "success_count": 1,
  "fail_count": 0
}
```

- `message` is required, at most 4096 characters.
- `filters` is optional: `broker` (text) and `notifications_enabled`
  (boolean).
- Users with notifications turned off are always skipped.
- The call answers after every message has been tried.

| Status | Message |
|---|---|
| 400 | `Message is required` |
| 400 | `Message must not exceed 4096 characters` |
| 403 | `Broadcast is disabled` |
| 500 | `Failed to broadcast message` |

In the desktop, broadcast really sends. On the web this endpoint checks the
request and always reports `success_count: 0` without sending anything.

## POST /api/v1/telegram/notify

Sends one message to the Telegram user linked to an OpenAlgo username. In the
desktop that is your own OpenAlgo login name, once you have sent `/link` to the
bot from Telegram.

```bash
curl -X POST 'http://127.0.0.1:5000/api/v1/telegram/notify' \
  -H 'Content-Type: application/json' \
  -d '{
    "apikey": "<APIKEY>",
    "username": "<USER_ID>",
    "message": "*NIFTY* crossed 25,000",
    "wait_for_delivery": false
  }'
```

```json
{"status": "success", "message": "Notification queued for delivery"}
```

| Field | Type | Notes |
|---|---|---|
| `apikey` | string | Required (or the `X-API-KEY` header). |
| `username` | string | Required. The OpenAlgo username linked to Telegram. |
| `message` | string | Required. Telegram Markdown (`*bold*`, `_italic_`). If Telegram cannot read the formatting, the message is sent again as plain text. Telegram refuses messages over 4096 characters. |
| `wait_for_delivery` | boolean | Default `false`: answer at once and send in the background. `true`: answer after one attempt. |
| `priority` | integer | Accepted and ignored, as on the web. |

With `wait_for_delivery: true`, a delivered message answers
`{"status": "success", "message": "Notification sent successfully"}`. A
message that could not be delivered still answers 200 with
`"Notification queued for delivery"`. The failed message is recorded, but it is
**not sent again later**, on the web or in the desktop. If you need to know
whether it arrived, use `wait_for_delivery: true` and check for
`Notification sent successfully`.

| Status | Message |
|---|---|
| 409 | `Telegram bot is stopped. Start the bot to send notifications.` |
| 400 | `Username and message are required` |
| 404 | `User not found or not linked to Telegram` |
| 500 | `Failed to send notification` |

The 409 check comes first: while the bot is stopped, every notify answers 409
whatever the body says.

## GET /api/v1/telegram/stats

`days` is optional: 1 to 365, default 7. A value outside the range is moved to
the nearest end; a value that is not a number counts as 7.

```bash
curl 'http://127.0.0.1:5000/api/v1/telegram/stats?days=30' \
  -H 'X-API-KEY: <APIKEY>'
```

```json
{
  "status": "success",
  "data": {
    "total_commands": 42,
    "commands_by_type": {"positions": 18, "quote": 15, "funds": 9},
    "active_users": 1,
    "top_users": [["rajesh_trades", 42]],
    "period_days": 30
  }
}
```

`top_users` holds up to 10 `[telegram_username, count]` pairs. Errors: 500
`Failed to get statistics`.

## GET /api/v1/telegram/preferences

```bash
curl 'http://127.0.0.1:5000/api/v1/telegram/preferences?telegram_id=111111111' \
  -H 'X-API-KEY: <APIKEY>'
```

```json
{
  "status": "success",
  "data": {
    "order_notifications": true,
    "trade_notifications": true,
    "pnl_notifications": true,
    "daily_summary": true,
    "summary_time": "18:00",
    "language": "en",
    "timezone": "Asia/Kolkata"
  }
}
```

A user with no saved preferences gets the defaults shown above.

| Status | Message |
|---|---|
| 400 | `telegram_id is required` |
| 500 | `Failed to get preferences` |

## POST /api/v1/telegram/preferences

```bash
curl -X POST 'http://127.0.0.1:5000/api/v1/telegram/preferences' \
  -H 'Content-Type: application/json' \
  -d '{
    "apikey": "<APIKEY>",
    "telegram_id": 111111111,
    "daily_summary": false,
    "summary_time": "16:30"
  }'
```

```json
{"status": "success", "message": "Preferences updated successfully"}
```

- `telegram_id` is required.
- `order_notifications`, `trade_notifications`, `pnl_notifications` and
  `daily_summary` must be `true` or `false`.
- `summary_time`, `language` and `timezone` must be text.
- Preferences you leave out keep their saved value.

The preferences are stored and returned. Neither the desktop nor the web uses
them yet to decide which alerts to send.

| Status | Message |
|---|---|
| 400 | `telegram_id is required` |
| 400 | `<name> must be true or false` |
| 400 | `<name> must be text` |
| 500 | `Failed to update preferences` |

In the desktop, a preference of the wrong type is refused with 400. On the web
it fails with 500.

## POST /api/v1/telegram/webhook

This endpoint is for Telegram, not for scripts, and the desktop bot does not
need it: the bot polls. It is kept so the namespace matches the web.

- When a bot token is saved, the request must carry
  `X-Telegram-Bot-Api-Secret-Token` equal to the first 32 hexadecimal
  characters of the SHA-256 of the token. A missing header answers
  `401 Unauthorized`, a wrong one `403 Forbidden`.
- With no token saved, no header is checked.
- A JSON object with an `update_id`, or an empty body, answers 200 with an
  empty body. A JSON object without `update_id` answers `400 Bad Request`.
- Answers are plain text, not JSON.
- The update is acknowledged and not processed, as on the web.

The web can take the secret from a `TELEGRAM_WEBHOOK_SECRET` environment
variable. The desktop reads no environment variables, so the secret is always
the one derived from the token.

## Order alerts

While the bot is started, the desktop sends an alert to your linked Telegram
account for each order event: placed, modified, cancelled, all cancelled,
position closed, basket, split, options and multi-leg orders, and the GTT
events (placed, modified, cancelled, triggered, expired). Failed orders are not
alerted. Alerts in analyzer mode are marked as such. Stopping the bot stops
them. No `/notify` call is needed.

## Rate limits

| Endpoint | Limit per caller |
|---|---|
| `/api/v1/telegram/broadcast` | 5 per minute |
| `/api/v1/telegram/webhook` | the general API limit, 100 per second |
| every other `/api/v1/telegram/*` endpoint | 30 per minute |

Over the limit the answer is 429 with the web's body, for example
`{"message": "30 per 1 minute"}`.

---

**Back to**: [API Documentation](../README.md)
