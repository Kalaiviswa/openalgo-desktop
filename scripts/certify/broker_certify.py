#!/usr/bin/env python3
"""Live broker certification for OpenAlgo Desktop (BR-06).

Runs the certification steps against a RUNNING OpenAlgo Desktop app through
its public API (`/api/v1`) and its WebSocket feed, with the trader's own API
key, and writes a report: one row per step with PASS, FAIL or N/A, a note,
and the answer the app passed on from the broker, with secrets redacted.

The checklist that goes with it is docs/release/broker-certification.md.

Standard library only: nothing to install. Typical runs:

    # Everything except orders (safe at any time):
    python3 scripts/certify/broker_certify.py --broker zerodha

    # Full run in market hours, with one tiny far-from-market order and a GTT:
    python3 scripts/certify/broker_certify.py --broker zerodha --place-orders --gtt

    # Next morning after 03:00 IST, before signing in again:
    python3 scripts/certify/broker_certify.py --broker zerodha --boundary-check

    # After logging out of the broker in the app:
    python3 scripts/certify/broker_certify.py --broker zerodha --after-logout

The API key is read from the OPENALGO_CERT_API_KEY environment variable or
asked for at a prompt. `--api-key` works too, but other programs on the
computer can read a command line, so the script warns when it is used.

Orders are placed only with --place-orders, only in Live mode, only as a
LIMIT BUY priced below the market (never MARKET), and only after the
operator confirms at a prompt (or passes --yes). Every order or GTT the
script placed is cancelled before it exits, also after a failed step.
"""

import argparse
import base64
import datetime
import getpass
import hashlib
import json
import os
import re
import socket
import struct
import sys
import threading
import time
import urllib.error
import urllib.request
from decimal import ROUND_FLOOR, Decimal, InvalidOperation

VERSION = "1.0"
PASS, FAIL, NA = "PASS", "FAIL", "N/A"
STRATEGY = "Certification"
IST = datetime.timezone(datetime.timedelta(hours=5, minutes=30))
# Brokers that trade around the clock: their session has no 03:00 IST end.
CRYPTO_BROKERS = {"deltaexchange"}
TERMINAL_STATUSES = {"cancelled", "rejected", "complete"}


# --------------------------------------------------------------- redaction


class Redactor:
    """Removes secrets and account identity from everything the report keeps.

    Values under identity or secret field names are replaced, and every such
    value is also removed wherever it appears inside other text (a broker
    message that repeats the client id). Patterns catch what has no field
    name: JWTs, e-mail addresses, Indian mobile numbers and PANs.
    """

    SECRET_FIELDS = {
        "apikey", "apisecret", "secret", "password", "pin", "totp", "otp",
        "accesstoken", "authtoken", "feedtoken", "refreshtoken", "sessiontoken",
        "jwt", "jwttoken", "susertoken", "jkey", "authorization",
        "userid", "username", "clientid", "clientcode", "client", "accountid",
        "account", "accountno", "accountnumber", "actid", "uid", "ucc",
        "email", "emailid", "mobile", "mobileno", "phone", "phonenumber",
        "pan", "panno", "pannumber", "dpid", "boid", "demat", "bankaccount",
    }
    PATTERNS = [
        re.compile(r"eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]*"),
        re.compile(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}"),
        re.compile(r"(?<!\d)[6-9]\d{9}(?!\d)"),
        re.compile(r"\b[A-Z]{5}[0-9]{4}[A-Z]\b"),
    ]
    MASK = "<redacted>"

    def __init__(self, api_key):
        self.known = {api_key} if api_key else set()

    @staticmethod
    def _field(name):
        return re.sub(r"[^a-z0-9]", "", str(name).lower())

    def learn(self, obj):
        """Remember every value held under a secret field name."""
        if isinstance(obj, dict):
            for k, v in obj.items():
                if self._field(k) in self.SECRET_FIELDS and isinstance(v, (str, int)):
                    text = str(v).strip()
                    if len(text) >= 3:
                        self.known.add(text)
                self.learn(v)
        elif isinstance(obj, list):
            for v in obj:
                self.learn(v)

    def text(self, s):
        for value in sorted(self.known, key=len, reverse=True):
            s = s.replace(value, self.MASK)
        for pattern in self.PATTERNS:
            s = pattern.sub(self.MASK, s)
        return s

    def scrub(self, obj):
        if isinstance(obj, dict):
            return {
                k: self.MASK if self._field(k) in self.SECRET_FIELDS else self.scrub(v)
                for k, v in obj.items()
            }
        if isinstance(obj, list):
            return [self.scrub(v) for v in obj]
        if isinstance(obj, str):
            return self.text(obj)
        return obj


# ------------------------------------------------------------------ report


class Report:
    """The steps in the order they ran. Secrets are removed when written."""

    def __init__(self, meta, redactor):
        self.meta = meta
        self.redactor = redactor
        self.steps = []

    def record(self, step, result, note="", raw=None, request=None):
        entry = {"step": step, "result": result, "note": note, "raw": raw}
        if request is not None:
            entry["request"] = request
        self.steps.append(entry)
        self.redactor.learn(raw)
        print("{:<24} {:<5} {}".format(step, result, self.redactor.text(note)), flush=True)
        return result == PASS

    def failed(self):
        return any(s["result"] == FAIL for s in self.steps)

    def write(self, base):
        self.redactor.learn(self.steps)
        doc = self.redactor.scrub({"meta": self.meta, "steps": self.steps})
        with open(base + ".json", "w", encoding="utf-8") as f:
            json.dump(doc, f, indent=2, ensure_ascii=False)
        with open(base + ".md", "w", encoding="utf-8") as f:
            f.write(markdown(doc))
        return base + ".json", base + ".md"


def excerpt(raw, limit=160):
    text = raw if isinstance(raw, str) else json.dumps(raw, ensure_ascii=False)
    text = text.replace("|", "\\|").replace("\n", " ")
    return text if len(text) <= limit else text[: limit - 3] + "..."


def markdown(doc):
    meta = doc["meta"]
    lines = [
        "# Broker certification: {}".format(meta["broker"]),
        "",
        "| Item | Value |",
        "| --- | --- |",
    ]
    for key in ("started_ist", "app", "feed", "mode", "script_version", "options"):
        lines.append("| {} | {} |".format(key, excerpt(meta.get(key, ""), 200)))
    counts = {r: sum(1 for s in doc["steps"] if s["result"] == r) for r in (PASS, FAIL, NA)}
    lines += [
        "",
        "Result: {} PASS, {} FAIL, {} N/A.".format(counts[PASS], counts[FAIL], counts[NA]),
        "",
        "| Step | Result | Note | Answer (excerpt) |",
        "| --- | --- | --- | --- |",
    ]
    for s in doc["steps"]:
        raw = "" if s["raw"] is None else excerpt(s["raw"])
        lines.append("| {} | {} | {} | {} |".format(s["step"], s["result"], excerpt(s["note"], 300), raw))
    return "\n".join(lines) + "\n"


# -------------------------------------------------------------------- HTTP


class Api:
    """POSTs to the app's /api/v1 with the API key in the body."""

    def __init__(self, host, port, api_key, timeout=30):
        self.base = "http://{}:{}".format(host, port)
        self.api_key = api_key
        self.timeout = timeout
        self.calls = []

    def post(self, path, body=None):
        """(HTTP status, parsed JSON or the body text). Status 0: no answer."""
        payload = dict(body or {})
        self.calls.append((path, dict(payload)))
        payload["apikey"] = self.api_key
        req = urllib.request.Request(
            self.base + path,
            data=json.dumps(payload).encode(),
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as resp:
                return resp.status, parse(resp.read())
        except urllib.error.HTTPError as e:
            return e.code, parse(e.read())
        except (urllib.error.URLError, OSError) as e:
            return 0, "No answer from the app at {}: {}".format(self.base, e)


def parse(body):
    text = body.decode("utf-8", "replace")
    try:
        return json.loads(text)
    except ValueError:
        return text


def ok(status, answer):
    return status == 200 and isinstance(answer, dict) and answer.get("status") == "success"


def message_of(answer):
    if isinstance(answer, dict):
        return str(answer.get("message") or answer.get("error") or "")
    return str(answer)[:200]


# --------------------------------------------------------------- WebSocket


class WsClosed(Exception):
    pass


class WsClient:
    """A small RFC 6455 client: text frames, masking, ping/pong, close."""

    GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

    def __init__(self, sock):
        self.sock = sock
        self.buf = b""
        self.parts = []
        self.send_lock = threading.Lock()

    @classmethod
    def connect(cls, host, port, timeout=10):
        sock = socket.create_connection((host, port), timeout=timeout)
        key = base64.b64encode(os.urandom(16)).decode()
        request = (
            "GET / HTTP/1.1\r\nHost: {}:{}\r\nUpgrade: websocket\r\n"
            "Connection: Upgrade\r\nSec-WebSocket-Key: {}\r\n"
            "Sec-WebSocket-Version: 13\r\n\r\n"
        ).format(host, port, key)
        sock.sendall(request.encode())
        client = cls(sock)
        head = client._read_head(time.monotonic() + timeout)
        expected = base64.b64encode(hashlib.sha1((key + cls.GUID).encode()).digest()).decode()
        if " 101 " not in head.split("\r\n", 1)[0] or expected not in head:
            sock.close()
            raise WsClosed("the feed did not accept the WebSocket connection")
        return client

    def _read_head(self, deadline):
        while b"\r\n\r\n" not in self.buf:
            self._fill(deadline)
        head, self.buf = self.buf.split(b"\r\n\r\n", 1)
        return head.decode("latin-1")

    def _fill(self, deadline):
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError()
        self.sock.settimeout(remaining)
        try:
            data = self.sock.recv(65536)
        except socket.timeout:
            raise TimeoutError()
        if not data:
            raise WsClosed("the feed closed the connection")
        self.buf += data

    def _frame(self):
        """One complete frame from the buffer, or None (data kept)."""
        if len(self.buf) < 2:
            return None
        b0, b1 = self.buf[0], self.buf[1]
        length, pos = b1 & 0x7F, 2
        if length == 126:
            if len(self.buf) < 4:
                return None
            length, pos = struct.unpack("!H", self.buf[2:4])[0], 4
        elif length == 127:
            if len(self.buf) < 10:
                return None
            length, pos = struct.unpack("!Q", self.buf[2:10])[0], 10
        mask = b""
        if b1 & 0x80:
            mask, pos = self.buf[pos:pos + 4], pos + 4
        if len(self.buf) < pos + length:
            return None
        payload = self.buf[pos:pos + length]
        self.buf = self.buf[pos + length:]
        if mask:
            payload = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
        return b0 & 0x80, b0 & 0x0F, payload

    def _send(self, opcode, payload):
        n = len(payload)
        if n < 126:
            header = struct.pack("!BB", 0x80 | opcode, 0x80 | n)
        elif n < 65536:
            header = struct.pack("!BBH", 0x80 | opcode, 0x80 | 126, n)
        else:
            header = struct.pack("!BBQ", 0x80 | opcode, 0x80 | 127, n)
        mask = os.urandom(4)
        body = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
        with self.send_lock:
            self.sock.sendall(header + mask + body)

    def send_json(self, obj):
        self._send(0x1, json.dumps(obj).encode())

    def recv_json(self, timeout):
        """The next JSON message, or None when none arrives in `timeout`."""
        deadline = time.monotonic() + timeout
        while True:
            frame = self._frame()
            if frame is None:
                try:
                    self._fill(deadline)
                except TimeoutError:
                    return None
                continue
            fin, opcode, payload = frame
            if opcode == 0x9:
                self._send(0xA, payload)
            elif opcode == 0x8:
                raise WsClosed("the feed closed the connection")
            elif opcode in (0x0, 0x1):
                self.parts.append(payload)
                if fin:
                    text, self.parts = b"".join(self.parts), []
                    try:
                        return json.loads(text.decode("utf-8", "replace"))
                    except ValueError:
                        continue

    def close(self):
        try:
            self._send(0x8, struct.pack("!H", 1000))
        except OSError:
            pass
        self.sock.close()


def ws_authenticate(ws, api_key, wait):
    """The auth answer (or an error frame, or None)."""
    ws.send_json({"action": "authenticate", "api_key": api_key})
    deadline = time.monotonic() + wait
    while time.monotonic() < deadline:
        msg = ws.recv_json(deadline - time.monotonic())
        if msg is None:
            return None
        if msg.get("type") == "auth" or msg.get("status") == "error":
            return msg
    return None


class OrderStream:
    """Keeps the 8765 `subscribe_orders` stream open and collects updates."""

    def __init__(self, ws):
        self.ws = ws
        self.updates = []
        self.lock = threading.Lock()
        self.stop = threading.Event()
        self.thread = threading.Thread(target=self._read, daemon=True)

    def start(self):
        self.thread.start()

    def _read(self):
        while not self.stop.is_set():
            try:
                msg = self.ws.recv_json(0.5)
            except (WsClosed, OSError):
                return
            if isinstance(msg, dict) and msg.get("type") == "order_update":
                with self.lock:
                    self.updates.append(msg)

    def for_order(self, orderid):
        with self.lock:
            return [u for u in self.updates if str(u.get("orderid")) == str(orderid)]

    def close(self):
        self.stop.set()
        self.thread.join(timeout=2)
        self.ws.close()


# ------------------------------------------------------------------ prices


def to_decimal(value):
    try:
        d = Decimal(str(value))
    except (InvalidOperation, ValueError):
        return None
    return d if d.is_finite() and d > 0 else None


def below_market(ltp, tick, percent):
    """A price `percent` below `ltp`, rounded down to the tick; None if not positive."""
    ltp, tick = to_decimal(ltp), to_decimal(tick)
    if ltp is None or tick is None:
        return None
    raw = ltp * (Decimal(100) - Decimal(str(percent))) / Decimal(100)
    price = (raw / tick).to_integral_value(rounding=ROUND_FLOOR) * tick
    return price.quantize(tick) if price > 0 else None


# ---------------------------------------------------------------- the steps


class Certifier:
    def __init__(self, args, api, report):
        self.a = args
        self.api = api
        self.report = report
        self.live = None  # unknown until the mode step reads it
        self.ltp = None
        self.instrument = None
        self.placed_any = False
        self.open_orders = set()
        self.open_gtts = set()

    # -- helpers

    def call(self, step, path, body, check, note_ok=""):
        """POST, then record PASS when `check(answer)` holds."""
        status, answer = self.api.post(path, body)
        if ok(status, answer) and check(answer):
            self.report.record(step, PASS, note_ok, answer, body)
            return answer
        note = "HTTP {}: {}".format(status, message_of(answer) or "unexpected answer")
        self.report.record(step, FAIL, note, answer, body)
        return None

    def na(self, steps, reason):
        for step in steps:
            self.report.record(step, NA, reason)

    # -- sign-in, mode, master

    def sign_in(self):
        status, answer = self.api.post("/api/v1/ping")
        data = answer.get("data", {}) if isinstance(answer, dict) else {}
        if ok(status, answer) and data.get("broker") == self.a.broker:
            return self.report.record("sign_in", PASS, "signed in to " + self.a.broker, answer)
        if ok(status, answer):
            note = "the app is signed in to {}, not {}".format(data.get("broker"), self.a.broker)
        else:
            note = "HTTP {}: {} (sign in to the broker in the app first)".format(
                status, message_of(answer))
        return self.report.record("sign_in", FAIL, note, answer)

    def mode(self):
        status, answer = self.api.post("/api/v1/analyzer")
        data = answer.get("data") if ok(status, answer) else None
        if not isinstance(data, dict) or not isinstance(data.get("analyze_mode"), bool):
            note = "HTTP {}: {}".format(status, message_of(answer) or "unexpected answer")
            return self.report.record("mode", FAIL, note, answer)
        self.live = not data["analyze_mode"]
        self.report.meta["mode"] = "live" if self.live else "analyze"
        note = "Live mode" if self.live else "analyzer mode is on: orders go to the sandbox, not the broker"
        return self.report.record("mode", PASS, note, answer)

    def master(self):
        sym, ex = self.a.symbol, self.a.exchange
        self.call("master_search", "/api/v1/search", {"query": sym, "exchange": ex},
                  lambda v: any(r.get("symbol") == sym for r in v.get("data") or []))
        answer = self.call("master_symbol", "/api/v1/symbol", {"symbol": sym, "exchange": ex},
                           lambda v: instrument_ok(v.get("data"), sym))
        self.instrument = answer["data"] if answer else None
        self.master_fo()

    def master_fo(self):
        if not self.a.fo_symbol:
            return self.na(["master_fo_symbol", "master_expiry"], "no --fo-symbol given")
        answer = self.call("master_fo_symbol", "/api/v1/symbol",
                           {"symbol": self.a.fo_symbol, "exchange": self.a.fo_exchange},
                           lambda v: instrument_ok(v.get("data"), self.a.fo_symbol)
                           and bool(v["data"].get("expiry")))
        if answer is None:
            return self.na(["master_expiry"], "the F&O contract was not found")
        data = answer["data"]
        kind = "futures" if str(data.get("instrumenttype", "")).upper().startswith("FUT") else "options"
        expiry = str(data.get("expiry", "")).upper()
        self.call("master_expiry", "/api/v1/expiry",
                  {"symbol": data.get("name"), "exchange": self.a.fo_exchange, "instrumenttype": kind},
                  lambda v: expiry in [str(e).upper() for e in v.get("data") or []],
                  "the contract's expiry is listed")

    # -- books, funds, market data

    def books(self):
        self.call("orderbook", "/api/v1/orderbook", {},
                  lambda v: isinstance((v.get("data") or {}).get("orders"), list))
        self.call("tradebook", "/api/v1/tradebook", {}, lambda v: isinstance(v.get("data"), list))
        self.call("positionbook", "/api/v1/positionbook", {}, lambda v: isinstance(v.get("data"), list))
        self.call("holdings", "/api/v1/holdings", {},
                  lambda v: isinstance((v.get("data") or {}).get("holdings"), list))
        self.call("funds", "/api/v1/funds", {},
                  lambda v: "availablecash" in (v.get("data") or {}))

    def market_data(self):
        pair = {"symbol": self.a.symbol, "exchange": self.a.exchange}
        answer = self.call("quotes", "/api/v1/quotes", pair,
                           lambda v: to_decimal((v.get("data") or {}).get("ltp")) is not None)
        self.ltp = to_decimal(answer["data"]["ltp"]) if answer else None
        legs = [pair] + ([{"symbol": self.a.fo_symbol, "exchange": self.a.fo_exchange}]
                         if self.a.fo_symbol else [])
        self.call("multiquotes", "/api/v1/multiquotes", {"symbols": legs},
                  lambda v: len(v.get("results") or []) == len(legs)
                  and all(isinstance(r.get("data"), dict) for r in v["results"]))
        self.call("depth", "/api/v1/depth", pair,
                  lambda v: isinstance((v.get("data") or {}).get("bids"), list)
                  and isinstance(v["data"].get("asks"), list))

    def history(self):
        answer = self.call("intervals", "/api/v1/intervals", {},
                           lambda v: any((v.get("data") or {}).get(k) for k in ("days", "minutes", "hours")))
        data = answer["data"] if answer else {}
        today = datetime.datetime.now(IST).date()
        runs = [("history_daily", pick(data.get("days"), "D"), 10),
                ("history_intraday", pick(data.get("minutes"), "5m") or pick(data.get("hours"), "1h"), 5)]
        for step, interval, days in runs:
            if not interval:
                self.na([step], "the broker lists no such interval")
                continue
            body = {"symbol": self.a.symbol, "exchange": self.a.exchange, "interval": interval,
                    "start_date": str(today - datetime.timedelta(days=days)), "end_date": str(today)}
            self.call(step, "/api/v1/history", body,
                      lambda v: candles_ok(v.get("data")), "interval " + interval)

    # -- the market feed

    def feed(self):
        steps = ["feed_ltp", "feed_quote", "feed_depth"]
        try:
            ws = WsClient.connect(self.a.host, self.a.ws_port)
        except (OSError, WsClosed, TimeoutError) as e:
            return self.fail_all(steps, "could not connect to the feed: {}".format(e))
        try:
            auth = ws_authenticate(ws, self.api.api_key, self.a.feed_wait)
            if not auth or auth.get("status") != "success":
                return self.fail_all(steps, "the feed refused the API key", auth)
            for step, mode in zip(steps, (1, 2, 3)):
                self.feed_mode(ws, step, mode)
        except (OSError, WsClosed) as e:
            self.report.record("feed", FAIL, "the feed connection broke: {}".format(e))
        finally:
            ws.close()

    def feed_mode(self, ws, step, mode):
        instrument = {"symbol": self.a.symbol, "exchange": self.a.exchange}
        request = {"action": "subscribe", "symbols": [instrument], "mode": mode}
        if mode == 3:
            request["depth"] = 5
        ws.send_json(request)
        ack, frame = None, None
        deadline = time.monotonic() + self.a.feed_wait
        while frame is None and time.monotonic() < deadline:
            msg = ws.recv_json(deadline - time.monotonic())
            if not isinstance(msg, dict):
                break
            if msg.get("type") == "subscribe" or (msg.get("status") == "error" and ack is None):
                ack = msg
            elif msg.get("type") == "market_data" and msg.get("mode") == mode \
                    and msg.get("symbol") == self.a.symbol:
                frame = msg
        ws.send_json({"action": "unsubscribe", "symbols": [instrument], "mode": mode})
        self.record_feed(step, ack, frame)

    def record_feed(self, step, ack, frame):
        raw = {"ack": ack, "first_frame": frame}
        text = json.dumps(ack or {})
        if "no live market data feed" in text:
            return self.report.record(step, NA, "the broker has no live feed in OpenAlgo Desktop", raw)
        if frame is not None:
            return self.report.record(step, PASS, "first frame received", raw)
        if self.a.market_closed:
            return self.report.record(step, NA, "no frame; the operator says the market is closed", raw)
        return self.report.record(step, FAIL, "no frame within {} s".format(self.a.feed_wait), raw)

    def fail_all(self, steps, note, raw=None):
        for step in steps:
            self.report.record(step, FAIL, note, raw)

    # -- orders

    ORDER_STEPS = ["order_stream_subscribe", "order_place", "order_modify", "order_cancel",
                   "order_stream_updates"]
    GTT_STEPS = ["gtt_place", "gtt_modify", "gtt_cancel"]

    def order_gate(self):
        """(result, why) when no order may be placed, else None."""
        if not self.a.place_orders:
            return NA, "not run: pass --place-orders to place and cancel one test order"
        if self.live is None:
            return FAIL, "not placed: the app's mode (Live or analyzer) could not be read"
        if not self.live:
            return NA, "analyzer mode is on: orders go to the sandbox, not the broker"
        if self.ltp is None or not self.instrument:
            return FAIL, "not placed: the last price or the instrument could not be read"
        if self.order_price(self.a.offset) is None:
            return FAIL, "not placed: no valid price below the market"
        if not self.confirmed():
            return NA, "not placed: the operator did not confirm"
        return None

    def confirmed(self):
        if self.a.yes:
            return True
        if not sys.stdin.isatty():
            return False
        print("This places a real BUY LIMIT order for {} {} on {} at {} (product {}), "
              "then modifies and cancels it.{}".format(
                  self.quantity(), self.a.symbol, self.a.exchange,
                  self.order_price(self.a.offset), self.product(),
                  " It also places, modifies and cancels one GTT." if self.a.gtt else ""))
        return input("Type YES to continue: ").strip() == "YES"

    def quantity(self):
        """One lot (1 for an equity), as a JSON number."""
        lot = to_decimal((self.instrument or {}).get("lotsize"))
        if lot is None:
            return 1
        return int(lot) if lot == lot.to_integral_value() else float(lot)

    def product(self):
        if self.a.product:
            return self.a.product
        return "CNC" if self.a.exchange in ("NSE", "BSE") else "NRML"

    def order_price(self, percent):
        return below_market(self.ltp, (self.instrument or {}).get("tick_size"), percent)

    def orders(self, gate):
        if gate:
            return self.record_all(self.ORDER_STEPS, *gate)
        stream = self.open_order_stream()
        try:
            orderid = self.place()
            if orderid:
                self.modify(orderid)
                self.cancel(orderid)
            self.check_stream(stream, orderid)
        finally:
            if stream:
                stream.close()

    def record_all(self, steps, result, note):
        for step in steps:
            self.report.record(step, result, note)

    def open_order_stream(self):
        try:
            ws = WsClient.connect(self.a.host, self.a.ws_port)
            auth = ws_authenticate(ws, self.api.api_key, self.a.feed_wait)
            if not auth or auth.get("status") != "success":
                ws.close()
                self.report.record("order_stream_subscribe", FAIL, "the feed refused the API key", auth)
                return None
            ws.send_json({"action": "subscribe_orders", "request_id": "certify"})
            ack = ws.recv_json(self.a.feed_wait)
        except (OSError, WsClosed, TimeoutError) as e:
            self.report.record("order_stream_subscribe", FAIL, "could not connect: {}".format(e))
            return None
        if not isinstance(ack, dict) or ack.get("status") != "success":
            self.report.record("order_stream_subscribe", FAIL, "no subscribe acknowledgement", ack)
            ws.close()
            return None
        self.report.record("order_stream_subscribe", PASS, "subscribed", ack)
        stream = OrderStream(ws)
        stream.start()
        return stream

    def order_body(self, price):
        return {"strategy": STRATEGY, "symbol": self.a.symbol, "exchange": self.a.exchange,
                "action": "BUY", "pricetype": "LIMIT", "product": self.product(),
                "quantity": self.quantity(), "price": float(price), "trigger_price": 0,
                "disclosed_quantity": 0}

    def place(self):
        body = self.order_body(self.order_price(self.a.offset))
        status, answer = self.api.post("/api/v1/placeorder", body)
        self.placed_any = True
        orderid = answer.get("orderid") if isinstance(answer, dict) else None
        if not ok(status, answer) or not orderid:
            self.report.record("order_place", FAIL, "HTTP {}: {}".format(status, message_of(answer)),
                               answer, body)
            return None
        self.open_orders.add(str(orderid))
        seen = self.wait_status(orderid, lambda d: d.get("order_status") is not None)
        state = (seen or {}).get("order_status")
        if state in ("open", "trigger pending"):
            self.report.record("order_place", PASS, "order {} is open".format(orderid),
                               {"placed": answer, "status": seen}, body)
            return str(orderid)
        if state == "complete":
            self.open_orders.discard(str(orderid))
            note = "the order FILLED: you now hold the position; close it in the app"
        else:
            note = "order {} is {}".format(orderid, state or "not in the order book")
        self.report.record("order_place", FAIL, note, {"placed": answer, "status": seen}, body)
        return None

    def modify(self, orderid):
        price = self.order_price(self.a.offset + 1)
        body = dict(self.order_body(price), orderid=orderid)
        status, answer = self.api.post("/api/v1/modifyorder", body)
        if not ok(status, answer):
            return self.report.record("order_modify", FAIL,
                                      "HTTP {}: {}".format(status, message_of(answer)), answer, body)
        seen = self.wait_status(orderid, lambda d: to_decimal(d.get("price")) == price)
        if seen:
            return self.report.record("order_modify", PASS, "price moved to {}".format(price),
                                      {"modified": answer, "status": seen}, body)
        return self.report.record("order_modify", FAIL, "the order book never showed {}".format(price),
                                  {"modified": answer}, body)

    def cancel(self, orderid, step="order_cancel"):
        body = {"strategy": STRATEGY, "orderid": orderid}
        status, answer = self.api.post("/api/v1/cancelorder", body)
        if not ok(status, answer):
            return self.report.record(step, FAIL, "HTTP {}: {}".format(status, message_of(answer)),
                                      answer, body)
        seen = self.wait_status(orderid, lambda d: d.get("order_status") == "cancelled")
        if seen:
            self.open_orders.discard(str(orderid))
            return self.report.record(step, PASS, "order {} cancelled".format(orderid),
                                      {"cancelled": answer, "status": seen}, body)
        return self.report.record(step, FAIL, "the order book never showed it cancelled",
                                  {"cancelled": answer}, body)

    def wait_status(self, orderid, done):
        """The order's status once `done(data)` holds, else None after --wait."""
        deadline = time.monotonic() + self.a.wait
        while True:
            status, answer = self.api.post("/api/v1/orderstatus",
                                           {"orderid": orderid, "strategy": STRATEGY})
            data = answer.get("data") if ok(status, answer) else None
            if isinstance(data, dict):
                if str(data.get("order_status")) in TERMINAL_STATUSES - {"cancelled"}:
                    self.open_orders.discard(str(orderid))
                if done(data):
                    return data
            if time.monotonic() >= deadline:
                return None
            time.sleep(1)

    def check_stream(self, stream, orderid):
        step = "order_stream_updates"
        if stream is None:
            return self.report.record(step, FAIL, "the order stream did not start")
        if not orderid:
            return self.report.record(step, NA, "no order was placed")
        deadline = time.monotonic() + self.a.stream_wait
        while time.monotonic() < deadline:
            if any(u.get("order_status") == "cancelled" for u in stream.for_order(orderid)):
                break
            time.sleep(0.5)
        updates = stream.for_order(orderid)
        statuses = [u.get("order_status") for u in updates]
        if "cancelled" in statuses:
            return self.report.record(step, PASS, "updates seen: " + ", ".join(map(str, statuses)), updates)
        note = "updates seen: {} (expected one ending in cancelled)".format(statuses or "none")
        return self.report.record(step, FAIL, note, updates)

    # -- GTT

    def gtt(self, gate):
        if not self.a.gtt:
            return self.record_all(self.GTT_STEPS, NA, "not run: pass --gtt with --place-orders")
        if gate:
            return self.record_all(self.GTT_STEPS, *gate)
        trigger_id = self.gtt_place()
        if trigger_id:
            self.gtt_modify(trigger_id)
            self.gtt_cancel(trigger_id)

    def gtt_body(self, percent):
        trigger = self.order_price(percent)
        tick = to_decimal(self.instrument.get("tick_size"))
        price = trigger - tick if trigger - tick > 0 else trigger
        return {"strategy": STRATEGY, "trigger_type": "SINGLE", "exchange": self.a.exchange,
                "symbol": self.a.symbol, "action": "BUY",
                "product": "CNC" if self.a.exchange in ("NSE", "BSE") else "NRML",
                "quantity": self.quantity(), "pricetype": "LIMIT", "price": float(price),
                "triggerprice_sl": float(trigger), "triggerprice_tg": 0,
                "stoploss": None, "target": None}

    def gtt_place(self):
        body = self.gtt_body(self.a.offset)
        status, answer = self.api.post("/api/v1/placegttorder", body)
        self.placed_any = True
        if "not supported" in message_of(answer):
            self.report.record("gtt_place", NA, message_of(answer), answer, body)
            self.na(["gtt_modify", "gtt_cancel"], "the broker has no GTT in OpenAlgo Desktop")
            return None
        trigger_id = answer.get("trigger_id") if isinstance(answer, dict) else None
        if not ok(status, answer) or not trigger_id:
            self.report.record("gtt_place", FAIL, "HTTP {}: {}".format(status, message_of(answer)),
                               answer, body)
            return None
        self.open_gtts.add(str(trigger_id))
        row = self.find_gtt(trigger_id)
        result = PASS if row else FAIL
        note = "trigger {} is active".format(trigger_id) if row else "not in the GTT book"
        self.report.record("gtt_place", result, note, {"placed": answer, "book_row": row}, body)
        return str(trigger_id)

    def gtt_modify(self, trigger_id):
        body = dict(self.gtt_body(self.a.offset + 1), trigger_id=trigger_id)
        status, answer = self.api.post("/api/v1/modifygttorder", body)
        if not ok(status, answer):
            return self.report.record("gtt_modify", FAIL,
                                      "HTTP {}: {}".format(status, message_of(answer)), answer, body)
        wanted = self.order_price(self.a.offset + 1)
        row = self.find_gtt(trigger_id)
        prices = [to_decimal(p) for p in (row or {}).get("trigger_prices") or []]
        result = PASS if wanted in prices else FAIL
        note = "trigger moved to {}".format(wanted) if result == PASS else "the GTT book does not show it"
        return self.report.record("gtt_modify", result, note, {"modified": answer, "book_row": row}, body)

    def gtt_cancel(self, trigger_id, step="gtt_cancel"):
        body = {"strategy": STRATEGY, "trigger_id": trigger_id}
        status, answer = self.api.post("/api/v1/cancelgttorder", body)
        if not ok(status, answer):
            return self.report.record(step, FAIL, "HTTP {}: {}".format(status, message_of(answer)),
                                      answer, body)
        gone = self.find_gtt(trigger_id) is None
        if gone:
            self.open_gtts.discard(str(trigger_id))
        return self.report.record(step, PASS if gone else FAIL,
                                  "trigger cancelled" if gone else "still in the active GTT book",
                                  answer, body)

    def find_gtt(self, trigger_id):
        status, answer = self.api.post("/api/v1/gttorderbook", {})
        rows = answer.get("data") if ok(status, answer) else None
        for row in rows if isinstance(rows, list) else []:
            if str(row.get("trigger_id")) == str(trigger_id):
                return row
        return None

    # -- cleanup

    def cleanup(self):
        """Cancel whatever the script placed and still finds open."""
        if not self.placed_any:
            return self.report.record("cleanup", NA, "nothing was placed")
        leftovers = sorted(self.open_orders), sorted(self.open_gtts)
        if not any(leftovers):
            return self.report.record("cleanup", PASS, "nothing the script placed is left open")
        for orderid in leftovers[0]:
            self.cancel(orderid, step="cleanup")
        for trigger_id in leftovers[1]:
            self.gtt_cancel(trigger_id, step="cleanup")
        return None

    # -- whole runs

    def run(self):
        if not self.sign_in():
            return
        self.mode()
        self.master()
        self.books()
        self.market_data()
        self.history()
        self.feed()
        gate = self.order_gate()
        try:
            self.orders(gate)
            self.gtt(gate)
        finally:
            self.cleanup()

    def boundary_check(self):
        """After 03:00 IST: the previous session is gone; the key itself still works."""
        expect_session = self.a.broker in CRYPTO_BROKERS
        self.session_gone("daily_boundary", expect_session,
                          "a crypto session has no 03:00 IST end" if expect_session
                          else "the session ended at the boundary")

    def after_logout(self):
        self.session_gone("logout", False, "the app no longer trades for this broker")

    def session_gone(self, step, expect_session, note):
        status, ping = self.api.post("/api/v1/ping")
        key_status, search = self.api.post("/api/v1/search",
                                           {"query": self.a.symbol, "exchange": self.a.exchange})
        raw = {"ping": [status, ping], "search_with_key_only": [key_status, search]}
        if key_status == 0:
            return self.report.record(step, FAIL, "the app did not answer; is it running?", raw)
        if key_status in (401, 403):
            return self.report.record(step, FAIL, "the API key itself was refused; check the key", raw)
        signed_in = ok(status, ping)
        if signed_in == expect_session and (signed_in or status == 403):
            return self.report.record(step, PASS, note, raw)
        return self.report.record(step, FAIL, "ping answered HTTP {}: {}".format(
            status, message_of(ping)), raw)


def instrument_ok(data, symbol):
    return (isinstance(data, dict) and data.get("symbol") == symbol and bool(data.get("token"))
            and to_decimal(data.get("tick_size")) is not None
            and to_decimal(data.get("lotsize")) is not None)


def candles_ok(data):
    keys = ("timestamp", "open", "high", "low", "close")
    return isinstance(data, list) and len(data) > 0 and all(k in data[0] for k in keys)


def pick(offered, preferred):
    offered = offered or []
    return preferred if preferred in offered else (offered[0] if offered else None)


# --------------------------------------------------------------------- main


def parse_args(argv):
    p = argparse.ArgumentParser(description="Certify one broker on a running OpenAlgo Desktop app.")
    p.add_argument("--broker", required=True, help="catalogue id, e.g. zerodha")
    p.add_argument("--host", default="127.0.0.1")
    p.add_argument("--port", type=int, default=5000, help="the app's HTTP port")
    p.add_argument("--ws-port", type=int, default=8765, help="the app's WebSocket feed port")
    p.add_argument("--api-key", help="visible to other programs; prefer OPENALGO_CERT_API_KEY or the prompt")
    p.add_argument("--symbol", default="SBIN")
    p.add_argument("--exchange", default="NSE")
    p.add_argument("--fo-symbol", help="an F&O contract in OpenAlgo format, e.g. NIFTY30DEC26FUT")
    p.add_argument("--fo-exchange", default="NFO")
    p.add_argument("--product", help="order product (default CNC on NSE/BSE, else NRML)")
    p.add_argument("--place-orders", action="store_true", help="place, modify and cancel one test order")
    p.add_argument("--gtt", action="store_true", help="also place, modify and cancel one GTT")
    p.add_argument("--yes", action="store_true", help="do not ask before placing")
    # 5 % stays inside the exchanges' price bands (10 % dynamic bands on F&O
    # stocks, 2-20 % circuits elsewhere): an order outside them is rejected
    # and could not be modified or cancelled, which is what is being tested.
    p.add_argument("--offset", type=float, default=5.0,
                   help="percent below the last price for the test order (default 5)")
    p.add_argument("--market-closed", action="store_true", help="a feed with no frame is N/A, not FAIL")
    p.add_argument("--boundary-check", action="store_true", help="run only the daily boundary step")
    p.add_argument("--after-logout", action="store_true", help="run only the logout step")
    p.add_argument("--wait", type=float, default=15, help="seconds to wait for the order book")
    p.add_argument("--feed-wait", type=float, default=15, help="seconds to wait for a feed frame")
    p.add_argument("--stream-wait", type=float, default=20, help="seconds to wait for order updates")
    p.add_argument("--out", help="report path without extension")
    a = p.parse_args(argv)
    if not 0 < a.offset < 50:
        p.error("--offset must be between 0 and 50 percent")
    return a


def read_api_key(args):
    if args.api_key:
        print("Warning: --api-key is visible to other programs on this computer. "
              "Prefer OPENALGO_CERT_API_KEY or the prompt.", file=sys.stderr)
        return args.api_key
    key = os.environ.get("OPENALGO_CERT_API_KEY", "").strip()
    if key:
        return key
    if sys.stdin.isatty():
        return getpass.getpass("OpenAlgo API key (from the API Key page): ").strip()
    return ""


def meta_for(args):
    options = {k: v for k, v in vars(args).items() if k != "api_key"}
    now = datetime.datetime.now(IST)
    return {"broker": args.broker, "started_ist": now.strftime("%Y-%m-%d %H:%M:%S IST"),
            "app": "http://{}:{}".format(args.host, args.port),
            "feed": "ws://{}:{}".format(args.host, args.ws_port),
            "mode": "unknown", "script_version": VERSION, "options": options}


def main(argv=None):
    args = parse_args(sys.argv[1:] if argv is None else argv)
    api_key = read_api_key(args)
    if not api_key:
        print("No API key: set OPENALGO_CERT_API_KEY or run in a terminal to be asked.", file=sys.stderr)
        return 2
    report = Report(meta_for(args), Redactor(api_key))
    certifier = Certifier(args, Api(args.host, args.port, api_key), report)
    try:
        if args.boundary_check:
            certifier.boundary_check()
        elif args.after_logout:
            certifier.after_logout()
        else:
            certifier.run()
    finally:
        stamp = datetime.datetime.now(IST).strftime("%Y%m%d-%H%M%S")
        base = args.out or "certification-{}-{}".format(args.broker, stamp)
        paths = report.write(base)
        print("Report: {} and {}".format(*paths))
    return 1 if report.failed() else 0


if __name__ == "__main__":
    sys.exit(main())
