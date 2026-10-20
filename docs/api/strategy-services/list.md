# Strategy List

List your strategies, newest first.

## Endpoint URL

```http
POST http://127.0.0.1:5000/api/v1/strategy/list
```

## Sample API Request

```json
{
  "apikey": "<APIKEY>"
}
```

## Sample cURL Request

```bash
curl -X POST http://127.0.0.1:5000/api/v1/strategy/list \
  -H 'Content-Type: application/json' \
  -d '{
  "apikey": "<APIKEY>"
}'
```

## Sample API Response

```json
{
  "status": "success",
  "data": [
    {
      "id": 7,
      "name": "NIFTY Short Straddle",
      "strategy_kind": "batch",
      "direction": "both",
      "universe_tab": "weekly_monthly",
      "underlying": "NIFTY",
      "underlying_exchange": "NSE_INDEX",
      "strategy_type": "intraday",
      "entry_time": "09:20",
      "exit_time": "15:10",
      "product": "NRML",
      "pricetype": "MARKET",
      "overall_sl_mtm": -5000.0,
      "overall_target_mtm": 8000.0,
      "lock_profit": {
        "mode": "lock_and_trail",
        "if_profit_reaches": 5000.0,
        "lock_profit": 2500.0,
        "trail_step": 1000.0
      },
      "trail_sl_to_entry": false,
      "scheduler": {
        "enabled": true,
        "days": ["MON", "TUE", "WED", "THU", "FRI"],
        "start_time": "09:20",
        "auto_stop_time": "15:10",
        "default_mode": "sandbox"
      },
      "live_enabled": false,
      "webhook_locked": false,
      "webhook_ip_allowlist": null,
      "daily_loss_limit_inr": 10000.0,
      "status": "running",
      "current_run_id": 42,
      "created_at": "2026-08-24T04:11:52.104883+00:00",
      "updated_at": "2026-08-30T03:50:11.482913+00:00",
      "last_finalized_run": {
        "id": 41,
        "pnl_realized": 1250.0,
        "stopped_at": "2026-08-29T09:40:11.482913+00:00"
      }
    },
    {
      "id": 4,
      "name": "RELIANCE Signals",
      "strategy_kind": "signal",
      "direction": "long_only",
      "universe_tab": "stocks_fno",
      "underlying": "MULTI",
      "underlying_exchange": "NSE",
      "strategy_type": "positional",
      "entry_time": null,
      "exit_time": null,
      "product": "MIS",
      "pricetype": "MARKET",
      "overall_sl_mtm": null,
      "overall_target_mtm": null,
      "lock_profit": null,
      "trail_sl_to_entry": false,
      "scheduler": null,
      "live_enabled": true,
      "webhook_locked": false,
      "webhook_ip_allowlist": null,
      "daily_loss_limit_inr": null,
      "status": "stopped",
      "current_run_id": null,
      "created_at": "2026-08-18T06:02:19.775410+00:00",
      "updated_at": "2026-08-18T06:02:19.775410+00:00",
      "last_finalized_run": {
        "id": 16,
        "pnl_realized": -52.0,
        "stopped_at": "2026-08-18T06:02:19.775410+00:00"
      }
    }
  ]
}
```

With no strategies yet, `data` is an empty list:

```json
{
  "status": "success",
  "data": []
}
```

## Sample API Request (Filtered)

```json
{
  "apikey": "<APIKEY>",
  "status": "running",
  "q": "NIFTY"
}
```

## Request Body

| Parameter | Description | Mandatory/Optional | Default Value |
|-----------|-------------|-------------------|---------------|
| apikey | Your OpenAlgo API key | Mandatory | - |
| status | Filter by strategy status: `stopped`, `running`, `paused`, `errored` | Optional | null (no filter) |
| q | Case-insensitive substring match on the strategy name, 100 characters at most | Optional | null (no filter) |

## Response Fields

| Field | Type | Description |
|-------|------|-------------|
| status | string | "success" or "error" |
| data | array | Strategy objects, newest first by creation time |

Each object in `data`:

| Field | Type | Description |
|-------|------|-------------|
| id | integer | Strategy id, used as `strategy_id` on every other endpoint |
| name | string | Strategy name, unique |
| strategy_kind | string | `batch` or `signal` |
| direction | string | `both`, `long_only` or `short_only`. Signal mode only |
| universe_tab | string | The instrument universe the strategy was built from: `weekly_monthly`, `monthly_only`, `stocks_fno` or `mcx`. It decides which segments a leg may use; cash is offered on `stocks_fno` only |
| underlying | string | Underlying symbol |
| underlying_exchange | string | Exchange the underlying is quoted on |
| strategy_type | string | `intraday` or `positional` |
| entry_time | string or null | IST entry time as `HH:MM` |
| exit_time | string or null | IST square-off time as `HH:MM` |
| product | string | `CNC`, `NRML` or `MIS`, as configured. Read as the intent when an order goes out: `MIS` is intraday everywhere, anything else means carry, sent as `NRML` on a derivatives venue and `CNC` on cash. The `product` field on [`/orders`](./orders.md) shows what was actually sent |
| pricetype | string | `MARKET` |
| overall_sl_mtm | number or null | Strategy-level stop loss in rupees of MTM |
| overall_target_mtm | number or null | Strategy-level target in rupees of MTM |
| lock_profit | object or null | `{mode, if_profit_reaches, lock_profit, trail_step}` |
| trail_sl_to_entry | boolean | Whether a stop on one leg trails the others to entry |
| scheduler | object or null | `{enabled, days, start_time, auto_stop_time, default_mode}` |
| live_enabled | boolean | Whether this strategy may run in `live` mode |
| webhook_locked | boolean | Whether the webhook kill switch is on |
| webhook_ip_allowlist | array or null | CIDR ranges allowed to trigger the webhook |
| daily_loss_limit_inr | number or null | Daily loss ceiling in rupees |
| status | string | `stopped`, `running`, `paused` or `errored` |
| current_run_id | integer or null | The run this strategy is executing, if any |
| created_at | string | ISO 8601 UTC |
| updated_at | string | ISO 8601 UTC |
| last_finalized_run | object or null | The most recently finished run: `{id, pnl_realized, stopped_at}`. For a stopped strategy, `pnl_realized` is the final P&L |

## Notes

- Rows are ordered by creation time, **newest first**.
- The list form omits `legs`. Call [`/api/v1/strategy/status`](./status.md) for
  one strategy's legs.
- No response here or anywhere else on this surface carries a webhook token.
- An out-of-vocabulary `status` is a 400 (`Must be one of: stopped, running,
  paused, errored.`), not an empty list.
- `status` and `q` may be sent as `null`; that is the same as omitting them. A
  `q` of only spaces is no filter.
- A checkpoint is a live mark only. After a run stops, use
  `last_finalized_run.pnl_realized` as the final total.

## Use Cases

- **Dashboards**: list strategies and their status without opening the app
- **Health checks**: find every strategy whose `status` is `errored`
- **Automation**: turn a strategy name into the `strategy_id` the other
  endpoints need

---

**Back to**: [Strategy Module API](./README.md), [API Documentation](../README.md)
