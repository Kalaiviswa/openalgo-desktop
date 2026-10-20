"""Self-test for broker_certify.py against a fake OpenAlgo Desktop app.

The fake serves the /api/v1 calls the script makes and a WebSocket feed on
ephemeral loopback ports; nothing reaches a broker or a running app.

    python3 -I scripts/certify/test_broker_certify.py
"""

import base64
import contextlib
import hashlib
import json
import os
import socket
import struct
import sys
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import broker_certify  # noqa: E402

API_KEY = "<APIKEY>"
USER_ID = "alice-secret-user"
JWT = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ4In0.c2ln"
SECRETS = [API_KEY, USER_ID, JWT, "AB1234", "trader@example.com", "9876543210", "ABCDE1234F"]


# ------------------------------------------------------------ fake feed


def ws_accept(conn):
    data = b""
    while b"\r\n\r\n" not in data:
        chunk = conn.recv(4096)
        if not chunk:
            return False
        data += chunk
    key = ""
    for line in data.decode("latin-1").split("\r\n"):
        if line.lower().startswith("sec-websocket-key:"):
            key = line.split(":", 1)[1].strip()
    accept = base64.b64encode(hashlib.sha1((key + broker_certify.WsClient.GUID).encode()).digest())
    conn.sendall(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n"
                 b"Connection: Upgrade\r\nSec-WebSocket-Accept: " + accept + b"\r\n\r\n")
    return True


def recv_exact(conn, n):
    out = b""
    while len(out) < n:
        chunk = conn.recv(n - len(out))
        if not chunk:
            raise ConnectionError()
        out += chunk
    return out


def ws_read(conn):
    """(opcode, payload) of one client frame (clients always mask)."""
    b0, b1 = recv_exact(conn, 2)
    length = b1 & 0x7F
    if length == 126:
        length = struct.unpack("!H", recv_exact(conn, 2))[0]
    elif length == 127:
        length = struct.unpack("!Q", recv_exact(conn, 8))[0]
    mask = recv_exact(conn, 4)
    payload = recv_exact(conn, length)
    return b0 & 0x0F, bytes(b ^ mask[i % 4] for i, b in enumerate(payload))


def ws_send(conn, lock, opcode, payload):
    n = len(payload)
    header = struct.pack("!BB", 0x80 | opcode, n) if n < 126 else struct.pack("!BBH", 0x80 | opcode, 126, n)
    with lock:
        conn.sendall(header + payload)


class FeedConnection:
    def __init__(self, app, conn):
        self.app, self.conn, self.lock = app, conn, threading.Lock()

    def send(self, obj):
        ws_send(self.conn, self.lock, 0x1, json.dumps(obj).encode())

    def serve(self):
        if not ws_accept(self.conn):
            return
        ws_send(self.conn, self.lock, 0x9, b"are-you-there")
        try:
            while True:
                opcode, payload = ws_read(self.conn)
                if opcode == 0x8:
                    return
                if opcode == 0x1:
                    self.handle(json.loads(payload))
        except (ConnectionError, OSError):
            return
        finally:
            self.app.unsubscribe_orders(self)
            self.conn.close()

    def handle(self, msg):
        action = msg.get("action")
        if action == "authenticate":
            if msg.get("api_key") != API_KEY:
                return self.send({"status": "error", "code": "AUTHENTICATION_ERROR",
                                  "message": "Invalid API key"})
            return self.send({"type": "auth", "status": "success", "broker": self.app.broker,
                              "user_id": USER_ID, "message": "Authentication successful"})
        if action == "subscribe":
            inst = msg["symbols"][0]
            self.send({"type": "subscribe", "status": "success", "broker": self.app.broker,
                       "subscriptions": [dict(inst, status="success", mode=msg["mode"])]})
            return self.send({"type": "market_data", "mode": msg["mode"], "broker": self.app.broker,
                              "symbol": inst["symbol"], "exchange": inst["exchange"],
                              "data": {"ltp": self.app.ltp or 0, "timestamp": 1}})
        if action == "unsubscribe":
            return self.send({"type": "unsubscribe", "status": "success"})
        if action == "subscribe_orders":
            self.app.subscribe_orders(self)
            return self.send({"type": "subscribe_orders", "status": "success",
                              "message": "Subscribed to order updates",
                              "request_id": msg.get("request_id")})


# ------------------------------------------------------------- fake app


class FakeApp:
    """The /api/v1 answers and the feed of a signed-in app, scriptable."""

    def __init__(self, broker="zerodha", ltp=800.0, analyze=False, signed_in=True,
                 fail_modify=False, cancel_failures=0, funds_extra=None):
        self.broker, self.ltp, self.analyze, self.signed_in = broker, ltp, analyze, signed_in
        self.fail_modify, self.cancel_failures = fail_modify, cancel_failures
        self.funds_extra = funds_extra or {}
        self.orders, self.gtts, self.calls = {}, {}, []
        self.order_subscribers = []
        self.lock = threading.Lock()
        self.http = ThreadingHTTPServer(("127.0.0.1", 0), self._handler())
        self.ws = socket.socket()
        self.ws.bind(("127.0.0.1", 0))
        self.ws.listen()
        self.connections = []
        threading.Thread(target=self.http.serve_forever, daemon=True).start()
        self.acceptor = threading.Thread(target=self._accept, daemon=True)
        self.acceptor.start()

    @property
    def port(self):
        return self.http.server_address[1]

    @property
    def ws_port(self):
        return self.ws.getsockname()[1]

    def close(self):
        self.http.shutdown()
        self.http.server_close()
        try:
            self.ws.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        self.ws.close()
        self.acceptor.join(timeout=2)
        for t in self.connections:
            t.join(timeout=2)

    def _accept(self):
        while True:
            try:
                conn, _ = self.ws.accept()
            except OSError:
                return
            t = threading.Thread(target=FeedConnection(self, conn).serve, daemon=True)
            self.connections.append(t)
            t.start()

    def subscribe_orders(self, c):
        with self.lock:
            self.order_subscribers.append(c)

    def unsubscribe_orders(self, c):
        with self.lock:
            if c in self.order_subscribers:
                self.order_subscribers.remove(c)

    def push(self, order):
        update = {"type": "order_update", "user_id": USER_ID, "mode": "live", "broker": self.broker,
                  "orderid": order["orderid"], "order_status": order["order_status"],
                  "price": order["price"]}
        with self.lock:
            subscribers = list(self.order_subscribers)
        for c in subscribers:
            try:
                c.send(update)
            except OSError:
                pass

    def _handler(self):
        app = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers["Content-Length"])) or b"{}")
                app.calls.append(self.path)
                status, answer = app.answer(self.path, body)
                data = json.dumps(answer).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        return Handler

    # -- routes

    def answer(self, path, body):
        if body.get("apikey") != API_KEY:
            return 403, {"status": "error", "message": "Invalid openalgo apikey"}
        name = path.rsplit("/", 1)[-1]
        if name in ("search", "expiry"):
            return getattr(self, name)(body)
        if not self.signed_in:
            return 403, {"status": "error", "message": "Invalid openalgo apikey"}
        return getattr(self, name)(body)

    @staticmethod
    def success(**fields):
        return 200, dict(status="success", **fields)

    def ping(self, _):
        return self.success(data={"message": "pong", "broker": self.broker})

    def analyzer(self, _):
        return self.success(data={"analyze_mode": self.analyze,
                                  "mode": "analyze" if self.analyze else "live", "total_logs": 0})

    def search(self, b):
        return self.success(data=[{"symbol": b["query"], "exchange": b["exchange"], "token": "3045"}])

    def expiry(self, _):
        return self.success(data=["30-DEC-26"])

    def symbol(self, b):
        return self.success(data={"symbol": b["symbol"], "exchange": b["exchange"], "token": "3045",
                                  "tick_size": 0.05, "lotsize": 1, "expiry": "", "name": "SBIN"})

    def orderbook(self, _):
        return self.success(data={"orders": list(self.orders.values()), "statistics": {}})

    def tradebook(self, _):
        return self.success(data=[])

    def positionbook(self, _):
        return self.success(data=[])

    def holdings(self, _):
        return self.success(data={"holdings": [], "statistics": {}})

    def funds(self, _):
        return self.success(data=dict({"availablecash": "1000.00"}, **self.funds_extra),
                            message="funds for key " + API_KEY)

    def quotes(self, _):
        return self.success(data={} if self.ltp is None else {"ltp": self.ltp})

    def multiquotes(self, b):
        return self.success(results=[dict(s, data={"ltp": self.ltp}) for s in b["symbols"]])

    def depth(self, _):
        return self.success(data={"bids": [{"price": 799.9, "quantity": 5}],
                                  "asks": [{"price": 800.1, "quantity": 5}]})

    def intervals(self, _):
        return self.success(data={"days": ["D"], "minutes": ["1m", "5m"], "hours": []})

    def history(self, _):
        return self.success(data=[{"timestamp": 1, "open": 1, "high": 2, "low": 1, "close": 2}])

    def placeorder(self, b):
        orderid = "2510200000{:03d}".format(len(self.orders) + 1)
        order = {"orderid": orderid, "order_status": "open", "price": b["price"], "symbol": b["symbol"]}
        self.orders[orderid] = order
        self.push(order)
        return self.success(orderid=orderid)

    def modifyorder(self, b):
        if self.fail_modify:
            return 400, {"status": "error", "message": "RMS: modify refused for client AB1234"}
        order = self.orders[b["orderid"]]
        order["price"] = b["price"]
        self.push(order)
        return self.success(orderid=b["orderid"])

    def cancelorder(self, b):
        if self.cancel_failures:
            self.cancel_failures -= 1
            return 500, {"status": "error", "message": "The broker is busy"}
        order = self.orders[b["orderid"]]
        order["order_status"] = "cancelled"
        self.push(order)
        return self.success(orderid=b["orderid"])

    def orderstatus(self, b):
        return self.success(data=dict(self.orders[b["orderid"]]))

    def placegttorder(self, b):
        trigger_id = "9{:05d}".format(len(self.gtts) + 1)
        self.gtts[trigger_id] = {"trigger_id": trigger_id, "status": "active",
                                 "trigger_prices": [b["triggerprice_sl"]]}
        return self.success(trigger_id=trigger_id)

    def modifygttorder(self, b):
        self.gtts[b["trigger_id"]]["trigger_prices"] = [b["triggerprice_sl"]]
        return self.success(trigger_id=b["trigger_id"])

    def cancelgttorder(self, b):
        self.gtts[b["trigger_id"]]["status"] = "cancelled"
        return self.success(trigger_id=b["trigger_id"])

    def gttorderbook(self, _):
        return self.success(data=[g for g in self.gtts.values() if g["status"] == "active"])


# ---------------------------------------------------------------- tests


class CertifyTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.apps = []

    def tearDown(self):
        for app in self.apps:
            app.close()
        self.tmp.cleanup()

    def run_script(self, app, *extra):
        self.apps.append(app)
        base = os.path.join(self.tmp.name, "report")
        argv = ["--broker", app.broker, "--port", str(app.port), "--ws-port", str(app.ws_port),
                "--wait", "3", "--feed-wait", "3", "--stream-wait", "3", "--out", base] + list(extra)
        with mock.patch.dict(os.environ, {"OPENALGO_CERT_API_KEY": API_KEY}), \
                open(os.devnull, "w") as quiet, contextlib.redirect_stdout(quiet):
            code = broker_certify.main(argv)
        with open(base + ".json", encoding="utf-8") as f:
            report = json.load(f)
        with open(base + ".md", encoding="utf-8") as f:
            md = f.read()
        results = {s["step"]: s["result"] for s in report["steps"]}
        return code, results, report, md

    def test_a_full_run_passes_and_leaves_nothing_open(self):
        app = FakeApp()
        code, results, _, md = self.run_script(app, "--place-orders", "--gtt", "--yes")
        wrong = {k: v for k, v in results.items() if v != broker_certify.PASS
                 and k not in ("master_fo_symbol", "master_expiry")}
        self.assertEqual(wrong, {})
        self.assertEqual(code, 0)
        self.assertEqual([o["order_status"] for o in app.orders.values()], ["cancelled"])
        self.assertEqual([g["status"] for g in app.gtts.values()], ["cancelled"])
        placed = app.orders["2510200000001"]
        self.assertLess(float(placed["price"]), 800 * 0.95 + 0.001)
        self.assertIn("| order_stream_updates | PASS |", md)

    def test_a_failed_step_still_cancels_the_order(self):
        app = FakeApp(fail_modify=True, cancel_failures=1)
        code, results, _, _ = self.run_script(app, "--place-orders", "--yes")
        self.assertEqual(results["order_place"], broker_certify.PASS)
        self.assertEqual(results["order_modify"], broker_certify.FAIL)
        self.assertEqual(results["order_cancel"], broker_certify.FAIL)
        self.assertEqual(results["cleanup"], broker_certify.PASS)
        self.assertEqual(app.orders["2510200000001"]["order_status"], "cancelled")
        self.assertEqual(code, 1)

    def test_without_place_orders_no_order_is_sent(self):
        app = FakeApp()
        code, results, _, _ = self.run_script(app)
        for step in broker_certify.Certifier.ORDER_STEPS + broker_certify.Certifier.GTT_STEPS:
            self.assertEqual(results[step], broker_certify.NA, step)
        self.assertEqual(results["cleanup"], broker_certify.NA)
        self.assertNotIn("/api/v1/placeorder", app.calls)
        self.assertEqual(code, 0)

    def test_secrets_never_reach_the_report(self):
        app = FakeApp(funds_extra={"clientcode": "AB1234", "email": "trader@example.com",
                                   "note": "session {} for AB1234, mobile 9876543210, PAN ABCDE1234F"
                                   .format(JWT)})
        _, _, report, md = self.run_script(app, "--place-orders", "--yes")
        text = json.dumps(report) + md
        leaked = [s for s in SECRETS if s in text]
        self.assertEqual(leaked, [])
        self.assertIn("<redacted>", text)

    def test_no_order_without_a_readable_last_price(self):
        app = FakeApp(ltp=None)
        code, results, report, _ = self.run_script(app, "--place-orders", "--yes")
        self.assertEqual(results["quotes"], broker_certify.FAIL)
        self.assertEqual(results["order_place"], broker_certify.FAIL)
        note = next(s["note"] for s in report["steps"] if s["step"] == "order_place")
        self.assertIn("last price", note)
        self.assertNotIn("/api/v1/placeorder", app.calls)
        self.assertEqual(code, 1)

    def test_analyzer_mode_places_nothing(self):
        app = FakeApp(analyze=True)
        _, results, report, _ = self.run_script(app, "--place-orders", "--gtt", "--yes")
        self.assertEqual(results["order_place"], broker_certify.NA)
        self.assertEqual(results["gtt_place"], broker_certify.NA)
        self.assertEqual(report["meta"]["mode"], "analyze")
        self.assertNotIn("/api/v1/placeorder", app.calls)
        self.assertNotIn("/api/v1/placegttorder", app.calls)

    def test_boundary_and_logout_expect_the_session_gone(self):
        code, results, _, _ = self.run_script(FakeApp(signed_in=False), "--boundary-check")
        self.assertEqual((code, results), (0, {"daily_boundary": broker_certify.PASS}))
        code, results, _, _ = self.run_script(FakeApp(signed_in=False), "--after-logout")
        self.assertEqual((code, results), (0, {"logout": broker_certify.PASS}))
        code, results, _, _ = self.run_script(FakeApp(signed_in=True), "--after-logout")
        self.assertEqual((code, results), (1, {"logout": broker_certify.FAIL}))
        # A crypto session has no 03:00 IST end: it must still be there.
        crypto = FakeApp(broker="deltaexchange", signed_in=True)
        code, results, _, _ = self.run_script(crypto, "--boundary-check")
        self.assertEqual((code, results), (0, {"daily_boundary": broker_certify.PASS}))

    def test_prices_round_down_to_the_tick(self):
        self.assertEqual(str(broker_certify.below_market(812.35, 0.05, 5)), "771.70")
        self.assertEqual(str(broker_certify.below_market(24077.8, 5, 5)), "22870")
        self.assertIsNone(broker_certify.below_market(None, 0.05, 5))
        self.assertIsNone(broker_certify.below_market(0.04, 0.05, 5))


if __name__ == "__main__":
    unittest.main()
