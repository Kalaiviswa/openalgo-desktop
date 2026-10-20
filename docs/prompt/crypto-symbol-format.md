# Crypto Symbol Format

OpenAlgo Desktop trades crypto through one broker, Delta Exchange (India).
Crypto contracts use the same symbol grammar as Indian futures and options, so
a strategy written for `NIFTY...FUT` or `...CE` carries over with no new
syntax. Only perpetual futures and spot pairs have crypto-only shapes.

The desktop builds these symbols exactly as OpenAlgo web does, from Delta's
live product list. For signing in to Delta Exchange and its daily session,
see [docs/user/brokers.md](../user/brokers.md).

## Exchange code

Every crypto contract is on the exchange `CRYPTO`: perpetuals, dated futures,
options and spot. In the master contract, `brexchange` is `DELTAIN` (Delta
Exchange India).

## Perpetual futures

**Format:** `[Delta symbol]FUT`

| Instrument | Delta symbol | OpenAlgo symbol |
| --- | --- | --- |
| BTC perpetual | `BTCUSD` | `BTCUSDFUT` |
| ETH perpetual | `ETHUSD` | `ETHUSDFUT` |

`instrumenttype` is `PERPFUT` and `expiry` is empty, since a perpetual never
expires. The stored and served symbol is `BTCUSDFUT`; the TradingView style
`BTCUSD.P` is not an OpenAlgo symbol.

## Dated futures

**Format:** `[Underlying][DDMMMYY]FUT`, the same as Indian futures.

| Instrument | Delta symbol | OpenAlgo symbol |
| --- | --- | --- |
| BTC future, 27 November 2026 | `BTCUSD27Nov2026` | `BTC27NOV26FUT` |

The quote currency (`USDT`, `USD`, `BTC` or `ETH`) is removed to give the
underlying, then the expiry date is added. `instrumenttype` is `FUT` and
`expiry` is the settlement date as `DD-MMM-YY` (UTC).

## Options

**Format:** `[Underlying][DDMMMYY][Strike][CE or PE]`, the same as Indian
options.

| Instrument | Delta symbol | OpenAlgo symbol |
| --- | --- | --- |
| BTC 62000 call, 27 November 2026 | `C-BTC-62000-271126` | `BTC27NOV2662000CE` |
| BTC 62000 put, 27 November 2026 | `P-BTC-62000-271126` | `BTC27NOV2662000PE` |

Turbo and synthetic options end in the same `CE` or `PE`: turbo call (`TCE`)
and synthetic call (`SYNCE`) become `...CE`; turbo put (`TPE`) and synthetic
put (`SYNPE`) become `...PE`. Their `instrumenttype` keeps the Delta code
(`TCE`, `SYNCE`, `TPE`, `SYNPE`).

## Spot

**Format:** `[Base][Quote]`, the Delta pair without the underscore.

| Instrument | Delta symbol | OpenAlgo symbol |
| --- | --- | --- |
| BTC/INR spot | `BTC_INR` | `BTCINR` |
| ETH/USDT spot | `ETH_USDT` | `ETHUSDT` |

`instrumenttype` is `SPOT` and `expiry` is empty. The quote currency is part of
the pair: `BTCINR` is priced in rupees, `BTCUSDT` in USDT.

## Other contract types

Move options (`MOVE`), interest-rate swaps (`IRS`), spreads (`SPREAD`) and
option combos (`COMBO`) keep Delta's own symbol.

## Instrument type codes

| Delta `contract_type` | OpenAlgo `instrumenttype` | Symbol shape |
| --- | --- | --- |
| `perpetual_futures` | `PERPFUT` | `[Delta symbol]FUT` |
| `futures` | `FUT` | `[Underlying][Expiry]FUT` |
| `call_options` | `CE` | `...CE` |
| `put_options` | `PE` | `...PE` |
| `turbo_call_options` | `TCE` | `...CE` |
| `turbo_put_options` | `TPE` | `...PE` |
| `synth_call_options` | `SYNCE` | `...CE` |
| `synth_put_options` | `SYNPE` | `...PE` |
| `spot` | `SPOT` | `[Base][Quote]` |
| `move_options` | `MOVE` | Delta's symbol |
| `interest_rate_swaps` | `IRS` | Delta's symbol |
| `spreads` | `SPREAD` | Delta's symbol |
| `options_combos` | `COMBO` | Delta's symbol |

Only live, tradable products are kept. A product Delta allows only
reduce-only orders on is left out of the list.

## Order values for crypto

- **Exchange:** `CRYPTO`.
- **Product:** Delta has no CNC, NRML or MIS. The request still needs one of
  the three to pass the check (`MIS` if you leave it out), and Delta ignores
  it. The books report `NRML` for futures and options and `CNC` for spot
  balances, so send `NRML` to keep smart orders and position checks
  consistent. In analyzer mode `CNC` on `CRYPTO` is refused, as on the web.
- **Price type:** `MARKET`, `LIMIT`, `SL` and `SL-M`. `SL` and `SL-M` become
  Delta stop orders triggered on the last traded price.
- **Time in force:** orders go to Delta as good-till-cancelled. The `/api/v1`
  order body has no time-in-force, post-only or reduce-only field; an unknown
  field is refused, as on the web.
- **Order id:** Delta order ids come back as `"<product id>:<order id>"`, for
  example `"27:123456789"`. Pass the whole string to `modifyorder`,
  `cancelorder` and `orderstatus`.

### Quantity

- Futures and options are counted in whole contracts. A fraction is refused
  ("Fractional quantity (...) not allowed for derivative contracts.").
- Spot takes a fractional size, for example `0.0005` BTC.
- In the desktop, a fractional size works only for `placeorder` and
  `modifyorder` sent to Delta in live mode. Smart, basket and split orders,
  and every order in analyzer mode, take whole numbers only and answer
  "Fractional quantities are not supported in OpenAlgo Desktop yet. Use a
  whole-number quantity." OpenAlgo web passes the fraction on in live mode and
  cuts it off in analyzer mode.

### Lot size and contract value

`lotsize` is Delta's minimum order size. For futures and options it is a whole
number of contracts. For spot it can be fractional (`0.0001`); the desktop
serves the exact value from `/api/v1/symbol`, as the web does.

Each contract also has a contract value (for example 0.001 BTC per `BTCUSD`
contract), stored with the master contract. The sandbox uses it for P&L and
margin; live P&L comes from Delta.

## Leverage

When Delta Exchange is your broker, a **Leverage** entry appears in the profile
menu (the page is `/leverage`). It holds one value for every crypto order:
1x, 2x, 3x, 5x, 10x, 20x or 50x, or **Default (Broker Setting)**. Before each
new order is placed, the desktop sets that leverage on the contract at Delta.
With Default, the contract keeps whatever leverage Delta already has. If Delta
refuses the change, the refusal is logged and the order still goes out at the
contract's current leverage, as on the web.

## Crypto in analyzer mode

Analyzer mode (the sandbox) handles `CRYPTO` like the web, with these points
to know:

- Crypto trades around the clock: there is no MIS square-off for `CRYPTO`, and
  weekends count as trading days for a crypto book.
- Margin is the notional value divided by the sandbox's futures leverage
  (Sandbox settings, not the Leverage page). In the desktop the notional
  includes the contract value, the same multiplier P&L uses; the web leaves it
  out of margin and so blocks far more margin than the position needs.
- Sandbox capital is in rupees and crypto prices are mostly in US dollars. The
  sandbox does not convert: a margin or P&L of 60 (dollars) is booked as 60
  against the rupee capital, in the desktop and on the web.
- Quantities are whole numbers only (see Quantity above).

See [docs/user/sandbox.md](../user/sandbox.md) for analyzer mode in general.

## Master contract columns for crypto

Crypto rows use the same table as every other instrument
([symbol-format.md](symbol-format.md)):

| Field | Crypto meaning |
| --- | --- |
| `symbol` | OpenAlgo symbol, for example `BTCUSDFUT` |
| `brsymbol` | Delta's symbol, for example `BTCUSD` |
| `name` | The underlying asset, for example `BTC` |
| `exchange` | `CRYPTO` |
| `brexchange` | `DELTAIN` |
| `token` | Delta's product id |
| `expiry` | `DD-MMM-YY` for dated futures and options; empty for perpetuals and spot |
| `strike` | Option strike; `0.0` otherwise |
| `lotsize` | Minimum order size (see above) |
| `instrumenttype` | `PERPFUT`, `FUT`, `CE`, `PE`, `SPOT` and the others above |
| `tick_size` | Minimum price step |

The crypto master follows the UTC day: it downloads again after 00:00 UTC,
when Delta lists the new day's contracts.
