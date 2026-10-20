# OptionsMultiOrder

Place several option legs on one underlying in a single request: spreads, straddles, strangles, iron condors. Each leg names an offset (ATM, ITM or OTM) and OpenAlgo picks the contract. BUY legs are placed before SELL legs, so the hedge is in place before the short leg needs margin.

## Endpoint URL

```http
POST http://127.0.0.1:5000/api/v1/optionsmultiorder
```

## Sample API Request (Iron Condor, Same Expiry)

```json
{
  "apikey": "<APIKEY>",
  "strategy": "Iron Condor Test",
  "underlying": "NIFTY",
  "exchange": "NSE_INDEX",
  "expiry_date": "06OCT26",
  "legs": [
    {"offset": "OTM6", "option_type": "CE", "action": "BUY", "quantity": 65},
    {"offset": "OTM6", "option_type": "PE", "action": "BUY", "quantity": 65},
    {"offset": "OTM4", "option_type": "CE", "action": "SELL", "quantity": 65},
    {"offset": "OTM4", "option_type": "PE", "action": "SELL", "quantity": 65}
  ]
}
```

## Sample cURL Request

```bash
curl -X POST http://127.0.0.1:5000/api/v1/optionsmultiorder \
  -H 'Content-Type: application/json' \
  -d '{
  "apikey": "<APIKEY>",
  "strategy": "Iron Condor Test",
  "underlying": "NIFTY",
  "exchange": "NSE_INDEX",
  "expiry_date": "06OCT26",
  "legs": [
    {"offset": "OTM6", "option_type": "CE", "action": "BUY", "quantity": 65},
    {"offset": "OTM6", "option_type": "PE", "action": "BUY", "quantity": 65},
    {"offset": "OTM4", "option_type": "CE", "action": "SELL", "quantity": 65},
    {"offset": "OTM4", "option_type": "PE", "action": "SELL", "quantity": 65}
  ]
}'
```

## Sample API Request (Bull Call Spread)

```json
{
  "apikey": "<APIKEY>",
  "strategy": "fixtures",
  "underlying": "NIFTY",
  "exchange": "NSE_INDEX",
  "expiry_date": "06OCT26",
  "legs": [
    {"offset": "ATM", "option_type": "CE", "action": "BUY", "quantity": 65, "pricetype": "MARKET", "product": "NRML"},
    {"offset": "OTM2", "option_type": "CE", "action": "SELL", "quantity": 65, "pricetype": "MARKET", "product": "NRML"}
  ]
}
```

## Sample API Response (Bull Call Spread, Analyzer Mode)

```json
{
  "mode": "analyze",
  "results": [
    {
      "action": "BUY",
      "exchange": "NFO",
      "leg": 1,
      "mode": "analyze",
      "offset": "ATM",
      "option_type": "CE",
      "orderid": "26100308621852",
      "product": "NRML",
      "status": "success",
      "strike": null,
      "symbol": "NIFTY06OCT2622400CE"
    },
    {
      "action": "SELL",
      "exchange": "NFO",
      "leg": 2,
      "mode": "analyze",
      "offset": "OTM2",
      "option_type": "CE",
      "orderid": "26100311258271",
      "product": "NRML",
      "status": "success",
      "strike": null,
      "symbol": "NIFTY06OCT2622500CE"
    }
  ],
  "status": "success",
  "underlying": "NIFTY",
  "underlying_ltp": 22421.95
}
```

In live mode the top-level `mode` is absent and each leg says `"mode": "live"`.

## Sample API Response (One Leg Failed)

The request succeeds as a whole (HTTP 200, `status: success`); each leg reports its own `status`.

```json
{
  "mode": "analyze",
  "results": [
    {
      "action": "BUY",
      "exchange": "NFO",
      "leg": 1,
      "mode": "analyze",
      "offset": "ATM",
      "option_type": "PE",
      "orderid": "26100378391337",
      "product": "NRML",
      "status": "success",
      "strike": null,
      "symbol": "NIFTY06OCT2622400PE"
    },
    {
      "action": "SELL",
      "leg": 2,
      "message": "Offset BAD is out of range for available strikes. Please use a smaller offset.",
      "offset": "BAD",
      "option_type": "PE",
      "status": "error",
      "strike": null
    }
  ],
  "status": "success",
  "underlying": "NIFTY",
  "underlying_ltp": 22421.95
}
```

## Sample API Request (Diagonal Spread, Different Expiries)

```json
{
  "apikey": "<APIKEY>",
  "strategy": "Diagonal Spread Test",
  "underlying": "NIFTY",
  "exchange": "NSE_INDEX",
  "legs": [
    {"offset": "ITM2", "option_type": "CE", "action": "BUY", "quantity": 65, "expiry_date": "27OCT26"},
    {"offset": "OTM2", "option_type": "CE", "action": "SELL", "quantity": 65, "expiry_date": "06OCT26"}
  ]
}
```

## Request Body

| Parameter | Description | Mandatory/Optional | Default Value |
|-----------|-------------|-------------------|---------------|
| apikey | Your OpenAlgo API key | Mandatory | - |
| strategy | Strategy name, used to tag and log the orders | Mandatory | - |
| underlying | Underlying symbol (`NIFTY`, `BANKNIFTY`, `RELIANCE`), or a futures symbol that carries the expiry | Mandatory | - |
| exchange | Exchange of the underlying (`NSE_INDEX`, `BSE_INDEX`, `NSE`, `BSE`, `MCX` and the other exchanges in the shared list) | Mandatory | - |
| expiry_date | Expiry for every leg, `DDMMMYY` (`06OCT26`); a leg's own `expiry_date` overrides it | Optional | - |
| strike_int | Strike interval for every leg; when left out the listed strikes are used (recommended) | Optional | - |
| legs | 1 to 20 leg objects | Mandatory | - |

### Leg Object Fields

| Parameter | Description | Mandatory/Optional | Default Value |
|-----------|-------------|-------------------|---------------|
| offset | `ATM`, `ITM1` to `ITM50`, or `OTM1` to `OTM50` | Mandatory | - |
| option_type | `CE` or `PE` (lowercase is accepted) | Mandatory | - |
| action | `BUY` or `SELL` (lowercase is accepted) | Mandatory | - |
| quantity | Whole number of units, at least 1 | Mandatory | - |
| expiry_date | This leg's expiry, `DDMMMYY`, for calendar and diagonal spreads | Optional | The common `expiry_date` |
| splitsize | Chunk size for this leg; 0 places one order | Optional | 0 |
| pricetype | `MARKET`, `LIMIT`, `SL` or `SL-M` | Optional | MARKET |
| product | `MIS` or `NRML` | Optional | MIS |
| price | Limit price | Optional | 0 |
| trigger_price | Trigger price | Optional | 0 |
| disclosed_quantity | Disclosed quantity | Optional | 0 |

Any other field, at the top level or in a leg, is refused as `Unknown field.`

## Response Fields

| Field | Type | Description |
|-------|------|-------------|
| status | string | `success` whenever the request was processed, even if every leg failed |
| underlying | string | The `underlying` you sent |
| underlying_ltp | number or null | The underlying price used for every leg; `null` when it could not be read once (each leg then reads its own) |
| results | array | One entry per leg, in request order |
| mode | string | `analyze` in analyzer mode; absent in live mode |

### Results Array Fields

| Field | Type | Description |
|-------|------|-------------|
| leg | number | Leg number, from 1, in request order |
| action | string | `BUY` or `SELL` |
| offset | string | The offset sent |
| option_type | string | `CE` or `PE` |
| strike | null | Always `null` (kept for the web's shape) |
| symbol | string | The contract picked (absent when the contract could not be picked) |
| exchange | string | Where it was placed (absent when the contract could not be picked) |
| product | string | `MIS` or `NRML` (on a placed leg) |
| orderid | string | The order id (on a placed leg without `splitsize`) |
| status | string | `success` or `error` for this leg |
| message | string | Why this leg failed |
| mode | string | `analyze` or `live` (on a placed leg) |
| total_quantity, split_size, split_results | number, number, array | On a leg with `splitsize`: the chunks, each `{order_num, quantity, status, orderid}` or `{order_num, quantity, status, message}`; the leg is `success` if any chunk was placed |

## Analyzer Mode

With analyzer mode on, the contracts are still picked from the live underlying price, then every leg goes to the sandbox. The answer and each placed leg carry `"mode": "analyze"`. Orders fill the way a [PlaceOrder](./placeorder.md) order does, never at a price older than 30 seconds, and F&O quantities must be a multiple of the lot size. A broker session is still needed, for the underlying price. See [Sandbox mode](../../user/sandbox.md).

## Errors

Validation errors on this endpoint use their own shape: a fixed `message` and the field errors as an object under `errors`, with no `mode` key:

```json
{
  "errors": {
    "legs": [
      "Legs must contain 1 to 20 items."
    ]
  },
  "message": "Validation error",
  "status": "error"
}
```

A field error inside a leg is reported under the leg's position, for example `{"legs": {"0": {"product": ["Must be one of: MIS, NRML."]}}}`.

| HTTP | `message` | When |
|------|-----------|------|
| 400 | `Validation error` (field errors in `errors`) | `legs` is empty or has more than 20 entries, or a field is missing or not allowed |

Everything after validation is reported per leg, inside a 200 answer:

| Leg `message` | When |
|---------------|------|
| `Offset BAD is out of range for available strikes. Please use a smaller offset.` | The offset is not usable or there are not that many listed strikes |
| `No strikes found for NIFTY expiring 01JAN20. Please check expiry date or update master contract.` | The leg's expiry is not a listed `DDMMMYY` date |
| `Expiry date required. Provide via expiry_date parameter or embed in underlying (e.g., NIFTY28OCT25FUT).` | Neither the leg nor the request gives an expiry |
| `Split orders would exceed maximum limit of 100 per leg` | The leg's `quantity / splitsize` needs more than 100 orders |
| `Quantity must be in multiples of lot size 65` | Analyzer mode: the leg's quantity is not a lot multiple (any sandbox refusal of [PlaceOrder](./placeorder.md#errors) can appear here) |
| The broker's own reason, or the pacing message | The broker refused the leg, or it would have waited more than 10 seconds for its turn |

Errors common to every endpoint (missing or invalid API key, OpenAlgo locked, wrong method, rate limit) are listed in the [API overview](../README.md#errors-common-to-every-endpoint).

## Supported Strategies

| Strategy | Legs | Description |
|----------|------|-------------|
| Iron Condor | 4 | OTM CE buy, OTM PE buy, nearer OTM CE sell, nearer OTM PE sell |
| Strangle | 2 | OTM CE and OTM PE, same expiry |
| Straddle | 2 | ATM CE and ATM PE, same expiry |
| Bull Call Spread | 2 | Buy the lower strike CE, sell a higher strike CE |
| Bear Put Spread | 2 | Buy the higher strike PE, sell a lower strike PE |
| Calendar Spread | 2 | Same offset, different expiries |
| Diagonal Spread | 2 | Different offsets, different expiries |

## Notes

- BUY legs are placed first, then SELL legs, each group in the order you sent them. `results` lists the legs in request order.
- A leg that fails does not stop the others.
- The underlying price is read once and used for every leg, so all offsets are counted from the same price.
- When any leg has `splitsize`, consecutive orders are sent 100 ms apart.
- When the API key's order mode is Semi-Auto, nothing is placed: the whole request waits in the Action Center and the answer is `{"status": "success", "message": "Order queued for approval in Action Center", "mode": "semi_auto", "pending_order_id": 12}`.
- Rate limit: 10 requests per second per caller, shared with the other order endpoints. See [Rate limiting](../rate-limiting.md).

## In the desktop

- In the desktop, a leg sent with no definite answer from the broker is never sent again: OpenAlgo looks for it in the broker's order book. See [Order placement outcomes](../../port/order-outcomes.md).
- In the desktop, if OpenAlgo cannot read whether the order mode is Semi-Auto, the request is refused (500) instead of being placed; the web places it.

---

**Back to**: [API Documentation](../README.md)
