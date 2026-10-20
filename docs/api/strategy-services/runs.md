# Strategy Runs

Every activation of a strategy, newest first.

## Endpoint URL

```http
POST http://127.0.0.1:5000/api/v1/strategy/runs
```

## Sample API Request

```json
{
  "apikey": "<APIKEY>",
  "strategy_id": 7
}
```

## Sample cURL Request

```bash
curl -X POST http://127.0.0.1:5000/api/v1/strategy/runs \
  -H 'Content-Type: application/json' \
  -d '{
  "apikey": "<APIKEY>",
  "strategy_id": 7,
  "limit": 10
}'
```

## Sample API Response

```json
{
  "status": "success",
  "data": [
    {
      "id": 42,
      "strategy_id": 7,
      "mode": "sandbox",
      "broker": "sandbox",
      "started_at": "2026-08-30T03:50:11.402118+00:00",
      "stopped_at": "2026-08-30T09:40:02.771905+00:00",
      "stop_reason": "eod",
      "stop_requested_at": null,
      "stop_requested_reason": null,
      "pnl_realized": 3140.5,
      "pnl_peak": 4880.0,
      "pnl_trough": -1220.25,
      "trigger_source": "manual",
      "webhook_event_id": null,
      "resolved_expiries": {
        "1": "04-SEP-26",
        "2": "04-SEP-26"
      }
    },
    {
      "id": 39,
      "strategy_id": 7,
      "mode": "live",
      "broker": "zerodha",
      "started_at": "2026-08-29T03:50:04.118332+00:00",
      "stopped_at": "2026-08-29T06:12:44.900410+00:00",
      "stop_reason": "overall_sl",
      "stop_requested_at": null,
      "stop_requested_reason": null,
      "pnl_realized": -5024.0,
      "pnl_peak": 910.0,
      "pnl_trough": -5180.0,
      "trigger_source": "webhook",
      "webhook_event_id": 811,
      "resolved_expiries": {
        "1": "04-SEP-26",
        "2": "04-SEP-26"
      }
    }
  ]
}
```

## Request Body

| Parameter | Description | Mandatory/Optional | Default Value |
|-----------|-------------|-------------------|---------------|
| apikey | Your OpenAlgo API key | Mandatory | - |
| strategy_id | Strategy id, a positive integer | Mandatory | - |
| limit | How many runs to return, 1 to 500 | Optional | 100 |

## Response Fields

| Field | Type | Description |
|-------|------|-------------|
| status | string | "success" or "error" |
| data | array | Run objects, newest first by start time |

Each object in `data`:

| Field | Type | Description |
|-------|------|-------------|
| id | integer | Run id, usable as the `run_id` filter on [`/orders`](./orders.md) and [`/events`](./events.md) |
| strategy_id | integer | Owning strategy |
| mode | string | `live` or `sandbox`, fixed for the life of the run |
| broker | string | Broker the run was bound to, recorded at start. `sandbox` for a sandbox run |
| started_at | string | ISO 8601 UTC |
| stopped_at | string or null | ISO 8601 UTC, `null` while the run is still open |
| stop_reason | string or null | Why the run ended |
| stop_requested_at | string or null | ISO 8601 UTC, set while a saved stop is pending; cleared when the run finishes |
| stop_requested_reason | string or null | Pending stop reason. While set, new signal entries are refused and a restart resumes the stop |
| pnl_realized | number | Realized P&L, written when the run was confirmed flat |
| pnl_peak | number | Highest P&L the run reached |
| pnl_trough | number | Lowest P&L the run reached |
| trigger_source | string | `manual`, `webhook` or `scheduler` |
| webhook_event_id | integer or null | The webhook audit row that started this run, when one did |
| resolved_expiries | object or null | Leg id to the expiry resolved at start, keyed by string |

### Stop reasons

`manual`, `scheduler`, `overall_sl`, `overall_target`, `lock_profit`, `eod`,
`expiry`, `daily_loss_limit`, `tick_stale`, `recovery_failed`, `error`

## Notes

- Rows are ordered by start time, **newest first**.
- **`limit` is bounded rather than clamped.** A value below 1 or above 500 is a
  400 (`Must be greater than or equal to 1 and less than or equal to 500.`), so
  you learn the value was refused.
- The top row is the current run only while its `stopped_at` is `null`. Compare
  with `current_run_id` from [`/status`](./status.md) if you need certainty.
- `stop_requested_at` and `stop_requested_reason` mark a saved stop that is still
  waiting for fills. The run stays current, subscribed and managed until it is
  proven flat; finishing it writes `stopped_at` and `stop_reason`, releases the
  strategy and clears both request fields.
- `pnl_peak` and `pnl_trough` are final only once the run has stopped.
- **An overall threshold triggers an exit; it does not promise the result.**
  Aggregate risk is checked after each tick from the latest price of each leg,
  not a single simultaneous snapshot. Market exits fill at the available price,
  so `pnl_realized` can differ from the threshold and from the peak or trough. A
  run stopped by the combined target keeps `stop_reason: "overall_target"` even
  when its final P&L is below that target.
- A fill the broker reports without a usable average price is flagged at once
  with a critical event on the leg: the quantity is managed, but its value and
  the realized P&L cannot be verified. Check the fill price with your broker.
- A signal strategy has one run per trading session rather than one per entry
  and exit: the first signal after the session boundary opens it and the
  scheduled square-off (or the first signal of the next day) closes it.
- In the desktop, a failed read answers 500 `An unexpected error occurred`
  rather than an empty list, so an error never reads as "never ran".

## Use Cases

- **Performance review**: pull the last N runs and their realized P&L
- **Post-mortem**: find which runs ended on `overall_sl` or `daily_loss_limit`
- **Drill-down**: take a `run_id` from here into [`/orders`](./orders.md) or
  [`/events`](./events.md)

---

**Back to**: [Strategy Module API](./README.md), [API Documentation](../README.md)
