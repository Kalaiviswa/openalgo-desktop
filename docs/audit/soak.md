# Soak runs

Measured results of the leak soak tests. Both are `#[ignore]`d in the normal
suite and run weekly (and on demand) by `.github/workflows/soak.yml` on
ubuntu. Run locally with:

```
cd src-tauri
cargo test --test it soak -- --ignored --nocapture
```

## Results per commit

Every soak run that measured a commit, newest first. A release cites the
row for its commit (or the nearest earlier one, with the commits between
named). Each scheduled run uploads its log as the `soak-log` artifact.

| Date | Commit | Where | Session soak (150 cycles) | Feed soak (1000 clients) | Run |
| --- | --- | --- | --- | --- | --- |
| 2026-10-20 | this change on `92689307` | CI, ubuntu-22.04, debug build, dispatch | AFTER_ROW | not run | AFTER_RUN |
| 2026-10-20 | `92689307` plus per-cycle diagnostics | CI, ubuntu-22.04, debug build, dispatch | diagnostic: the 2026-10-18 step reproduced in the warm-up (RSS 174,712 KiB at cycle 1 to 273,320 at cycle 9 in five steps of about 19,600 KiB, then flat); heap in use 10 to 12 MiB throughout; see below | not run | [39358239967](https://github.com/marketcalls/openalgo-desktop/actions/runs/39358239967) |
| 2026-10-18 | `32fcbcf` | CI, ubuntu-22.04, debug build, scheduled | **fail**: memory grew 118,504 KiB over the run (bound 48 MiB): RSS 232,532 KiB at cycle 10 to 350,180 at cycle 20, then flat to 351,036 (+384 KiB in the second half); descriptors 41 flat, pool 8 flat, tasks 23 flat. Cause below: Argon2 working memory kept by glibc, not a leak | pass: descriptors 11 -> 11, RSS +2,292 KiB | [39090876797](https://github.com/marketcalls/openalgo-desktop/actions/runs/39090876797) |
| 2026-10-11 | `272d460` | CI, ubuntu-22.04, debug build | pass: descriptors 31 -> 30, pool 3 -> 3, RSS 265,180 -> 305,952 KiB (plateau from cycle 40), tasks 21 -> 22 | pass: descriptors 11 -> 11, RSS +2,416 KiB | [38106625181](https://github.com/marketcalls/openalgo-desktop/actions/runs/38106625181) |
| 2026-10-08 | not recorded | macOS 14 (Apple Silicon), debug build | pass (below) | not recorded | local |

## Broker session soak (`tests/it/soak.rs`)

One cycle, on the real `AppState`, HTTP server with Socket.IO, and 8765 feed
server, ephemeral ports only, `MockBroker` registered as `zerodha` and a fake
broker market socket:

1. Broker sign-in (OAuth callback with the server-issued `state`).
2. Master contract load (4 symbols).
3. Broker feed connected; an 8765 client authenticates, subscribes to SBIN
   and receives streaming ticks (one every 25 ms).
4. One live order (mock broker) and one sandbox order through
   `/api/v1/placeorder`.
5. The broker socket is dropped without a close frame; the feed reconnects,
   resubscribes and ticks resume.
6. 110 `/api/v1` requests over 14 endpoints (ping, funds, orderbook,
   tradebook, positionbook, holdings, quotes, depth, symbol, search,
   analyzer, intervals, multiquotes, openposition), every tenth on a fresh
   connection, paced under the 100 per second limit.
7. Two signed-in Socket.IO clients connect and leave; one without a session
   is refused.
8. The 8765 client leaves (unsubscribe and close frame, or a dropped socket,
   alternating).
9. Logout: session tasks, feed, registries, symbol cache and Socket.IO
   sockets all back to zero.

Samples are taken after 10 warm-up cycles and every 10 cycles after that.
Assertions: descriptors at the end at most the warm-up count plus two per
database pool connection added since (r2d2 refills `min_idle`; each holds the
database and its WAL) plus 4; live tokio tasks
(`RuntimeMetrics::num_alive_tasks`) at most the warm-up count plus 4; memory
growth at most 48 MiB, and the second half of the run growing no more than
half the first half plus 4 MiB (a plateau passes, a line fails). Memory is
the resident set on Linux and Windows and the physical footprint on macOS
(`proc_pid_rusage`, resident plus compressed): macOS compresses idle pages,
so its resident set alone falls and rises with no change in use, and an
earlier run asserting on it failed on that noise.

### 2026-10-18 failure: Argon2's working memory kept by glibc (fixed)

The scheduled run at `32fcbcf` failed its memory bound with a step, not a
line: about 118 MB between cycles 10 and 20, then flat. A dispatched run
with per-cycle diagnostics (run 39358239967: `mallinfo2`, SQLite's own
`sqlite3_memory_used`, `/proc/self/status` threads and `/proc/self/smaps`)
named it:

- Heap in use (`uordblks`) stayed between 10 and 12 MiB over 160 cycles;
  SQLite's own memory stayed at about 2.3 MiB; 16 threads throughout. No
  object was leaked, and no cache grew.
- What grew was free memory that glibc kept (`arena` and `fordblks` rose
  together), in steps of about 19,600 KiB: one Argon2id working set
  (`security/hashing.rs`, 19,456 KiB with the account's parameters) plus
  bookkeeping. The anonymous mappings holding it were glibc's per-thread
  arena heaps, two of them at about 58 MB each at the plateau.
- Why: the first 19 MiB block came from `mmap` and was returned, but freeing
  it raised glibc's dynamic mmap threshold (and its trim threshold to about
  38 MiB), so every later block came from the running thread's arena and
  stayed there after it was freed, some arenas keeping several. The plateau
  depends on which threads ran the hashes, which is why it fell inside the
  warm-up in one run and after it in another.
- Why since `272d460`: KEY-01 derives the key-vault key with Argon2 at each
  sign-in, and its security review pads every sign-in to the same number of
  Argon2 runs, so each soak cycle runs several more 19 MiB hashes, on more
  threads.

macOS and Windows return large frees to the system by themselves; this is
glibc only. The bound was right and stays. The fix: each Argon2 run gets its
working memory in pages of its own (an anonymous map, zeroed and unmapped
when the run ends, `security::hashing::Workspace`), so it never enters the
allocator. Regression: `security_memory::argon2_runs_on_many_threads_give_their_memory_back`
(Linux, its own process: twelve runs, four threads at a time, must leave
RSS within one run's 19 MiB). Hashes are byte for byte the argon2 crate's
own (known-answer tests, and a hash made before the change still verifies).

### 2026-10-11, CI (ubuntu-22.04), debug build, commit 272d460

Scheduled run 38106625181. 150 measured cycles after 10 warm-up cycles,
334 s.

| Cycle | Descriptors | Pool | RSS (KiB) | Live tasks |
| --- | --- | --- | --- | --- |
| 10 | 31 | 3 | 265,180 | 21 |
| 40 | 31 | 3 | 305,204 | 22 |
| 80 | 31 | 3 | 305,328 | 22 |
| 120 | 31 | 3 | 285,920 | 22 |
| 160 | 30 | 3 | 305,952 | 22 |

Resident memory rose 40 MiB in the first 30 cycles and then held between
285 and 306 MiB to the end: a plateau, not a line. Descriptors, pool
connections and live tasks stayed flat.

### 2026-10-08, macOS 14 (Apple Silicon), debug build

150 measured cycles after 10 warm-up cycles, 336 s.

| Measure | After warm-up (cycle 10) | End (cycle 160) |
| --- | --- | --- |
| Open descriptors | 37 | 33 |
| Database pool connections | 7 | 3 |
| RSS | 137,200 KiB | 138,048 KiB |
| Physical footprint | 30,626 KiB | 31,442 KiB |
| Live tokio tasks | 20 | 21 |

Footprint growth: 816 KiB in total, 672 KiB in the first half (cycles 10 to
90), 144 KiB in the second (90 to 160): a plateau. Descriptors read 37 at
every sample through cycle 150 and 33 at the end, once r2d2 had reaped four
idle pool connections (two descriptors each). Live tasks read 21 at every
sample from cycle 20 on.

No leak found in the app. Two test-harness defects were found and fixed:

- A broker sign-in reloads the settings, and in a debug build that load
  forces the development ports (5500 and 8766). Tests that set ephemeral
  ports only in memory then had the feed server's settings watcher move the
  listener to 8766, and a test that reloaded before starting its listeners
  bound 5500 and 8766. `AppState::pin_listener_ports` (test-support only) now
  keeps a test's ports through every reload. The soak asserts that it never
  binds 5000, 8765, 5500 or 8766 and that its listeners never move.
  `broker_session_e2e` has the same pin and a check after its sign-in.
- `MockBroker::calls` records every call without a bound. The soak clears
  it each cycle so the test's own bookkeeping does not show up as growth.

## Feed client soak (`tests/it/feed_soak.rs`)

1000 clients in waves of 100 connect, authenticate, subscribe, receive a
tick and leave. Descriptors at most the warm-up count plus 4, RSS growth at
most 16 MiB, and every registry entry and source subscription removed.
