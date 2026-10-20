# Performance and durability benchmarks

Measured results of the benchmarks for the roadmap's "Performance and
durability acceptance targets". Each benchmark runs the real services
(`AppState`, the HTTP server with Socket.IO, the 8765 feed server, the
event bus, the scalping monitor, the strategy module, Historify) with
`MockBroker` registered as `zerodha` and signed in through its OAuth
callback, a synthetic broker market socket and a broker order socket, in a
temporary data folder on ephemeral ports (never 5000, 8765, 5500 or 8766),
each alone in a child process.

## Targets

Measured on the supported floor hardware: an 8 GB Apple Silicon Mac (the
agents measure here) and a mid-range Windows laptop (the maintainer
measures there). The Raspberry Pi is out of scope (maintainer, 2026-10-16).
Log retention matches the web (maintainer, 2026-10-15): the order and
analyzer logs are kept forever, so the disk bound is the monitoring tables.

| Measure | Target | Benchmark |
| --- | --- | --- |
| Feed tick to risk decision (stop, target, trailing) | p99 under 50 ms at 1,000 subscribed symbols and 2,000 ticks/s | `perf::tick_to_risk` (scalping monitor and strategy module) |
| Order-socket reconnect to book reconciled | under 5 s | `perf::reconnect` |
| Order-path SQLite wait while Historify imports | p99 under 20 ms | `perf::sqlite_wait` |
| Critical event delivery (order, position, risk topics) | zero drops under a 10x burst; market-data drops counted and shown | `perf::event_burst` |
| Memory and descriptors over 8 h | flat within 5% after warm-up | `perf::endurance` (15 min here; 8 h by the maintainer) |
| Disk growth | monitoring tables bounded; growth reported in health | `perf::disk_growth`, `perf::endurance` |

## Running them

All are `#[ignore]`d and never run on pull requests. From `src-tauri`:

```
# every benchmark, one at a time (about 30 minutes in a debug build)
cargo test --locked --test it perf:: -- --ignored --nocapture --test-threads=1

# one of them
cargo test --locked --test it perf::tick_to_risk -- --ignored --nocapture

# the endurance run for 8 hours, samples written as they are taken
OPENALGO_PERF_MINUTES=480 OPENALGO_PERF_OUT=$PWD/endurance.jsonl \
  cargo test --locked --test it perf::endurance -- --ignored --nocapture
```

On this Mac every cargo command goes through the build lane
(`.lane/cargo-lane.sh`), as for any other test.

Each measurement prints one line:

```
PERF bench=<name> <key>=<value> ... build=debug os=macos arch=aarch64 cpus=8 mem_gib=8.0 load1=5.62 target=<what> verdict=<pass|miss>
```

`load1` is the one-minute load average when the line was printed: the
development Mac is shared with other agents' builds, so it says how much
else ran. With `OPENALGO_PERF_OUT=<file>` the same fields are appended to
the file as one JSON object per line. A benchmark asserts its target after
printing, except the two reporting gaps listed under follow-ups, which
print `verdict=miss` without failing the run until they are closed.

`.github/workflows/perf.yml` runs them nightly at 03:00 IST on
ubuntu-22.04 and on demand (`workflow_dispatch`, with the endurance length
and a test filter as inputs), and uploads `perf-results` (the JSON lines
and the log); the run summary lists the `PERF` lines.

The faults the endurance run injects, and the maintainer's drill on a real
install with the expected outcome of each fault, are in
[`fault-drill.md`](fault-drill.md). `scripts/perf/sample_app.py` samples a
running install for the 8-hour run.

## Layout

| File | What it measures |
| --- | --- |
| `tests/it/perf/mod.rs` | percentiles, the `PERF` line, the JSON record, process memory |
| `tests/it/perf/rig.rs` | the app under load: synthetic broker market socket (paced ticks, stamped ticks, drops), broker order socket, running clock |
| `tests/it/perf/tick_to_risk.rs` | tick to stop, target and trailing decisions |
| `tests/it/perf/reconnect.rs` | order-socket drop to order book reconciled |
| `tests/it/perf/sqlite_wait.rs` | order-path writes and `/api/v1/placeorder` while Historify imports |
| `tests/it/perf/event_burst.rs` | critical-lane delivery under a 10x burst; market-data drops |
| `tests/it/perf/disk_growth.rs` | monitoring tables and `logs.db` over repeated overfills; sizes in health |
| `tests/it/perf/endurance.rs`, `faults.rs` | the timed run with faults |
| `tests/it/perf/lag_log.rs` | ticks the app's consumers skipped, counted from their log lines (the app keeps no count) |

## Results

### 2026-10-20, macOS 14 (M1, 8 GB), debug build, branch `pr/benchmarks` on master `92689307`

The Mac is shared with other agents' builds, so every line carries the
one-minute load average (`load1`); 8 cores. These are debug builds: the
shipped release build is faster, so a pass here is a pass there, and a miss
is checked against the cause before it is called a product miss.

| Measure | Target | Measured | Verdict |
| --- | --- | --- | --- |
| Tick to decision, scalping monitor (300 legs: 100 stops, 100 targets, 100 trailing) | p99 under 50 ms | p50 4.2-5.3 ms, p99 10.1-18.3 ms, max 22-139 ms over 400 decisions, none lost (load 4.7-8.9) | pass |
| Tick to decision, strategy module (300 live runs, one leg each) | p99 under 50 ms | on master: p99 24.6 ms at load 4.6, 115.8 ms with four extra busy cores, 345 ms at load 3.9 in another run, 1,879 ms with 4 of 400 decisions lost at load 9.3; with the fix on `pr/strategy-tick-path` (`cee33a75`): p99 3.6 ms at load 4.6, 19.4 ms with four extra busy cores, none lost, no tick folded or missed | miss on master; pass with the fix |
| Order-socket reconnect to book reconciled | under 5 s | 0.31-0.50 s over 6 drops (each after a connection that lived 31 s): the reconnect backoff's first wait (250-500 ms) plus one order-book pass; a fill made while the socket was down was applied every time. A flapping line (drops 1 s after each reconnect): 0.61, 1.09, 3.13 s | pass |
| Order-path SQLite wait while Historify imports | p99 under 20 ms | stop save to `openalgo.db`: p99 3.29 ms (max 26.6 ms) while 3 imports of 200,000 rows ran (20,000 rows/s), against p99 1.24 ms without; `/api/v1/placeorder` p99 6.6 ms. Before the IMMEDIATE fix (PR #160, `fix(db)`) the save failed with "database is locked" within 30 s, imports or not | pass (after the fix) |
| Critical event delivery | zero drops under a 10x burst | 11,000 events (100/s, then 1,000/s for 10 s, then 100/s), every one delivered to the critical-lane probe: no critical drop, panic, closed queue or fact loss; deepest critical queue 36 of 10,000; delivery p99 0.28-0.89 ms; no best-effort entry shed | pass |
| Market-data drops | counted and shown | a feed client reading one frame every 2 ms missed 84% of 2,000 ticks/s (the feed keeps the latest per symbol, by design); the counts live per connection in memory and vanish with it, and health shows none | miss (reporting) |
| Disk growth: monitoring tables | bounded | six cycles writing 20% over every cap: every table at its cap after housekeeping; `logs.db` 99 MB after the first cycle, 141 MB after the second, then flat (141,227 to 141,255 KiB) | pass |
| Disk growth: reported in health | sizes and growth in health | every database's size, the sandbox's included since PR #160 (`fix(health)`); no growth figure | miss (reporting) |
| Memory and descriptors | flat within 5% after warm-up | 15-minute smoke (the harness check; 3-minute warm-up, 12 faults injected, every one handled as expected, no activity or API failure): memory 44,370 to 47,650 KiB (+7.4%), still rising about 15 MB an hour at the end; descriptors 50 to a peak of 53, pool 8, live tasks 45 to 47; data folder +34 MB an hour (the request log at 20 calls a second, until its 100,000-row cap). A 60-minute run (6-minute warm-up, 54 faults, every one as expected, no activity failure): memory 44,530 KiB at the warm-up, 49,138 at minute 30, then 49,922 from minute 50 to 60; the first half after the warm-up grew 4.6 MB, the second 0.8 MB: a plateau about 30 minutes in, so +12.1% over a 6-minute warm-up and +1.6% from minute 30. Live heap allocations (macOS `heap`) grew 0.40 MB from minute 13 to 30 and 0.13 MB from 30 to 45 while the footprint grew 2.8 and 0.7 MB: caches filling and the allocator keeping freed pages, not a line. SQLite's own memory stayed between 1.5 and 6.8 MB. Descriptors 50 to 55 with the pool 4 to 8 (two per connection), live tasks 47 throughout. 35 of about 72,000 `/api/v1` calls did not answer 200 in that run, most likely the harness's own catch-up bursts after a stall tripping the 100-a-second limit; it now paces its calls without bursts and records failures by status, and a 30-minute run with that change had none (27 faults, every one as expected) | harness proven; plateau within the hour; 5% verdict for the maintainer's 8-hour run (48-minute warm-up) |

The strategy module's miss on master: one task read the market-data
broadcast and awaited every held tick's evaluation, which scanned every
active run and, for the run holding the symbol, made two synchronous
SQLite reads and copied the strategy and the run (0.18 ms a tick with 30
runs, 0.29 ms with 180, 0.39 ms with 300, measured by direct calls); under
CPU contention it queued, and at load 9 the broadcast overran it and
skipped the deciding ticks (logged at debug, never counted). The slow
decisions were spread over the run, not clustered at the reconciler's
5-second pass. Capacity on master at load 3.5: 30 legs p99 6.7 ms, 90 legs
5.3 ms, 180 legs 15.5 ms. The trading agent's fix (`pr/strategy-tick-path`)
keeps no database on the tick path, splits a reader from the evaluator with
each symbol's prices folded so a spike is still decided, and counts queued,
folded and missed ticks in health; this benchmark is its acceptance check.

A first 45-minute run found a harness race, not an app defect: each
scalping round sent its breach tick once, and for symbols the SDK client
did not hold, the tick could reach the synthetic broker socket before the
monitor's subscription did, so the round never exited and every later
round failed. The harness now repeats the breach tick until the exit is
placed, as a market that fell through a stop keeps ticking under it; that
run's memory numbers are not used.

### Follow-ups

1. **Strategy tick path**: fixed on `pr/strategy-tick-path` (`cee33a75`,
   trading follow-ups agent); measured above. Once it lands, the nightly
   run holds it.
2. **Market-data drops shown in health** (observability): a feed-wide count
   of the frames each 8765 client's outbox coalesced or dropped (kept today
   per connection and lost when it closes), the bridge's and feed server's
   "fell behind; skipped N" (log lines only), and the scalping monitor's and
   sandbox forwarder's `Lagged` skips (debug log only), in a health block
   with an alert, like the strategy module's new `strategy_ticks`. The
   benchmarks count the log lines meanwhile (`tests/it/perf/lag_log.rs`).
3. **Data folder growth in health** (observability, UI): the folder's total
   and its growth over 24 hours, from the sizes every health sample already
   stores (`health_metrics.db_sizes`), on the Health page, which today shows
   no sizes at all.
4. **Release-build numbers**: every figure here is a debug build; a release
   run of `perf::` on the M1 and the maintainer's Windows laptop would give
   the shipped numbers.
5. **The weekly soak's one-time memory step** (soak run 39090876797): the
   CI-and-evidence agent is tracing it in `docs/audit/soak.md`; the endurance
   run measures after its warm-up.
