# Order placement outcomes: per-broker status

Codex LOG-08. A placement has three outcomes: accepted (an order id),
refused (provably never sent, or the broker said no) and uncertain (sent,
with no definite answer). The rules live in
`src-tauri/src/brokers/common/outcome.rs`; this page tracks how far each
adapter goes beyond what it inherits from them.

## What every adapter inherits

- A transport error after the request may have been sent (a timeout, a
  dropped or reset connection) is uncertain. Only a connection that never
  opened, or a request that could not be built, is a refusal
  (`outcome::after_send`). This holds for every adapter whose transport
  error reaches the order service as `AppError::Http`.
- Inside a placement, `common::http::read_json` turns a 5xx (any body) and an
  unreadable non-4xx body into `AppError::Uncertain`.
- A success without an order id (`Ok` with an empty id) is uncertain.
- An uncertain placement is never placed again. The order reconciler looks
  it up in the broker's order book: by client tag when the adapter reports
  tags (`Broker::get_order_book_tagged`), otherwise by symbol, exchange,
  side, quantity and product among orders nobody owns, never earlier than
  the placement, and never by guessing between several candidates.

## Adapters refined so far

| Adapter | Refinement |
| --- | --- |
| zerodha | A client tag per order (`oa` and 16 hex digits) instead of the web's constant `openalgo`; an uncertain answer carries it; the tagged book reads it back. Deliberate departure from the web: the Kite order book shows the per-order tag. |
| angel | The per-order `ordertag` (web #2176) is carried by the uncertain answer and read back by the tagged book. An unmatched ambiguous reply is uncertain, not a refusal. |
| groww | A server error, a success without an id, and a transport failure after the order was sent are uncertain. |
| hdfcsecurities | A transport failure after sending, and a success without an id, are uncertain. |
| indmoney | "No order number in RS response" is uncertain (the web answers with the made-up id `ORDER_PLACED`, which no book carries). A success without an id is uncertain. |
| iiflcapital | A throttle answer to an order write is uncertain. |
| aliceblue, definedge, deltaexchange, dhan, fyers, kotak, motilal, pocketful, upstox | A success without an id (and for Definedge an unreadable answer) is uncertain. |

## Still to classify per broker

Each item below is an answer received after sending that the adapter still
reports as a refusal (`AppError::Broker`). Until refined, an automation that
meets it treats the order as not placed. File references are under
`src-tauri/src/brokers/`.

| Adapter | Remaining post-send refusals | Client tag |
| --- | --- | --- |
| aliceblue | none found | constant `orderTag: openalgo` |
| angel | any HTTP status with `status: false`, a 5xx included | per order, done |
| definedge | a 5xx with a JSON body; success without `norenordno` | none |
| deltaexchange | a 5xx JSON with `success: false`; an unreadable body; an undecodable `result` (`deltaexchange/mod.rs`) | none |
| dhan | a 2xx other than 200/201 | none (the API offers `correlationId`) |
| firstock | an unreadable body, a 5xx JSON (`firstock/mod.rs`) | constant `remarks` |
| fivepaisa | success with no or zero `BrokerOrderID` | constant `RemoteOrderID` |
| fyers | none found | constant `orderTag: openalgo` (per-order tag possible) |
| groww | a 2xx with an unreadable or non-SUCCESS body | per order `order_reference_id`, not read back |
| hdfcsky | none found | per order `user_order_id`, not read back |
| kotak | none found | per order `ig`, read into `order_tag` but no tagged book |
| motilal | an empty or non-JSON body, a 5xx JSON | none |
| mstock | an empty 2xx, an unreadable body, success without `data.orderid` | empty |
| nubra | a 2xx without `intentOrderId` | constant `stratTags` |
| paytm | an unreadable body, success without an order number | none |
| samco | an empty or non-JSON body, a 5xx JSON, Success without `orderNumber` | none |
| tradejini | `s: ok` / `no-data` without an id | none |
| upstox | a non-JSON body, a 5xx JSON | constant `tag: openalgo` (per-order tag possible) |
| zerodha | a success envelope with no `data` | per order, done |
| xts family | `type: success` without `AppOrderID` | constant `orderUniqueIdentifier` |
| noren family | none found (a missing id is already uncertain) | constant `remarks` (tradesmart) |

Next, in order: per-order tags read back for Upstox, Dhan (`correlationId`),
Fyers, Kotak and Groww; then the "success without an id" cases above, which
need only `AppError::uncertain` at the named line.
