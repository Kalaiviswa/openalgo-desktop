# Changelog

All notable changes to OpenAlgo Desktop are recorded here. Versions follow
[Semantic Versioning](https://semver.org/).

## 1.0.3

A built-in ngrok tunnel for TradingView, Chartink and GoCharting alerts,
steadier Kotak and INDmoney market data, API answers brought in line with
OpenAlgo web, and screens that say what the desktop really does. If you run
1.0.2, OpenAlgo offers this update in the app; choose Update when no live
strategy is running.

### Built-in ngrok tunnel

- Paste your ngrok authtoken and your free ngrok domain in Profile, Broker
  tab, Server Configuration, turn on Enable Ngrok Tunnel, and TradingView,
  Chartink and GoCharting alerts reach OpenAlgo Desktop with nothing else to
  install.
- The tunnel always opens on your one ngrok domain, never on a random
  address, so the webhook addresses you paste into TradingView, Chartink and
  GoCharting never change. Host Server URL becomes `https://<your domain>`.
- It opens when you sign in, closes when you sign out or quit, reconnects by
  itself, and its status says what is wrong: the authtoken, the domain,
  another ngrok agent already using the domain, or no internet.
- Only the API, the strategy and Chartink webhooks and MCP are reachable
  through it. The OpenAlgo pages and their login stay on your computer, and
  a broker's sign-in redirect that arrives through the tunnel is passed back
  to OpenAlgo on your computer.
- With the tunnel on, the broker sign-in address is
  `https://<your domain>/<broker>/callback`, as on OpenAlgo web: register it
  with your broker, or keep a fixed local address on Broker Configuration.
- If Enable Ngrok Tunnel was already on, the status asks for your authtoken
  and domain; nothing changes until you add them.
- Strategy webhook URLs show your Host Server URL instead of 127.0.0.1.

### Market data

- Kotak: live prices no longer pause and reload when the market is quiet.
  The feed keeps its connection alive like OpenAlgo web and replaces a dead
  one within 40 seconds.
- Kotak: the option chain, order window and market depth move with every
  price update, not only when the order book changes.
- INDmoney: switching a symbol between quote and depth no longer interrupts
  its prices.
- Dhan, Samco, IIFL Capital and Nubra: quiet feeds stay connected instead of
  reconnecting every minute and a half.
- Health page: a new Broker Feed Connections card shows each broker feed's
  state, how often it reconnected and why.

### Compatibility with OpenAlgo web

- API rate limits count each endpoint on its own, as on OpenAlgo web: ten
  orders and ten cancels in the same second are no longer refused together.
  Option Greeks and WhatsApp alerts are limited to 30 a minute, as on the
  web.
- When your broker's rate limit would make a request wait more than 10
  seconds, OpenAlgo answers "too many requests" with a sentence telling you
  to try again in a few seconds, and nothing is sent to the broker.
- History from Historify works with just your API key, before you log in to
  your broker.
- In Semi-Auto, API tools can no longer switch analyzer mode; switch it in
  the app.
- The instruments CSV shows a crypto contract's exact lot size (0.0001), as
  the JSON answer does.
- Strategy API: the events filter accepts every event kind OpenAlgo writes,
  and a strategy whose run cannot be read reports an error instead of
  looking stopped.

### Screens

- Up and Down arrow keys move through the chart watchlist in the trading
  terminal and load each chart (#198).
- Profile, Broker, Server Status shows what is really running (the app, the
  market data server, the broker feeds, order checks and health checks) and
  what to do when a part is down.
- A Semi-Auto order placed from the chart or an order ticket says it is
  waiting for your approval in Action Center, instead of showing an error.
- The FAQ lists all 37 brokers, Rupeezy included.
- The Remote MCP, two-factor and TOTP screens no longer mention restarts,
  backup codes or password resets the desktop does not have.

### Health, monitoring and security

- Health Monitor: a strategy exit or stop that waits more than 15 seconds to
  be placed raises an alert, which clears itself once the exit is placed.
- Traffic Monitor: the Error column of the CSV export says why a request was
  refused, for example "Invalid openalgo apikey" or the missing field.
- Security: the Repeat Limit help says it applies to the bans you enter on
  the Security page; automatic bans always expire.

### Chartink, Telegram and P&L Tracker

- Chartink alerts read BUY, SELL, SHORT or COVER from the alert name, as the
  Chartink pages say, and still from the scan name.
- Chartink bulk import accepts the example header line and tells you how many
  stocks were added.
- Telegram's Rate Limit setting now works: alerts above it wait their turn
  and arrive in order.
- P&L Tracker shows today's MTM as a percentage of your sandbox capital in
  analyzer mode, or of your account margin when live, instead of a fixed
  1,00,000.

### Brokers

- Fyers loads NSE debt, warrant and InvIT series into the symbol master, and
  Angel drops non-tradable bond and index rows, types its index rows EQ and
  names the SENSEX 50 index SENSEX50, as Zerodha does. A port of
  marketcalls/openalgo#2280, contributed by Kalaiviswa (#207).

### Reliability

- OpenAlgo Desktop no longer fails to start when the computer is very busy
  at launch; it waits for its databases instead.
- Strategy pages and the strategy API no longer hold up live price handling
  while a long history loads.
- Strategy fills recovered from the broker's order book no longer pause
  price handling when the database is busy, and they reach each strategy in
  the order the broker reported them.
- On Linux, OpenAlgo returns memory it no longer uses to the system every
  minute, so its memory grows about half as fast through a trading day.

### Maintenance

- Dependency updates (React Router 8.5, Vite 8.3.5, Biome 2.6.2, Tauri CLI
  2.12.4, tokio, rmcp and others). Dependabot now waits 7 days after a
  release before proposing it, as OpenAlgo web does (marketcalls/openalgo
  24d81412b), and holds DuckDB at 1.x until Historify moves to DuckDB 2.0 as
  a planned upgrade.
- The release checks no longer fail at random: four tests that raced their
  own timers now wait for what they check, and a slow Ubuntu mirror fails
  its step with the cause instead of stalling the run.

### Known limitations

- Kotak positions do not yet carry OpenAlgo web's carry-forward valuation
  marker in the position book.
- AliceBlue and Angel market data keep their 1.0.2 keepalive until it is
  checked against live accounts.

## 1.0.2

Safety fixes from an independent production audit of 1.0.1: strategy stops
that see every price, orders that cannot split or switch between Live and
Sandbox on the way, scalping stops and order tracking that keep working
while OpenAlgo's database is busy, and a Health page that never shows a list
it could not read as empty. If you run 1.0.1, OpenAlgo offers this update in
the app; choose Update when no live strategy is running.

### Strategy stops and targets

- Every streamed price is checked against a strategy's stops, targets and
  trailing stops as it arrives. Under heavy market data 1.0.1 combined
  waiting prices, so a trailing stop could miss the price that should have
  exited it.
- An exit closes only the position it was decided for: a leg that re-entered
  meanwhile (a webhook flip) keeps its new position.
- Leg exits decided while a strategy stop is waiting are placed after it when
  the stop could not place its own orders, instead of being dropped.
- Reading prices never waits on OpenAlgo's database: a slow read for one
  strategy no longer delays the prices of the others.
- Health shows "Strategies with exits waiting" and "Longest exit wait", and
  warns when a decided exit has waited more than 15 seconds to be placed.

### Orders

- A split order is counted before any part of it is built; one that would
  need more than 100 orders is refused at once, as on OpenAlgo web.
- An options order, single or multi-leg, goes wholly to Live or wholly to
  Sandbox, decided once when it starts; switching analyzer mode while it is
  being sent no longer splits it between the two.
- A multi-leg options order whose leg got no definite answer from the broker
  says so in its reply, and an Action Center approval in that state stays
  unconfirmed, across a restart too. The remaining legs are still sent, as
  on OpenAlgo web.
- A GTT quantity that is zero, negative, blank, fractional or larger than
  any exchange accepts is refused on every exchange, from the GTT page's
  modify as well.

### Scalping, order tracking and OpenScript

- Scalping stops keep being checked while OpenAlgo's database is busy, and
  saving one stop's exit no longer holds up another stop.
- When the scalping stops cannot be read, OpenAlgo tries again and tells you
  after three failures, instead of silently not watching a stop you just
  saved.
- A scalping exit that would need more than 100 orders is refused with an
  alert and the stop stays managed, instead of sending hundreds of orders
  from a wrong position size.
- Order tracking keeps an order unconfirmed when the records it needs cannot
  be read, instead of treating it as settled.
- An OpenScript live run trades with the inputs it started with. A start
  whose saved parameters cannot be read is refused with the reason, instead
  of running the script's default quantity and risk.

### Health page

- A card that could not be refreshed keeps what it last showed, marked with
  when it was read; a list that was never read says so instead of looking
  empty. Unconfirmed orders no longer disappear after one failed refresh.
- Each figure updates as soon as it is read; a read that stalls is given up
  after 8 seconds and marked.
- A WARN or FAIL stays visible after a failed refresh, and times from an
  earlier day show their date.
- Market data skipped by a slow reader counts since OpenAlgo started, so a
  reconnect no longer hides it, and market depth is included. Database
  growth is shown over the last day.

### Security

- An API key is never read back from the window's browser storage,
  whichever version of OpenAlgo wrote it there.

### Releases

- Updates reach you sooner: a release now ships the installers CI built and
  tested for that version, signing only their update files, in about 15
  minutes instead of 50. A published release can no longer be changed by a
  later run.
- Building OpenAlgo Desktop from source needs Node.js 22.23.4, 24.22.1 or
  26.12.1 or later, following OpenAlgo web (marketcalls/openalgo f3e7de829).
  The app itself ships no Node.js.

## 1.0.1

The first update after 1.0.0. It fixes the limitations listed in 1.0.0,
makes busy pages stop slowing the app's handling of market ticks, and shows
more of the app's own health. If you run 1.0.0, OpenAlgo offers this update
in the app; choose Update when no live strategy is running.

### Windows

- OpenAlgo starts on a Windows PC that has never had Microsoft's Visual C++
  runtime installed. 1.0.0 stopped there with "MSVCP140.dll was not found";
  the app now carries that runtime inside it. A 1.0.0 that does not start
  cannot offer the update: download the 1.0.1 setup program and run it over
  1.0.0, which keeps your data.

### Order safety

- Delta Exchange: a smart order with a fractional target position is decided
  in exact quantities (holding 2 with a target of 0.5 sells exactly 1.5),
  never rounded down. Everywhere else, and in sandbox mode, a fractional
  target is refused instead of rounded.
- A GTT order with a fractional quantity is refused, when placing and when
  modifying, live and in sandbox mode.
- An options split order holds back its remaining chunks after one whose
  outcome is unknown, as a split order already did; check the order book
  before sending them again.
- Options orders, options multi-order legs and GTT orders refuse a quantity
  above 2,147,483,647, as placing an order already did.

### Security

- Your OpenAlgo API key is no longer saved in the app window's browser
  storage on disk. It is kept in memory while you are signed in, and a copy
  left there by 1.0.0 is erased the first time 1.0.1 starts.
- After you generate a new API key, the books and order forms use it at once;
  before, they kept sending the old key until the window was reloaded.
- WhatsApp alerts use ctutils 0.4.5, the release that fixes advisory
  RUSTSEC-2026-0385 (encryption code whose timing could reveal secrets).

### Speed under load

- The log, traffic, health, security, diagnostics, MCP audit and analyzer
  pages read, shape and send their answers off the threads that carry the
  market data feed. With a large log open in several browsers, strategy
  decisions were lost before; now none are, with decisions in about 5 ms.
- Strategy order books and history, OpenScript deployment books, the Action
  Center, the analyzer status and the Telegram and WhatsApp statistics do the
  same, and pages that refresh the same data share one read.
- The first API call after a start, a sign-in or a new key checks the key
  off the threads that carry the market data feed, so a burst of API calls
  no longer delays the handling of market ticks.

### Health

- The Health page shows each database's size and growth, what each reader of
  a broker feed had to skip, records that could not be saved, strategy price
  counts and order tracking, and turns amber or red with a plain sentence
  when anything was skipped or lost.
- Alerts that nothing clears on its own have a Resolve button. An open alert
  shows what is wrong now, not what was wrong when it opened.
- Health and Run diagnostics count market data dropped for apps on the 8765
  feed, and the size of every database including sandbox and Historify.
- The analyzer status and the WhatsApp statistics answer "could not be read"
  when their log cannot be read, instead of showing zero.

### Accessibility and wording

- Every signed-in page passes an accessibility scan with no exceptions, in
  light and dark: clearer contrast for margin values, statuses, password
  rules, toast messages and the Tools tiles, and labels for screen readers.
- The strategies panel says what is true in the desktop: a live run is a
  hidden app window, and quitting or signing out pauses every run and leaves
  its positions. The scalping badge reads One-Click on or off.

## 1.0.0

The first release of OpenAlgo Desktop: a single-user desktop version of
OpenAlgo web for Windows, macOS and Linux, with no server to set up, no Python
runtime and no `.env` file. Everything is set inside the app.

Installers: Windows x64 (NSIS, per user), macOS Apple Silicon (M1 to M4) and
Intel (`.dmg`), Linux x64 and ARM64 (AppImage and `.deb`). Raspberry Pi is not
a supported device for the desktop app; use OpenAlgo web on a Pi.

### Compatibility with OpenAlgo web

- `/api/v1` on `http://127.0.0.1:5000` answers like the web, field for field:
  every endpoint the web documents, pinned by contract tests against responses
  recorded from a live web instance. The Python SDK, TradingView, Amibroker,
  Chartink, GoCharting, Excel and MCP clients work unchanged.
- The market data WebSocket on `ws://127.0.0.1:8765` speaks the web protocol:
  LTP, Quote and Depth modes, depth 5, 20, 30 and 50, and the order update
  stream.
- Broker redirect URLs follow the web convention,
  `http://127.0.0.1:5000/<broker>/callback`.
- The OpenAlgo web screens carried over nearly verbatim, including the
  `/trading` terminal on openalgo-charts 2.6.7 with custom intervals and daily
  study history, and Strike Profile (GEX and DEX by strike and expiry, exactly
  as on the web).

### Brokers

- All 37 OpenAlgo web brokers: Zerodha, Fyers, Upstox, Dhan and Dhan
  (Sandbox), Kotak Securities, Groww, Angel One; the Symphony XTS family
  (5 Paisa (XTS), JainamXts, CompositEdge, RMoney, Ibulls, IIFL, Wisdom
  Capital); the Noren family (Shoonya, Flattrade, TradeSmart, Zebu) and
  Firstock; Arrow, Paytm Money, Pocketful, HDFC Sky, HDFC Securities, Rupeezy;
  5 Paisa, Tradejini, Nubra, IndMoney, IIFL Capital; Alice Blue, Definedge,
  mStock, Motilal Oswal, Samco; and Delta Exchange for crypto.
- Books, trades, positions and holdings return OpenAlgo symbols for every
  broker. A broker reply that cannot be read is an error, never an empty book
  or a flat position, so close-all never reports "all squared off" while a
  segment is unread.
- Broker keys are entered in the app; several brokers can be saved and one is
  connected at a time. Sessions resume after sign-in until the daily 03:00 IST
  cut-off.
- Orders are paced at each broker's published rate; Samco, Tradejini, HDFC Sky,
  HDFC Securities, Pocketful and 5 Paisa, which publish none, at 10 a second.

### Order safety

- A placement has three outcomes: accepted, refused, or unknown. An order whose
  outcome is unknown is never sent again; it is settled from the broker's order
  book, and new entries wait until the gap in order facts is repaired.
- Strategies, OpenScript runs, scalping stops and Action Center orders are tied
  to the broker account they were opened on. An exit is never sent through
  another account, and a run reports what it holds elsewhere.
- A strategy page that loses the app's answer keeps the order pending and asks
  again with the same identity, so a lost answer never becomes a second order.
- Semi-Auto orders approved in the Action Center are followed by their run; an
  unreadable Semi-Auto setting refuses the order instead of sending it.
- Strategy ticks never wait on the database, and a deciding price is never
  lost under load. Stop-loss saves wait for a busy database instead of failing.

### Trading

- Sandbox mode (analyzer mode) mirroring the web's sandbox: 1 crore starting
  capital, margin blocking, fills from live ticks, weighted-average netting,
  T+1 settlement, MIS auto square-off, GTT and catch-up after the app was
  closed, kept in its own database.
- Strategy module and risk management with a pure risk core that passes the
  web's risk vectors; strategy webhooks for TradingView and GoCharting.
- Chartink strategies and webhooks; Action Center for Semi-Auto orders.
- The `/trading` charting terminal with custom indicators, OpenScript files and
  live OpenScript runs on the same engine as backtests.
- Scalping terminal with its risk monitor.
- Options tools: option chain, Greeks, OI tracker and range, max pain, IV
  chart, IV smile, volatility surface, GEX, gamma density, Strike Profile,
  straddle charts, strategy builder and arbitrage.
- Historify on DuckDB with download jobs, scheduler and exports.
- Playground, API key management, P&L tracker, leverage settings.

### Alerts and AI clients

- Native Telegram bot with commands, alerts and analytics.
- Native WhatsApp bot with QR and pair-code pairing and alerts.
- Native MCP server with the web's 49 tools, over stdio for Claude Desktop and
  Claude Code and over HTTP at `/mcp`; scoped tokens created on the API Key
  page, per-token limits and an audit log.

### Monitoring and settings

- Live, sandbox, security, traffic and latency logs; Health Monitor; Run
  diagnostics, including how fresh each broker feed is.
- Admin pages for freeze quantities, holidays, market timings, diagnostics
  and Remote MCP.
- Server Settings for ports, bind address and access from other devices. A
  port already in use is reported in the app with the fix, including the
  AirPlay Receiver case on macOS.

### Security

- No OS keychain on any platform. The data key is derived from your OpenAlgo
  password and unlocked when you sign in; after a restart the API, webhooks
  and MCP stay locked until you sign in.
- Broker credentials and tokens encrypted with AES-256-GCM, each bound to its
  row and column.
- Passwords and API keys hashed with Argon2id; API keys found through an HMAC
  index with a short cache.
- Signed-in user required on every non-public route; browser origins compared
  exactly; strict Content Security Policy; devtools off in release builds.
- Broker sign-in verified by single-use `state`, with account binding for
  brokers whose redirect does not return it.
- Sign-in and API key failures throttled per address; `/api/v1` limited as on
  the web.
- Logs keep API keys, tokens, OAuth codes, passwords and TOTP secrets out.

### Updates and your data

- In-app updates: OpenAlgo checks for a newer version at start and from
  Check for Updates in the profile menu, shows the release notes, and
  downloads nothing until you choose Update. The update's signature is
  checked before it is installed, and a restart that would interrupt Live
  strategies, OpenScript runs, scalping protection or open orders is
  confirmed first. macOS, Windows and AppImage copies update themselves; a
  `.deb` install is pointed to the release page.
- Your data is backed up before a new version changes it, put back if the
  new version cannot open it, and the copy is deleted as soon as the new
  version has opened your data. An older version refuses data that a newer
  one has used, and changes nothing.
- Every release is built from a commit whose full test run passed on every
  platform, including the installed app on each system and a sandbox order
  placed through the app's own window on Linux and Windows.

### Documentation

- A documentation set organised like OpenAlgo web's, in `docs/`: the user
  guide (installation to the charting terminal), the full `/api/v1` and
  WebSocket reference, the MCP tool reference, and guides for contributors.
  Start at `docs/INDEX.md`.

### Known limitations

- The installers are not yet code-signed. Windows SmartScreen and macOS
  Gatekeeper ask you to confirm the first open; the README shows how.
- Not included: the Python Strategy Host, Flow, the Agent (including the
  Assistant panel in `/trading`), and the pandas backtesters (Portfolio
  Backtester, SIP Backtester, Portfolio Analyzer). Use OpenAlgo web for them.
- Fixes coming in the next update:
  - On Delta Exchange, a smart order with a fractional target position and a
    GTT with a fractional quantity are rounded down instead of refused. Use
    whole quantities until the update.
  - An options split order keeps sending the remaining chunks after one whose
    outcome is unknown; check the order book before resending.
  - With a very long order and log history, some log and book pages can slow
    the app's handling of market ticks while they load.
  - The Health page does not yet show market-data skips, database growth or
    records not saved; Run diagnostics does.
