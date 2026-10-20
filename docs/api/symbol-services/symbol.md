# Symbol

Look up one instrument in the master contract: its broker symbol, token, lot size, tick size, expiry and freeze quantity.

## Endpoint URL

```http
POST http://127.0.0.1:5000/api/v1/symbol
```

## Sample API Request (Equity)

```json
{
  "apikey": "<APIKEY>",
  "symbol": "RELIANCE",
  "exchange": "NSE"
}
```

## Sample cURL Request

```bash
curl -X POST http://127.0.0.1:5000/api/v1/symbol \
  -H 'Content-Type: application/json' \
  -d '{
  "apikey": "<APIKEY>",
  "symbol": "RELIANCE",
  "exchange": "NSE"
}'
```

## Sample API Response (Equity)

```json
{
  "data": {
    "brexchange": "NSE",
    "brsymbol": "RELIANCE",
    "exchange": "NSE",
    "expiry": "",
    "freeze_qty": 0,
    "id": 97798,
    "instrumenttype": "EQ",
    "lotsize": 1,
    "name": "RELIANCE INDUSTRIES",
    "strike": 0.0,
    "symbol": "RELIANCE",
    "tick_size": 0.1,
    "token": "738561::::2885"
  },
  "status": "success"
}
```

## Sample API Request (Option)

```json
{
  "apikey": "<APIKEY>",
  "symbol": "NIFTY06OCT2622400CE",
  "exchange": "NFO"
}
```

## Sample API Response (Option)

```json
{
  "data": {
    "brexchange": "NFO",
    "brsymbol": "NIFTY26O0622400CE",
    "exchange": "NFO",
    "expiry": "06-OCT-26",
    "freeze_qty": 1800,
    "id": 63347,
    "instrumenttype": "CE",
    "lotsize": 65,
    "name": "NIFTY",
    "strike": 22400.0,
    "symbol": "NIFTY06OCT2622400CE",
    "tick_size": 0.05,
    "token": "10415874::::40687"
  },
  "status": "success"
}
```

These answers were recorded with Zerodha. The broker symbol, broker exchange, token and `name` depend on your broker's master contract.

## Request Body

| Parameter | Description | Mandatory/Optional | Default Value |
|-----------|-------------|-------------------|---------------|
| apikey | Your OpenAlgo API key | Mandatory | - |
| symbol | OpenAlgo symbol (`RELIANCE`, `NIFTY27OCT26FUT`, `NIFTY06OCT2622400CE`, `NIFTY` on NSE_INDEX) | Mandatory | - |
| exchange | NSE, NFO, CDS, BSE, BFO, BCD, MCX, NCDEX, NCO, NSE_INDEX, BSE_INDEX, MCX_INDEX, GLOBAL_INDEX or CRYPTO (case-sensitive) | Mandatory | - |

## Response Fields

| Field | Type | Description |
|-------|------|-------------|
| status | string | `success` or `error` |
| data | object | The instrument |

### Data Object Fields

| Field | Type | Description |
|-------|------|-------------|
| id | number | The row's number in the master contract loaded now. It changes when the master contract is downloaded again; do not store it |
| symbol | string | OpenAlgo symbol |
| brsymbol | string | Your broker's symbol |
| name | string | Name as your broker gives it (a company name, an underlying or a description, depending on the broker) |
| exchange | string | OpenAlgo exchange |
| brexchange | string | Your broker's exchange code |
| token | string | Your broker's instrument token. Some brokers combine two ids (`738561::::2885` with Zerodha) |
| expiry | string | Expiry as `DD-MMM-YY` (`06-OCT-26`); empty for equity and indices |
| strike | number | Strike price; 0.0 for anything that is not an option (some brokers use -0.01) |
| lotsize | number | Lot size: 1 for equity, the contract lot for F&O. A fractional crypto lot is given exactly (`0.0001`) |
| instrumenttype | string | Your broker's instrument type (`EQ`, `FUT`, `CE`, `PE`, `OPTIDX` and so on) |
| tick_size | number | Smallest price step |
| freeze_qty | number | Largest quantity the exchange takes in one order, for F&O contracts that have a limit; 0 otherwise |

## Errors

Validation errors on this endpoint carry `message` as an object:

```json
{
  "message": {"symbol": ["Missing data for required field."]},
  "status": "error"
}
```

| HTTP | `message` | When |
|------|-----------|------|
| 404 | `Symbol FOOBARBAZ not found in exchange NSE` | The symbol is not in the master contract on that exchange |
| 403 | `Invalid openalgo apikey` | The key is wrong, or no broker is logged in |

Errors common to every endpoint (missing or invalid API key, OpenAlgo locked, wrong method, rate limit) are listed in the [API overview](../README.md#errors-common-to-every-endpoint).

## Notes

- The answer comes from the master contract the app downloaded for your broker. Until it has been downloaded, every symbol answers 404; download it on the **Master Contract** page.
- `freeze_qty` uses NSE's freeze limits for NFO, which are built into the app, unless you set your own on the **Freeze Quantities** page; yours take precedence.
- Expiries here are `DD-MMM-YY`. To build a symbol, write the date as `DDMMMYY`: `06-OCT-26` becomes `NIFTY06OCT26...`.
- This endpoint needs a live broker session, as on the web.
- Rate limit: 100 requests per second per caller. See [Rate limiting](../rate-limiting.md).

## In the desktop

- In the desktop, NSE's freeze limits are built into the app and arrive with each update; the Freeze Quantities page holds only the limits you add. The web copies NSE's list into its own table once, at setup, and later NSE changes reach it only through a migration.

---

**Back to**: [API Documentation](../README.md)
