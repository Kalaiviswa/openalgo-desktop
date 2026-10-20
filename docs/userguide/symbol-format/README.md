# Symbol Format Guide

OpenAlgo uses one symbol format for every broker. OpenAlgo Desktop uses
exactly the same format as OpenAlgo web, so a symbol that works there works
here, in the API, webhooks, the SDK and every page. Your broker's own trading
symbols are translated for you; every book (orders, trades, positions,
holdings) answers with OpenAlgo symbols.

A wrong symbol or exchange is the most common reason an order is refused.

The full reference:

- [Symbol format](../../prompt/symbol-format.md): every exchange, index and
  instrument rule.
- [Crypto symbol format](../../prompt/crypto-symbol-format.md): Delta
  Exchange contracts on the `CRYPTO` exchange.

## The rules in short

| Instrument | Format | Example | Exchange |
| --- | --- | --- | --- |
| Equity | `[Symbol]` | `SBIN`, `RELIANCE` | `NSE`, `BSE` |
| Future | `[Symbol][DD][MMM][YY]FUT` | `NIFTY30JAN25FUT`, `CRUDEOIL19FEB25FUT` | `NFO`, `BFO`, `MCX`, `CDS`, `BCD` |
| Option | `[Symbol][DD][MMM][YY][Strike][CE or PE]` | `NIFTY30JAN2521500CE`, `VEDL24APR25292.5CE` | `NFO`, `BFO`, `MCX`, `CDS`, `BCD` |
| Index (quotes only) | `[Index]` | `NIFTY`, `BANKNIFTY`, `SENSEX`, `INDIAVIX` | `NSE_INDEX`, `BSE_INDEX` |
| Crypto perpetual | `[Pair]FUT` | `BTCUSDFUT` | `CRYPTO` |
| Crypto dated future or option | as for Indian futures and options | `BTC28FEB25FUT`, `BTC28FEB2562000CE` | `CRYPTO` |

- **Upper case**, no spaces: `SBIN`, never `sbin`.
- **Two-digit day, three-letter month, two-digit year**: `30JAN25`.
- **Strikes** keep their decimals and drop trailing zeros: `292.5`, not
  `292.50`.
- **The exchange must match the instrument**: an NSE option goes on `NFO`, not
  `NSE`.

Exchange codes: `NSE` and `BSE` (equity), `NFO` and `BFO` (equity derivatives),
`CDS` and `BCD` (currency), `MCX` (commodities), `NSE_INDEX` and `BSE_INDEX`
(index values), `CRYPTO` (Delta Exchange). Which of them you can use depends
on your broker.

Products: `MIS` (intraday), `CNC` (delivery, equity only), `NRML`
(carry-forward derivatives). Crypto is leverage based; see the crypto
reference.

## An order with a symbol

```json
{
  "apikey": "<APIKEY>",
  "strategy": "MyStrategy",
  "symbol": "NIFTY30JAN2521500CE",
  "exchange": "NFO",
  "action": "BUY",
  "quantity": "75",
  "pricetype": "MARKET",
  "product": "NRML"
}
```

Send it to `http://127.0.0.1:5000/api/v1/placeorder`. Quantities must be whole
numbers, except crypto orders going to the live broker. In the desktop a
fractional crypto quantity in analyzer mode is refused with "Fractional
quantities are not supported in OpenAlgo Desktop yet."

## Finding the right symbol

- **Search** in the profile menu (`/search/token`): type a name and copy the
  symbol and exchange exactly as shown.
- The symbol search on order forms, the option chain and the charting
  terminal lists only symbols your broker offers today.
- From a program: `POST /api/v1/search` with `{"apikey": "<APIKEY>", "query":
  "NIFTY"}`; `POST /api/v1/optionsymbol` resolves an option by underlying,
  expiry and offset.

Symbols come from your broker's master contract, downloaded after each broker
login. Until it has loaded, symbols are not found. See
[Master contract errors](../../user/troubleshooting.md#master-contract-errors).

## Common mistakes

| Wrong | Right | Why |
| --- | --- | --- |
| `NIFTYJAN2521500CE` | `NIFTY30JAN2521500CE` | The day is missing |
| `NIFTY30JAN2521500CE` on `NSE` | the same on `NFO` | Options trade on the derivatives exchange |
| An option with product `CNC` | `NRML` or `MIS` | CNC is for equity delivery only |
| An expired contract | the current expiry | Old contracts are not in today's master contract |

---

**Return to**: [User Guide](../README.md)
