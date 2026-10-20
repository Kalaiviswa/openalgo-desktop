# Order Constants

These are the values the `/api/v1` order endpoints accept. OpenAlgo Desktop
accepts exactly the same values as OpenAlgo web and checks them the same way,
so a request that works against the web works here unchanged.

Values are case-sensitive. Only `action` also accepts lower case.

## Exchange

| Value | Market |
| --- | --- |
| `NSE` | NSE equity |
| `NFO` | NSE futures and options |
| `CDS` | NSE currency derivatives |
| `BSE` | BSE equity |
| `BFO` | BSE futures and options |
| `BCD` | BSE currency derivatives |
| `MCX` | MCX commodities |
| `NCDEX` | NCDEX commodities |
| `NCO` | NSE commodity futures and options |
| `NSE_INDEX` | NSE indices (quotes and data only) |
| `BSE_INDEX` | BSE indices (quotes and data only) |
| `MCX_INDEX` | MCX commodity sectoral indices, such as MCXBULLDEX (quotes and data only) |
| `GLOBAL_INDEX` | Global indices such as US30, JAPAN225, HANGSENG, GIFTNIFTY (quotes and data only) |
| `CRYPTO` | Crypto contracts, Delta Exchange only |

Every value is accepted by the request check, but your broker only trades the
exchanges its master contract carries. `NCO`, `MCX_INDEX`, `GLOBAL_INDEX` and
`NCDEX` exist only with some brokers, and `CRYPTO` only with Delta Exchange.
See [symbol-format.md](symbol-format.md).

## Product type

| Value | Meaning |
| --- | --- |
| `CNC` | Cash and carry (delivery) for equity |
| `NRML` | Normal (carry forward) for futures and options |
| `MIS` | Intraday, squared off the same day |

If `product` is left out, `MIS` is used.

In analyzer mode the sandbox also refuses the combinations a broker would
refuse, with the same messages as the web:

- `NRML` on `NSE` or `BSE`: "NRML product not allowed for NSE equity segment.
  Use CNC for delivery or MIS for intraday."
- `CNC` on `NFO`, `BFO`, `MCX`, `CDS`, `BCD`, `NCDEX` or `CRYPTO`: "CNC product
  not allowed for NFO derivatives segment. Use NRML for carryforward or MIS for
  intraday."

In live mode your broker makes that decision.

## Price type

| Value | Meaning | Needs |
| --- | --- | --- |
| `MARKET` | Market order | nothing |
| `LIMIT` | Limit order | `price` |
| `SL` | Stop-loss limit order | `price` and `trigger_price` |
| `SL-M` | Stop-loss market order | `trigger_price` |

If `pricetype` is left out, `MARKET` is used. `price` and `trigger_price`
default to `0` and must not be negative. In analyzer mode a missing price or
trigger is refused ("LIMIT orders require price", "SL orders require
trigger_price"); in live mode the broker refuses it.

## Action

| Value | Meaning |
| --- | --- |
| `BUY` | Buy |
| `SELL` | Sell |

`buy` and `sell` are accepted too and treated as `BUY` and `SELL`.

## Quantity

- Must be greater than zero.
- Must be a whole number on every exchange except `CRYPTO`. `1.5` on `NSE`
  answers "Fractional quantity (1.5) is not allowed for non-crypto exchanges."
- Is in units (shares or contracts), not lots. On a derivatives exchange it
  must be a multiple of the lot size: see [LotSize.md](LotSize.md).
- In the desktop, a quantity or disclosed quantity above 2,147,483,647 is
  refused before anything is sent ("Quantity is larger than any exchange
  accepts."). Brokers take a 32-bit quantity; the web passes a larger number
  on to the broker.
- In the desktop, fractional `CRYPTO` quantities work only for a single order
  (`placeorder`, `modifyorder`) sent to Delta Exchange in live mode. Smart,
  basket and split orders, and every order in analyzer mode, take whole
  numbers only and refuse a fraction ("Fractional quantities are not supported
  in OpenAlgo Desktop yet. Use a whole-number quantity."). The web passes the
  fraction to the broker on all of them, and its analyzer mode cuts the
  fraction off (1.5 becomes 1); the desktop refuses rather than change your
  quantity. See [crypto-symbol-format.md](crypto-symbol-format.md).

## Where the lists are narrower

| Where | Rule |
| --- | --- |
| `optionsorder`, `optionsmultiorder` | `product` is `MIS` or `NRML` only (default `MIS`) |
| `placegttorder`, `modifygttorder` | `product` is `CNC` or `NRML` only ("GTT supports only CNC (delivery) or NRML (overnight F&O); MIS is intraday-only."); `pricetype` is `LIMIT` (default) or `MARKET`; `trigger_type` is `SINGLE` or `OCO` |
| Strategy module | Entries and exits are `MARKET` only; the product is an intent (below) |

### Strategy module

The `/strategy` module uses the same constants with two narrower rules:

- The configured product is an intent, not always the literal value sent.
  `MIS` stays `MIS`. Either carry product (`CNC` or `NRML`) is sent as `NRML`
  on a derivatives exchange (`NFO`, `BFO`, `MCX`, `CDS`, `BCD`, `NCDEX`, `NCO`)
  and as `CNC` on a cash exchange. The product actually sent is stored on each
  strategy order.
- Every strategy entry and exit is a `MARKET` order. A strategy leg carries no
  price, so `LIMIT`, `SL` and `SL-M` are refused when the strategy is saved.

See [strategy_rms_documentation.md](strategy_rms_documentation.md).

## What a wrong value returns

A value outside the list answers HTTP 400. On the order endpoints the
`message` is the field errors written as text, exactly as the web sends it:

```json
{
  "status": "error",
  "message": "{'product': ['Must be one of: MIS, NRML, CNC.']}"
}
```

On the data endpoints (`quotes`, `depth`, `history` and the like) the same
errors come back as an object:

```json
{
  "status": "error",
  "message": {
    "exchange": [
      "Must be one of: NSE, NFO, CDS, BSE, BFO, BCD, MCX, NCDEX, NCO, NSE_INDEX, BSE_INDEX, MCX_INDEX, GLOBAL_INDEX, CRYPTO."
    ]
  }
}
```

A request field the endpoint does not know is refused the same way
(`"Unknown field."`), except on the GTT endpoints, which ignore it. The web
behaves the same.
