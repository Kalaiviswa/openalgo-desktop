# ClosePosition

Square off every open position in one request. OpenAlgo places a MARKET order against each open position: a SELL for a long, a BUY for a short, in the position's own product.

## Endpoint URL

```http
POST http://127.0.0.1:5000/api/v1/closeposition
```

## Sample API Request

```json
{
  "apikey": "<APIKEY>",
  "strategy": "Python"
}
```

## Sample cURL Request

```bash
curl -X POST http://127.0.0.1:5000/api/v1/closeposition \
  -H 'Content-Type: application/json' \
  -d '{
  "apikey": "<APIKEY>",
  "strategy": "Python"
}'
```

## Sample API Response

```json
{
  "message": "All Open Positions Squared Off",
  "status": "success"
}
```

## Sample API Response (Some Positions Still Open)

When the broker refused an exit, or gave no definite answer for it, the answer is an error that names each position that may still be open:

```json
{
  "message": "1 position(s) could not be squared off and are still open: SBIN (NSE): <the broker's reason>",
  "status": "error"
}
```

## Sample API Response (Analyzer Mode)

```json
{
  "closed_positions": 15,
  "failed_closures": 0,
  "message": "Closed 15 positions",
  "mode": "analyze",
  "status": "success"
}
```

## Sample API Response (No Positions, Analyzer Mode)

```json
{
  "message": "No open positions to close",
  "mode": "analyze",
  "status": "success"
}
```

## Request Body

| Parameter | Description | Mandatory/Optional | Default Value |
|-----------|-------------|-------------------|---------------|
| apikey | Your OpenAlgo API key (1 to 256 characters) | Mandatory | - |
| strategy | Strategy name | Mandatory | - |

`strategy` is required by the schema, as on the web (the web page lists it as optional). It does not limit which positions are closed: every open position is. Any other field is refused as `Unknown field.`

## Response Fields

| Field | Type | Description |
|-------|------|-------------|
| status | string | `success` or `error` |
| message | string | The result |
| closed_positions | number | Analyzer mode only: positions whose close has filled |
| failed_closures | number | Analyzer mode only: positions whose close could not be placed |
| mode | string | `analyze` when analyzer mode is on; absent in live mode |

## How It Works

1. OpenAlgo reads your position book from the broker.
2. For each position with a non-zero quantity it places a MARKET order for that quantity: SELL for a long, BUY for a short.
3. Each exit uses the position's product (MIS, CNC or NRML).
4. When every exit is accepted, the answer is `All Open Positions Squared Off`. When any exit is refused, or sent without a definite answer, the answer is HTTP 500 naming those positions.

## Analyzer Mode

With analyzer mode on, the sandbox closes its own positions and the answer carries `"mode": "analyze"`.

- A position counts as closed only once its close order has filled. When there is no usable price for a symbol, the MARKET close waits and fills as soon as a price arrives; the message then says so, for example `Closed 0 positions (1 close order waiting for a price)`.
- Asking again while a close is waiting does not place a second close; the waiting one stays.
- A close that could not be placed is counted in `failed_closures`, and the message adds, for example, `(Failed to close 1 positions)`.
- Closed positions drop out of the position book.

See [Sandbox mode](../../user/sandbox.md#closing-positions).

## Errors

Validation errors carry `message` as a string that spells out a Python dictionary. In analyzer mode the body also carries `"mode": "analyze"`:

```json
{
  "message": "{'strategy': ['Missing data for required field.']}",
  "mode": "analyze",
  "status": "error"
}
```

| HTTP | `message` | When |
|------|-----------|------|
| 400 | `{'strategy': ['Missing data for required field.']}` | `strategy` is missing |
| 400 | The broker's own reason | The broker refused, or its position book could not be read |
| 403 | `Close position operation is not allowed in Semi-Auto mode. Please switch to Auto mode to close positions.` | The API key's order mode is Semi-Auto (live mode only) |
| 500 | `N position(s) could not be squared off and are still open: SYMBOL (EXCHANGE): reason; ...` | One or more exits were refused or have no definite answer |
| 500 | `Failed to close positions due to internal error` | No usable answer from the broker |

Errors common to every endpoint (missing or invalid API key, OpenAlgo locked, wrong method, rate limit) are listed in the [API overview](../README.md#errors-common-to-every-endpoint).

## Notes

- This closes every position across every exchange, with no confirmation. To close one position, use [PlaceSmartOrder](./placesmartorder.md) with `position_size` 0.
- An exit that was sent without a definite answer is never sent again; check the order book before acting on it.
- Each exit takes its turn in the broker pacer; an exit that would wait more than 10 seconds is not sent and its position is named in the error.
- Close position is never queued in the Action Center. In Semi-Auto it is refused in live mode and allowed in analyzer mode, as on the web.
- Rate limit: 100 requests per second per caller, the general API limit. See [Rate limiting](../rate-limiting.md).

## Related Endpoints

- [CancelAllOrder](./cancelallorder.md): cancel every working order
- [PositionBook](../account-services/positionbook.md): your current positions

## In the desktop

- In the desktop, analyzer mode counts a position as closed only once its close fills, and a repeated request returns the waiting close instead of placing another. The web counted every placed close as closed and placed a new one each time.
- In the desktop, when the broker's position book cannot be read, the answer is an error. For several brokers the web answers success with nothing closed.
- In the desktop, if the order mode (Auto or Semi-Auto) cannot be read, a live close is refused with 500 and nothing is sent; the web treats an unreadable mode as Auto.

---

**Back to**: [API Documentation](../README.md)
