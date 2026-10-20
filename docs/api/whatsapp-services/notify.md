# WhatsApp Notify

Send a WhatsApp text message to yourself, to one number, to a linked user, or
to a small group of up to 5 numbers. This is the only WhatsApp endpoint that
takes an API key.

## Endpoint URL

```http
Local Host   :  POST http://127.0.0.1:5000/api/v1/whatsapp/notify
```

If you changed the port in Server Settings, use that port instead of 5000.

## Before you call it

- Pair WhatsApp on the **WhatsApp** page of the app. Until a device is paired
  and connected, every call answers 409.
- Be signed in to OpenAlgo Desktop. After the app starts, and after you log
  out, every `/api/v1` route answers 503 until you sign in.
- No broker login is needed.

## Sample API Requests

### Send to yourself (your own number)

```json
{
  "apikey": "<APIKEY>",
  "self": true,
  "message": "Strategy finished for the day. P&L: +1.2%"
}
```

### Send to a single phone number

```json
{
  "apikey": "<APIKEY>",
  "phone": "919876543210",
  "message": "Order placed: BUY RELIANCE x 10 @ MARKET"
}
```

### Send to an OpenAlgo username

```json
{
  "apikey": "<APIKEY>",
  "username": "<USER_ID>",
  "message": "Daily summary: 4 trades, net +2,350"
}
```

Your own OpenAlgo login name works here: it sends to your own number, the
same as `"self": true`, when the device was paired while you were signed in
to that account.

### Small group (max 5 recipients)

```json
{
  "apikey": "<APIKEY>",
  "phones": ["919876543210", "919812345678", "919900112233"],
  "message": "NIFTY closed above 25,000"
}
```

## Sample cURL Request

```bash
curl -X POST http://127.0.0.1:5000/api/v1/whatsapp/notify \
  -H 'Content-Type: application/json' \
  -d '{
    "apikey": "<APIKEY>",
    "self": true,
    "message": "Alert from OpenAlgo"
  }'
```

## Sample API Response

### Default: wait for delivery

```json
{
  "status": "success",
  "message": "Delivered to 2, failed 0",
  "data": {
    "sent":    ["919876543210@s.whatsapp.net", "919812345678@s.whatsapp.net"],
    "failed":  [],
    "skipped": 0
  }
}
```

### `wait_for_delivery: false`

```json
{
  "status": "success",
  "message": "Queued for 1 recipient(s)",
  "queued": 1
}
```

### Nothing delivered

When no recipient got the message and at least one send failed, the answer is
still HTTP 200, with `status` set to `error` and the first failure's reason as
the message:

```json
{
  "status": "error",
  "message": "WhatsApp did not accept the message. Check the number and that the device is still linked.",
  "data": {
    "sent":    [],
    "failed":  [{"to": "919876543210@s.whatsapp.net", "error": "WhatsApp did not accept the message. Check the number and that the device is still linked."}],
    "skipped": 0
  }
}
```

Check `status`, not only the HTTP code. This matches the web.

## Request Body

| Parameter | Type | Description |
|-----------|------|-------------|
| `apikey` | string | OpenAlgo API key. **Mandatory** (or send it in the `X-API-KEY` header). |
| `self` | boolean | If `true`, send to your own number (the paired device). |
| `username` | string | An OpenAlgo username. Resolves to a linked WhatsApp user, or to your own number when it is the account the device was paired from. |
| `phone` | string | One number, digits with country code (e.g. `919876543210`). Spaces, `+` and dashes are ignored; 7 to 15 digits. |
| `phones` | array of strings | Up to 5 numbers. Anything beyond the fifth is dropped. Invalid entries are skipped. |
| `message` | string | Text body. **Required in the desktop.** Max 4096 characters. |
| `wait_for_delivery` | boolean | Default `true`: send now and answer with a per-recipient report. `false`: answer at once and send in the background. |
| `image_path` | string | Refused in the desktop (see below). |
| `document_path` | string | Refused in the desktop (see below). |
| `caption`, `filename` | string | Accepted and unused in the desktop, since attachments are refused. |

Give exactly one recipient form. They are checked in this order and the first
one present wins: `self`, `phones`, `phone`, `username`.

## Response Fields (`wait_for_delivery`, the default)

| Field | Type | Description |
|-------|------|-------------|
| `status` | string | `success` when at least one recipient got it, or nothing failed; `error` when nobody got it and something failed |
| `message` | string | `Delivered to N, failed M`, or the first failure's reason |
| `data.sent` | array | WhatsApp IDs (`<digits>@s.whatsapp.net`) WhatsApp accepted |
| `data.failed` | array | `[{"to": "<id>", "error": "<reason>"}, ...]` |
| `data.skipped` | int | Recipients over the 5-recipient cap (always 0 here, since `phones` is trimmed to 5 first) |

## Response Fields (`wait_for_delivery: false`)

| Field | Type | Description |
|-------|------|-------------|
| `status` | string | `success` |
| `message` | string | `Queued for N recipient(s)` |
| `queued` | int | Number of recipients the background send will try |

A background send that fails is logged and dropped, not retried.

## Errors

| Status | Message |
|--------|---------|
| 401 | `Invalid or missing API key` |
| 409 | `WhatsApp is not paired or not connected. Pair the device first from the /whatsapp page in OpenAlgo before sending.` |
| 409 | `WhatsApp logged this device out, so alerts cannot be sent. Pair it again from the /whatsapp page in OpenAlgo.` |
| 400 | `Message must not exceed 4096 characters` |
| 400 | `Provide at least one of: message, image_path, document_path` |
| 400 | `image_path is not allowed` / `document_path is not allowed` |
| 400 | `'phones' must be a list` |
| 400 | `No valid phones in list` |
| 400 | `Invalid phone number` |
| 400 | `Specify one of: 'self', 'username', 'phone', or 'phones'` |
| 404 | `Username not found or not linked to WhatsApp` |
| 429 | over the rate limit |
| 503 | OpenAlgo is locked: sign in to the app |

The checks run in the order listed: the key, then whether WhatsApp is ready,
then the message, then the attachments, then the recipient.

Reasons that can appear in `data.failed[].error`:

- `This is not a valid WhatsApp number.`
- `WhatsApp did not accept the message. Check the number and that the device is still linked.`
- `send timeout` (no answer from WhatsApp within 30 seconds)
- `No recipient resolved` (`self` was asked for, but the paired number is not
  known yet)
- `Bot not connected`, or the logged-out message above, when the connection
  dropped just before the send

## Notes

- **Text only in the desktop.** The web can send an image or a document from a
  folder on its server, limited to the folders in its
  `WHATSAPP_ATTACHMENT_ROOTS` setting. The desktop reads no such setting and
  has no attachment folder, so `image_path` and `document_path` are always
  refused with 400.
- **Default is to wait.** `wait_for_delivery` defaults to `true` on the web and
  in the desktop. Older web docs say it defaults to `false`; the web code does
  not.
- **Not paired, not queued.** If WhatsApp is not paired or not connected, the
  call is refused with 409 rather than queued, so you find out at once. The web
  docs mention a queue for later retry; the desktop has none.
- **Rate limit.** In the desktop this endpoint falls under the general API
  limit of 100 requests per second per caller. The web limits it to 30 per
  minute. Keep to a few messages a minute anyway: bulk sending can get the
  paired device logged out by WhatsApp.
- The 5-recipient cap is a safety guardrail for WhatsApp's terms of service.
  Use the official WhatsApp Business API for genuine mass messaging.

---

**Back to**: [WhatsApp API](./README.md), [API Documentation](../README.md)
