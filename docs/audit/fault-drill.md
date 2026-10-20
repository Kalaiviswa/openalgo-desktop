# Fault drill and endurance run on a real install

The maintainer's runs for the "Performance and durability acceptance
targets" (`docs/audit/perf.md`): the 8-hour endurance run, the full trading
day and the weekend, each with faults injected by hand. The expected
outcome of every fault is written below **before** the run; the run records
what actually happened next to it.

The same faults, except disk full, are injected automatically into the
running app by the endurance harness (`tests/it/perf/endurance.rs`,
`tests/it/perf/faults.rs`) on a temporary data folder; this drill is for
the installed app with a real broker.

Machines: the 8 GB Apple Silicon Mac and a mid-range Windows laptop (the
supported floor; the Raspberry Pi is out of scope since 2026-10-16).

## Before the run

1. Install the candidate build. Note its version and commit (Profile >
   About, or the release page) in the record below.
2. Sign in, connect the broker, and set up a normal day: a watchlist and a
   `/trading` chart with about 50 symbols, an SDK or Amibroker client
   holding 200 to 1,000 symbols on the 8765 feed, analyzer (sandbox) mode
   with two scalping stops and one strategy run, and one small live order
   working if the broker allows it.
3. Find the app's process id: Activity Monitor (macOS) or Task Manager >
   Details (Windows), process `openalgo-desktop`.
4. Start the sampler in a terminal and leave it running for the whole
   run. It only reads process counters and file sizes:

   ```
   python3 scripts/perf/sample_app.py --pid <pid> --hours 8 --out endurance-<date>.csv
   ```

   Ctrl-C stops it early; `--summary endurance-<date>.csv` prints the
   verdict again later. The target: memory and descriptors (handles on
   Windows) within 5% of the end of the warm-up (the first tenth of the run,
   at least 30 minutes) to the end, and the data folder's growth per hour
   reported.

If memory is not flat, tell a cache filling from a leak before calling
it one: compare the second half of the run with the first (a plateau grows
less), and on macOS take `heap -s <pid>` at the end of the warm-up and at
the end. "All zones" is what is still allocated; when it grows far less
than the footprint, the growth is the allocator keeping freed pages, not
a leak. The endurance harness found a plateau about 30 minutes in
(`docs/audit/perf.md`).

## The faults

Inject one fault at a time, at least 15 minutes apart, during market hours
unless the row says otherwise, and note the time of each.

| # | Fault | How to inject it | Expected outcome (written before the run) |
| --- | --- | --- | --- |
| 1 | Broker feed disconnect | Turn Wi-Fi off (or pull the cable) for 60 s, then back on. | While off, the Health page shows the market feed reconnecting; prices freeze but no error dialog appears. Within 10 s of the network returning the feed reconnects and resubscribes every symbol; charts, the watchlist and the SDK client tick again. The order book is read again (Health: "order facts" back to OK). No order is placed twice; an order that filled while offline shows in the order book and positions after the reconnect. |
| 2 | Order-socket drop | For a broker with its own order socket (Fyers, Upstox, Dhan, Angel and others; Zerodha carries order updates on the market feed, so fault 1 covers it): block only the order socket's host for 60 s with the firewall (macOS `pf`, Windows Defender Firewall outbound rule), then unblock. If that is impractical, fault 1 drops both. | The order socket reconnects within 10 s of unblocking. Within 5 s of the reconnect the order book is reconciled: an order that changed while the socket was down (filled, cancelled) shows its final state in the order book, positions and any strategy or scalping leg that owns it, without a refresh. Health shows a gap for "order feed reconnected" open, then closed. |
| 3 | Sleep and wake | With a sandbox scalping stop active: close the lid or Apple menu > Sleep (macOS), Start > Power > Sleep (Windows), for 10 minutes. Once more across a scheduled time (for example the 15:15 MIS square-off) if the day allows it. | On wake, within 5 s: the log says "OpenAlgo was suspended for about 10 min", both broker sockets reconnect, the order book is read, and the sandbox catches up. A square-off whose time passed during the sleep runs late in the same session, once; if the session ended during the sleep it is reported, not run. The stop is still protected. No duplicate orders. |
| 4 | Clock change | macOS: System Settings > General > Date & Time, turn off "Set time and date automatically" and set the clock back 5 minutes; after 5 minutes set it forward 10 minutes; then turn automatic time back on. Windows: Settings > Time & language > Date & time, same steps. | Setting the clock back: nothing reconnects and nothing scheduled runs twice (a square-off or a strategy start already done today is not repeated). Setting it forward more than 30 s past the real time is handled as a wake (row 3): the sockets reconnect and the order book is read. Restoring automatic time causes no second action. |
| 5 | Disk full | Use the Windows laptop or a spare Mac account with nothing else running. Fill the system disk until about 200 MB is free (macOS `mkfile -n 50g ~/fill.bin` repeated with smaller sizes; Windows `fsutil file createnew C:\fill.bin <bytes>`), keep trading in analyzer mode for 10 minutes, then delete the file. Never on the maintainer's main working disk during live trading. | The app keeps running and does not crash. Writes that cannot be saved are counted and shown: Health shows "records not saved" (order log entries, monitoring records) with an alert; a scalping stop or strategy change that cannot be saved is refused with a trader-facing message, never silently lost. Orders placed through the API, the order pages and the charts still reach the broker (that path writes nothing to `openalgo.db` before sending); a strategy or scalping exit whose order row cannot be written is not sent and the trader is told, by design (CLAUDE.md, "An order path decides once"). After the file is deleted, saving resumes within a minute; on the next app restart every database opens without a recovery message. |
| 6 | Locked database | With the app running, in a terminal: `sqlite3 "<data folder>/openalgo.db"`, then `BEGIN EXCLUSIVE;`, wait 3 s, `ROLLBACK;`. Repeat holding it 10 s. Change nothing. (Data folder: README, "Where your data lives".) | Held 3 s: a stop or strategy save made meanwhile waits and is saved when the lock goes; live orders are accepted without waiting; nothing is shown to the trader. Held 10 s: a save made in the first 5 s of the lock is refused after 5 s (the busy budget, `db::sqlite::retry_busy`) with a trader-facing message, never lost silently; orders, quotes and the feed carry on; after the lock goes everything saves again. |

## Record

| Date | Machine | Build | Fault | Time | What happened | As expected? |
| --- | --- | --- | --- | --- | --- | --- |
| | | | | | | |

Endurance summary (from `sample_app.py --summary`):

| Date | Machine | Build | Hours | Memory warm-up -> peak | Descriptors warm-up -> peak | Data folder growth | Verdict |
| --- | --- | --- | --- | --- | --- | --- | --- |
| | | | | | | | |
