#!/usr/bin/env python3
"""Shared framework for the OpenAlgo Desktop broker QA audit runners.

Implements the harness rules from desktop-qa-audit-checklist.md section 1:
the six-value status model, per-call latency capture, the dynamically
resolved symbol matrix, the order ledger used for teardown, and the
colour-coded .xlsx report.

Ported from the web harness (openalgo/docs/audit/qa_audit/broker). What
differs on the desktop is where things live, not what is asserted:

  * The data folder is Tauri's app_data_dir (com.openalgo.desktop), not
    <repo>/db. openalgo.db holds symtoken, master_contract_status, settings
    and auth; sandbox.db holds the analyzer book; logs.db holds error_logs,
    the desktop's equivalent of log/errors.jsonl. Override the folder with
    OPENALGO_DESKTOP_DATA (a dev_server data folder, for instance).
  * Every database is opened READ-ONLY. The app is running and owns these
    files; the harness must never write to a trader's real data folder.
  * There is no plugin.json. The supported-exchange promise is the
    SUPPORTED_EXCHANGES const in src-tauri/src/brokers/<broker>/mod.rs.
  * The broker auth row is encrypted, so the active broker is read from
    settings.active_broker / symtoken_owner, and confirmed by /ping.

Not a test module. Lives outside the pytest tree deliberately.
"""

from __future__ import annotations

import csv
import json
import os
import re
import sqlite3
import sys
import time
from collections.abc import Callable, Iterable
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

IST = timezone(timedelta(hours=5, minutes=30))

# --------------------------------------------------------------------------
# desktop locations
# --------------------------------------------------------------------------

# docs/audit/qa_audit/broker/<this file> -> openalgo-desktop/
REPO_ROOT = Path(__file__).resolve().parents[4]
APP_IDENTIFIER = "com.openalgo.desktop"   # tauri.conf.json "identifier"

# The 503 body every /api/v1 route returns until the trader signs in to the
# app after a start or a logout (server/middleware.rs).
API_LOCKED_HINT = "sign in to openalgo to use the api"


def data_dir() -> Path:
    """Tauri app_data_dir() for this platform, or OPENALGO_DESKTOP_DATA."""
    env = os.getenv("OPENALGO_DESKTOP_DATA", "")
    if env:
        return Path(env)
    if os.name == "nt":
        return Path(os.getenv("APPDATA", str(Path.home()))) / APP_IDENTIFIER
    if sys.platform == "darwin":
        return Path.home() / "Library" / "Application Support" / APP_IDENTIFIER
    xdg = os.getenv("XDG_DATA_HOME", str(Path.home() / ".local" / "share"))
    return Path(xdg) / APP_IDENTIFIER


def connect_ro(name: str) -> sqlite3.Connection:
    """Open one of the app's SQLite stores read-only.

    mode=ro keeps the harness from ever writing to the trader's data while
    the app is running against it; WAL readers still see committed rows.
    """
    p = data_dir() / name
    need(p.is_file(), f"{name} not found at {p} - is OPENALGO_DESKTOP_DATA right?")
    return sqlite3.connect(f"file:{p.as_posix()}?mode=ro", uri=True, timeout=10)


def app_version() -> str:
    """CARGO_PKG_VERSION is what the running app reports; read it from Cargo.toml."""
    p = REPO_ROOT / "src-tauri" / "Cargo.toml"
    if p.is_file():
        m = re.search(r'^version\s*=\s*"([^"]+)"', p.read_text(encoding="utf-8"), re.M)
        if m:
            return m.group(1)
    return ""


def errlog_max_id() -> int:
    """Baseline for NG-09: the newest error_logs id before the run starts."""
    try:
        with connect_ro("logs.db") as c:
            row = c.execute("select max(id) from error_logs").fetchone()
        return int(row[0] or 0)
    except Exception:
        return 0


def errlog_since(start_id: int) -> list[list]:
    """error_logs rows written after start_id - the desktop's errors.jsonl."""
    with connect_ro("logs.db") as c:
        rows = c.execute(
            "select id, ts, level, module, message, coalesce(exception, '') "
            "from error_logs where id > ? order by id", (start_id,)).fetchall()
    return [[str(r[0]), f"{r[1]} {r[2]} {r[3] or ''}: {r[4]} {r[5]}"[:400]] for r in rows]


def sandbox_orderids() -> set[str]:
    """Every orderid in the analyzer book (sandbox.db) - SB-09 / DK isolation."""
    with connect_ro("sandbox.db") as c:
        return {str(r[0]) for r in c.execute("select orderid from sandbox_orders")}

# Status vocabulary - checklist section 16.2
PASS = "PASS"
FAIL = "FAIL"
WARN = "WARN"
SKIP = "SKIP"
BLOCKED = "BLOCKED"
ERROR = "ERROR"

_FILL = {
    PASS: "C6EFCE",
    FAIL: "FFC7CE",
    WARN: "FFEB9C",
    SKIP: "D9D9D9",
    BLOCKED: "A6A6A6",
    ERROR: "BDD7EE",
}


class Skip(Exception):
    """Raise inside a check to record SKIP with a reason."""


class Warn(Exception):
    """Raise inside a check to record WARN with a reason."""


@dataclass
class Result:
    id: str
    section: str
    mode: str
    endpoint: str = ""
    exchange: str = ""
    symbol: str = ""
    expected: str = ""
    actual: str = ""
    status: str = PASS
    latency_ms: float = 0.0
    http: str = ""
    message: str = ""


# --------------------------------------------------------------------------
# assertion helpers - depth beyond key presence (gap analysis defect A-07)
# --------------------------------------------------------------------------


def need(cond: bool, msg: str) -> None:
    if not cond:
        raise AssertionError(msg)


def need_keys(obj: dict, keys: Iterable[str], where: str) -> None:
    missing = [k for k in keys if k not in (obj or {})]
    need(not missing, f"{where}: missing {missing}")


def as_num(v: Any, where: str) -> float:
    """Documented numbers must parse. Rejects '1,234.50' and currency marks."""
    if isinstance(v, bool):
        raise AssertionError(f"{where}: bool where number expected")
    if isinstance(v, (int, float)):
        f = float(v)
    elif isinstance(v, str):
        need("," not in v, f"{where}: thousands separator in {v!r}")
        need(not re.search(r"[^\d.\-+eE]", v.strip()), f"{where}: non-numeric {v!r}")
        f = float(v)
    else:
        raise AssertionError(f"{where}: type {type(v).__name__}")
    need(f == f and abs(f) != float("inf"), f"{where}: NaN/Inf")
    return f


def need_nonzero(v: Any, where: str) -> float:
    f = as_num(v, where)
    need(f != 0, f"{where}: zero")
    return f


def need_2dp(v: Any, where: str) -> float:
    """UNI-07 - prices are rounded to 2 decimals in book/account responses."""
    f = as_num(v, where)
    s = f"{f!r}"
    if "." in s and "e" not in s.lower():
        dp = len(s.split(".")[1].rstrip("0"))
        need(dp <= 2, f"{where}: {dp} decimals in {v!r}")
    return f


def need_ohlc(d: dict, where: str) -> None:
    """Range-check prices against the day's band.

    A contract that has not traded today reports high and low as 0 while
    still carrying a real ltp from a previous session. Comparing against a
    [0, 0] band then fails a perfectly correct quote, so the range check is
    skipped when there is no band to check against - the individual values
    are still asserted numeric by the caller.
    """
    lo, hi = as_num(d["low"], f"{where}.low"), as_num(d["high"], f"{where}.high")
    need(lo <= hi, f"{where}: low {lo} > high {hi}")
    if lo == 0 and hi == 0:
        return
    for k in ("open", "ltp", "close"):
        if k in d and d[k] not in (None, 0):
            v = as_num(d[k], f"{where}.{k}")
            need(lo <= v <= hi, f"{where}: {k} {v} outside [{lo},{hi}]")


def parse_ist(ts: Any, where: str) -> datetime:
    """HS-07 / OB-03 - assert the IST wall-clock meaning, not mere presence."""
    if isinstance(ts, (int, float)):
        secs = float(ts) / 1000.0 if float(ts) > 1e11 else float(ts)
        return datetime.fromtimestamp(secs, IST)
    s = str(ts).strip()
    for fmt in (
        "%Y-%m-%d %H:%M:%S%z",
        "%Y-%m-%d %H:%M:%S",
        "%d-%b-%Y %H:%M:%S",
        "%d-%m-%Y %H:%M:%S",
        "%H:%M:%S",
        "%Y-%m-%dT%H:%M:%S%z",
    ):
        try:
            dt = datetime.strptime(s, fmt)
            return dt.astimezone(IST) if dt.tzinfo else dt.replace(tzinfo=IST)
        except ValueError:
            continue
    raise AssertionError(f"{where}: unparseable timestamp {ts!r}")


# Tick slabs - checklist section 2.1
def expected_equity_tick(price: float) -> float:
    for cap, tick in ((250, 0.01), (1000, 0.05), (5000, 0.10), (10000, 0.50), (20000, 1.00)):
        if price < cap:
            return tick
    return 5.00


def expected_index_fut_tick(level: float) -> float:
    return 0.05 if level < 15000 else (0.10 if level < 30000 else 0.20)


# --------------------------------------------------------------------------
# runner
# --------------------------------------------------------------------------


@dataclass
class Runner:
    client: Any
    mode: str
    host: str = "http://127.0.0.1:5000"
    db_path: str = ""
    results: list[Result] = field(default_factory=list)
    order_ledger: list[dict] = field(default_factory=list)
    gtt_ledger: list[str] = field(default_factory=list)
    limits: dict = field(default_factory=dict)
    quirks: list[dict] = field(default_factory=list)
    matrix: dict = field(default_factory=dict)
    env: dict = field(default_factory=dict)
    _section: str = ""
    _t0: float = 0.0

    # ---- lifecycle -------------------------------------------------------
    def section(self, name: str) -> Runner:
        self._section = name
        print(f"\n=== {name} [{self.mode}] ===")
        return self

    def check(self, cid: str, fn: Callable[[], Any], **meta) -> Any:
        """Run one case. Times it, classifies the outcome, records a row."""
        r = Result(id=cid, section=self._section, mode=self.mode, **meta)
        t = time.perf_counter()
        out = None
        try:
            out = fn()
            r.status = PASS
        except Skip as e:
            r.status, r.message = SKIP, str(e) or "no reason given"
            if not str(e):
                r.status, r.message = FAIL, "SKIP raised without a reason (section 16.2)"
        except Warn as e:
            r.status, r.message = WARN, str(e)
        except AssertionError as e:
            r.status, r.message = FAIL, str(e)
        except Exception as e:  # transport, 429, timeout -> harness fault
            r.status, r.message = ERROR, f"{type(e).__name__}: {e}"
        r.latency_ms = round((time.perf_counter() - t) * 1000, 1)
        self.results.append(r)
        mark = {PASS: "ok", FAIL: "FAIL", WARN: "warn", SKIP: "skip",
                BLOCKED: "blocked", ERROR: "err"}[r.status]
        line = f"  [{mark:>7}] {cid:<10} {r.latency_ms:>8.1f}ms  {meta.get('endpoint','')}"
        if r.message:
            line += f"  <- {r.message[:110]}"
        print(line)
        return out

    def record(self, cid: str, status: str, message: str, **meta) -> None:
        self.results.append(
            Result(id=cid, section=self._section, mode=self.mode,
                   status=status, message=message, **meta)
        )
        print(f"  [{status:>7}] {cid:<10}  {message[:110]}")

    def blocked(self, cid: str, why: str, **meta) -> None:
        self.record(cid, BLOCKED, why, **meta)

    def note_limit(self, k: str, v: Any) -> None:
        self.limits[k] = v

    def note_quirk(self, what: str, evidence: str) -> None:
        self.quirks.append({"what": what, "evidence": evidence})

    # ---- helpers ---------------------------------------------------------
    def ok(self, resp: Any, where: str) -> dict:
        """UNI-02/03 - status is exactly success, errors carry a message."""
        need(isinstance(resp, dict), f"{where}: {type(resp).__name__} not dict")
        st = resp.get("status")
        if API_LOCKED_HINT in str(resp.get("message", "")).lower():
            # Not a broker verdict: the app is locked until the trader signs in.
            raise RuntimeError("app is locked (503) - sign in to OpenAlgo Desktop and re-run")
        # "partial" is a documented third value, not a stray one: the
        # multi-item endpoints use it when some items succeeded and some
        # failed, and the websocket subscribe ack uses it for the same
        # reason. Rejecting it would fail a correct response.
        need(st in ("success", "error", "partial"), f"{where}: status={st!r}")
        if st == "error":
            msg = str(resp.get("message", ""))
            need(bool(msg), f"{where}: error without message")
            need("Traceback" not in msg, f"{where}: traceback leaked")
            raise AssertionError(f"{where}: {msg[:160]}")
        return resp

    def expect_error(self, resp: Any, where: str) -> str:
        """Negative cases - a clean error, never a 500 or a silent success."""
        need(isinstance(resp, dict), f"{where}: {type(resp).__name__} not dict")
        need(resp.get("status") == "error", f"{where}: expected error, got {resp.get('status')!r}")
        msg = str(resp.get("message", ""))
        need(bool(msg), f"{where}: error without message")
        need("Traceback" not in msg, f"{where}: traceback leaked")
        return msg

    def db(self) -> sqlite3.Connection:
        """openalgo.db (symtoken, master_contract_status, settings), read-only."""
        if self.db_path:
            need(os.path.isfile(self.db_path), f"symtoken db not found at {self.db_path}")
            return sqlite3.connect(f"file:{Path(self.db_path).as_posix()}?mode=ro", uri=True)
        return connect_ro("openalgo.db")

    def track_order(self, orderid: str, symbol: str, exchange: str, product: str) -> None:
        if orderid:
            self.order_ledger.append(
                {"orderid": str(orderid), "symbol": symbol,
                 "exchange": exchange, "product": product}
            )


# --------------------------------------------------------------------------
# symbol matrix - checklist section 1.3, replaces the hardcoded dict
# --------------------------------------------------------------------------


def resolve_matrix(run: Runner, exchanges: list[str]) -> dict:
    """Resolve every slot at run start. An unresolved slot is a finding."""
    m: dict[str, Any] = {"_unresolved": []}

    def try_slot(name: str, fn):
        try:
            v = fn()
            if v:
                m[name] = v
                return
        except Exception as e:
            m["_unresolved"].append(f"{name}: {type(e).__name__}: {e}")
            return
        m["_unresolved"].append(f"{name}: not found")

    def cheapest_equity(ex: str, candidates: list[str]) -> tuple[str, float] | None:
        best = None
        for s in candidates:
            try:
                d = run.client.quotes(symbol=s, exchange=ex).get("data") or {}
                ltp = float(d.get("ltp") or 0)
                if ltp > 0 and (best is None or ltp < best[1]):
                    best = (s, ltp)
            except Exception:
                continue
        return best

    if "NSE" in exchanges:
        try_slot("EQ_LIQUID", lambda: ("RELIANCE", "NSE"))
        cheap = cheapest_equity("NSE", ["IDEA", "YESBANK", "NHPC", "SUZLON", "PNB"])
        try_slot("EQ_CHEAP", lambda: (cheap[0], "NSE") if cheap else None)
        if cheap:
            m["EQ_CHEAP_LTP"] = cheap[1]
        try_slot("EQ_SPECIAL", lambda: ("BAJAJ-AUTO", "NSE"))
        # ETF slots removed: ETF tick size is deliberately not verified, and
        # no other check consumes them.
    if "BSE" in exchanges:
        # BSE is cash equity, not futures - without its own slot the
        # per-exchange streaming check silently skipped it entirely.
        def bse_equity():
            with run.db() as c:
                for cand in ("RELIANCE", "TCS", "SBIN", "INFY"):
                    if c.execute("select 1 from symtoken where symbol=? and exchange='BSE'",
                                 (cand,)).fetchone():
                        return (cand, "BSE")
            return None

        try_slot("EQ_BSE", bse_equity)
    if "GLOBAL_INDEX" in exchanges:
        def global_index():
            with run.db() as c:
                row = c.execute(
                    "select symbol from symtoken where exchange='GLOBAL_INDEX' "
                    "order by symbol limit 1").fetchone()
            return (row[0], "GLOBAL_INDEX") if row else None

        try_slot("IDX_GLOBAL", global_index)
    if "NSE_INDEX" in exchanges:
        try_slot("IDX_NSE", lambda: ("NIFTY", "NSE_INDEX"))
    if "BSE_INDEX" in exchanges:
        try_slot("IDX_BSE", lambda: ("SENSEX", "BSE_INDEX"))
    if "MCX_INDEX" in exchanges:
        try_slot("IDX_MCX", lambda: ("MCXBULLDEX", "MCX_INDEX"))

    # derivatives: nearest expiry per exchange, from /expiry
    und = {"NFO": "NIFTY", "BFO": "SENSEX", "CDS": "USDINR", "BCD": "USDINR",
           "MCX": "CRUDEOIL", "NCO": "GOLD"}
    for ex in ("NFO", "BFO", "CDS", "BCD", "MCX", "NCO"):
        if ex not in exchanges:
            continue
        u = und[ex]

        def fut(ex=ex, u=u):
            r = run.client.expiry(symbol=u, exchange=ex, instrumenttype="futures")
            ds = (r or {}).get("data") or []
            if not ds:
                return None
            # Skip a contract expiring today. On expiry day it has already
            # settled by the time most of a session has run, so quotes come
            # back all-zero except prev_close and history returns nothing -
            # which reads as a broker defect when it is just a dead contract.
            today = datetime.now(IST).date()
            exp = None
            for d in ds:
                try:
                    if datetime.strptime(d, "%d-%b-%y").date() > today:
                        exp = d
                        break
                except ValueError:
                    continue
            if exp is None:
                exp = ds[0]          # nothing later listed; fall back
            s = run.client.search(query=f"{u} FUT", exchange=ex).get("data") or []
            for row in s:
                if row.get("expiry") == exp and row.get("instrumenttype") == "FUT":
                    return (row["symbol"], ex)
            return None

        try_slot(f"FUT_{ex}", fut)

    # options via /optionsymbol - ATM for structure, OTM40 for order placement
    for idx_ex, opt_ex, u in (("NSE_INDEX", "NFO", "NIFTY"), ("BSE_INDEX", "BFO", "SENSEX")):
        if opt_ex not in exchanges or idx_ex not in exchanges:
            continue

        # expiry_date is documented Optional, but it is only optional when the
        # underlying itself carries an expiry. For a bare "NIFTY" the service
        # answers "Expiry date required", so resolve the nearest one first and
        # convert DD-MMM-YY to the DDMMMYY form the options services parse.
        exp = ""
        try:
            r = run.client.expiry(symbol=u, exchange=opt_ex, instrumenttype="options")
            ds = (r or {}).get("data") or []
            exp = ds[0].replace("-", "").upper() if ds else ""
        except Exception as e:
            m["_unresolved"].append(f"OPT_*_{opt_ex}: cannot resolve an expiry for {u}: {e}")
        if not exp:
            m["_unresolved"].append(
                f"OPT_*_{opt_ex}: no {u} option expiry from /expiry - option slots skipped")
            continue

        def osym(off, ot, u=u, idx_ex=idx_ex, opt_ex=opt_ex, exp=exp):
            r = run.client.optionsymbol(underlying=u, exchange=idx_ex, expiry_date=exp,
                                        offset=off, option_type=ot)
            if not isinstance(r, dict):
                raise RuntimeError(f"optionsymbol returned {type(r).__name__}")
            if not r.get("symbol"):
                # Surface the service's own reason instead of a bare "not found".
                raise RuntimeError(f"{u} {off} {ot} exp={exp}: "
                                   f"{str(r.get('message') or r)[:120]}")
            return (r["symbol"], r.get("exchange", opt_ex))

        try_slot(f"OPT_ATM_CE_{opt_ex}", lambda f=osym: f("ATM", "CE"))
        try_slot(f"OPT_ATM_PE_{opt_ex}", lambda f=osym: f("ATM", "PE"))
        # OPT_CHEAP = OTM40 - near-zero premium, safe at freeze-qty size
        try_slot(f"OPT_CHEAP_{opt_ex}", lambda f=osym: f("OTM40", "CE"))

    # decimal-strike option, straight from the master
    def decimal_strike():
        """A decimal strike that has actually traded today.

        Two earlier attempts at this were wrong in instructive ways. An
        unordered `limit 1` returned whatever row SQLite reached first, which
        was a far-dated contract. Ordering by expiry fixed the date but not
        the liquidity, and the tiebreak that preferred an index underlying was
        a no-op: index strikes are whole numbers at 50 and 100-point
        intervals, so decimal strikes exist only on single stocks.

        symtoken carries no OI or volume, so liquidity cannot be read from it.
        The only reliable signal is the quote itself, so probe a bounded
        number of nearest-expiry candidates and take the first with a live
        price. Falling back to the nearest expiry keeps the slot resolvable
        when nothing has traded - the caller records that it is untraded.
        """
        with run.db() as c:
            rows = c.execute(
                "select symbol, exchange, expiry, name from symtoken "
                "where instrumenttype in ('CE','PE') and strike != round(strike) "
                "and expiry != ''").fetchall()
        if not rows:
            return None
        months = {m: i for i, m in enumerate(
            ("JAN", "FEB", "MAR", "APR", "MAY", "JUN",
             "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"), 1)}

        def when(r):
            try:
                d, mo, y = r[2].split("-")
                return (int(y), months.get(mo.upper(), 13), int(d))
            except Exception:
                return (99, 99, 99)

        rows.sort(key=when)
        probe = int(os.getenv("QA_DECIMAL_PROBE", "12"))
        for sym, exch, _exp, _name in rows[:probe]:
            try:
                q = run.client.quotes(symbol=sym, exchange=exch)
            except Exception:
                continue
            if q.get("status") != "success":
                continue
            d = q.get("data") or {}
            try:
                if float(d.get("ltp") or 0) > 0:
                    return (sym, exch)
            except (TypeError, ValueError):
                continue
        return (rows[0][0], rows[0][1])

    try_slot("OPT_DECIMAL_STRIKE", decimal_strike)
    return m


def load_plugin_exchanges(broker: str) -> list[str]:
    """PRE-02 - supported_exchanges is the promise that drives coverage.

    The desktop has no plugin.json: each adapter declares
    `pub const SUPPORTED_EXCHANGES: &[Exchange] = &[Exchange::Nse, ...]` in
    src-tauri/src/brokers/<broker>/mod.rs. Variant names are CamelCase
    (NseIndex) and map to the wire codes (NSE_INDEX). QA_EXCHANGES
    (comma-separated) overrides it when the source tree is not at hand.
    """
    env = os.getenv("QA_EXCHANGES", "")
    if env:
        return [e.strip().upper() for e in env.split(",") if e.strip()]
    p = REPO_ROOT / "src-tauri" / "src" / "brokers" / broker / "mod.rs"
    if not broker or not p.is_file():
        return []
    m = re.search(r"SUPPORTED_EXCHANGES\s*:\s*&\[\s*Exchange\s*\]\s*=\s*&\[(.*?)\];",
                  p.read_text(encoding="utf-8"), re.S)
    if not m:
        return []
    names = re.findall(r"Exchange::(\w+)", m.group(1))
    return [re.sub(r"(?<!^)(?=[A-Z])", "_", n).upper() for n in names]


def active_broker(db_path: str = "") -> str:
    """PRE-01 - the broker the app is signed in to.

    auth.broker_id and the tokens are encrypted, so read the plain-text
    pointers instead: settings.active_broker, then the symtoken owner.
    """
    try:
        c = (sqlite3.connect(f"file:{Path(db_path).as_posix()}?mode=ro", uri=True)
             if db_path else connect_ro("openalgo.db"))
    except Exception:
        return ""
    with c:
        try:
            row = c.execute("select active_broker from settings limit 1").fetchone()
            if row and row[0]:
                return str(row[0])
        except sqlite3.Error:
            pass
        try:
            row = c.execute("select broker from symtoken_owner order by id desc limit 1").fetchone()
            if row and row[0]:
                return str(row[0])
        except sqlite3.Error:
            pass
    return ""


# Section headings in docs/prompt/symbol-format.md that own each index set.
_INDEX_SECTIONS = [
    ("NSE_INDEX", "### Common NSE Index Symbols", "### Common BSE Index Symbols"),
    ("BSE_INDEX", "### Common BSE Index Symbols", "### NCO Commodity Underlyings"),
    ("MCX_INDEX", "### MCX Index Symbols", "### Common Global Index Symbols"),
    ("GLOBAL_INDEX", "### Common Global Index Symbols", "### Exchange  Codes"),
]


def canonical_index_symbols() -> dict[str, list[str]]:
    """MC-07 / MC-08 - the canonical index sets, PER EXCHANGE.

    Parsed from docs/prompt/symbol-format.md rather than copied here, so the
    spec stays the single source of truth and the lists cannot drift.

    Keying per exchange is the point: BANKNIFTY belongs to NSE_INDEX and
    SENSEX to BSE_INDEX. Checking one flat list against both exchanges
    reports every NSE index as missing from BSE and vice versa - all of it
    noise, and it buries the genuine findings.
    """
    # The spec lives in the web repo; the desktop does not vendor it. Point
    # QA_SYMBOL_FORMAT at it, or keep the two repos side by side.
    p = Path(os.getenv("QA_SYMBOL_FORMAT", "") or
             REPO_ROOT.parent / "openalgo" / "docs" / "prompt" / "symbol-format.md")
    if not p.is_file():
        return {}
    txt = p.read_text(encoding="utf-8")
    out: dict[str, list[str]] = {}
    for exchange, start, stop in _INDEX_SECTIONS:
        try:
            seg = txt[txt.index(start): txt.index(stop, txt.index(start))]
        except ValueError:
            continue
        syms = []
        for line in seg.splitlines():
            t = line.strip().rstrip("\\").replace("\\&", "&")
            if not t or t.startswith(("#", "{%", "<", ">", "|", "*", "-")):
                continue
            if re.fullmatch(r"[A-Z0-9&.]{2,40}", t):
                syms.append(t)
        if syms:
            out[exchange] = sorted(set(syms))
    return out


def normalise(s: str) -> str:
    """Collapse a broker display name to comparable form: 'HANGSENG BEES-NAV'
    and 'BSE POWER & ENERGY' become HANGSENGBEESNAV / BSEPOWERENERGY."""
    return re.sub(r"[^A-Z0-9]", "", str(s).upper())


# --------------------------------------------------------------------------
# report - checklist section 16
# --------------------------------------------------------------------------

_COLS = ["ID", "Section", "Mode", "Endpoint", "Exchange", "Symbol",
         "Expected", "Actual", "Status", "Latency_ms", "HTTP", "Message"]


def _rows(run: Runner) -> list[list]:
    return [[r.id, r.section, r.mode, r.endpoint, r.exchange, r.symbol,
             r.expected, r.actual, r.status, r.latency_ms, r.http, r.message]
            for r in run.results]


def _latency_table(run: Runner) -> list[list]:
    import statistics as st
    buckets: dict[str, list[float]] = {}
    for r in run.results:
        if r.latency_ms and r.endpoint:
            buckets.setdefault(f"{r.endpoint} [{r.mode}]", []).append(r.latency_ms)
    out = [["Endpoint", "N", "Min", "Median", "P95", "Max"]]
    for k, v in sorted(buckets.items()):
        v.sort()
        p95 = v[min(len(v) - 1, int(len(v) * 0.95))]
        out.append([k, len(v), round(min(v), 1), round(st.median(v), 1),
                    round(p95, 1), round(max(v), 1)])
    return out


def write_report(run: Runner, out_path: Path, extra_sheets: dict | None = None) -> Path:
    counts: dict[str, int] = {}
    for r in run.results:
        counts[r.status] = counts.get(r.status, 0) + 1
    summary = [["Field", "Value"],
               ["App", "OpenAlgo Desktop"],
               ["App version", run.env.get("app_version", "")],
               ["Broker", run.env.get("broker", "")],
               ["Mode", run.mode],
               ["Run started (IST)", run.env.get("started", "")],
               ["Host", run.host],
               ["Data folder", str(data_dir())],
               ["Supported exchanges", ", ".join(run.env.get("exchanges", []))],
               ["Total checks", len(run.results)]]
    summary += [[s, counts.get(s, 0)] for s in (PASS, FAIL, WARN, SKIP, BLOCKED, ERROR)]
    summary += [["Verdict", "PASS" if counts.get(FAIL, 0) == 0 else "FAIL"],
                ["Note", "Sandbox results do not evidence broker correctness "
                         "(checklist SB-10)" if run.mode == "SANDBOX" else ""]]

    sheets: dict[str, list[list]] = {
        "Summary": summary,
        "Environment": [["Slot", "Value"]] +
                       [[k, str(v)] for k, v in sorted(run.matrix.items()) if k != "_unresolved"] +
                       [["UNRESOLVED", u] for u in run.matrix.get("_unresolved", [])],
        "Results": [_COLS] + _rows(run),
        "Latency": _latency_table(run),
        "Observed Limits": [["Limit", "Value"]] + [[k, str(v)] for k, v in run.limits.items()],
        "Quirks": [["What", "Evidence"]] + [[q["what"], q["evidence"]] for q in run.quirks],
    }
    for k, v in (extra_sheets or {}).items():
        sheets[k] = v

    try:
        from openpyxl import Workbook
        from openpyxl.styles import Font, PatternFill
        from openpyxl.utils import get_column_letter
    except ImportError:
        base = out_path.with_suffix("")
        for name, rows in sheets.items():
            p = Path(f"{base}_{name.replace(' ', '_')}.csv")
            with p.open("w", newline="", encoding="utf-8") as fh:
                csv.writer(fh).writerows(rows)
        print(f"\nopenpyxl not installed - wrote CSV sheets next to {base}")
        print("For the colour-coded workbook: uv pip install openpyxl")
        return base

    wb = Workbook()
    wb.remove(wb.active)
    status_col = _COLS.index("Status") + 1
    lat_col = _COLS.index("Latency_ms") + 1
    for name, rows in sheets.items():
        ws = wb.create_sheet(name[:31])
        for row in rows:
            ws.append(row)
        if rows:
            for c in ws[1]:
                c.font = Font(bold=True)
            ws.freeze_panes = "A2"
        widths: dict[int, int] = {}
        for row in rows:
            for i, v in enumerate(row, 1):
                widths[i] = min(60, max(widths.get(i, 10), len(str(v)) + 2))
        for i, w in widths.items():
            ws.column_dimensions[get_column_letter(i)].width = w
        if name == "Results":
            slow = float(os.getenv("QA_SLOW_MS", "3000"))
            for r in range(2, ws.max_row + 1):
                st = ws.cell(r, status_col).value
                if st in _FILL:
                    fill = PatternFill("solid", fgColor=_FILL[st])
                    for c in range(1, len(_COLS) + 1):
                        ws.cell(r, c).fill = fill
                lat = ws.cell(r, lat_col).value
                if isinstance(lat, (int, float)) and lat > slow:
                    ws.cell(r, lat_col).fill = PatternFill("solid", fgColor="FFEB9C")
    wb.save(out_path)
    return out_path


def print_rollup(run: Runner) -> bool:
    counts: dict[str, int] = {}
    for r in run.results:
        counts[r.status] = counts.get(r.status, 0) + 1
    print("\n" + "-" * 62)
    print(f"{run.mode} run: " + "  ".join(f"{s}={counts.get(s,0)}"
                                          for s in (PASS, FAIL, WARN, SKIP, BLOCKED, ERROR)))
    fails = [r for r in run.results if r.status == FAIL]
    if fails:
        print(f"\n{len(fails)} failure(s):")
        for r in fails:
            print(f"  {r.id:<10} {r.endpoint:<22} {r.message[:100]}")
    return counts.get(FAIL, 0) == 0


def now_ist() -> str:
    return datetime.now(IST).strftime("%Y-%m-%d %H:%M:%S %Z")
