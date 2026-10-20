"""A sandbox order through the app's own window (native_e2e.py --sandbox-order).

Only against the E2E test build (the e2e-broker feature): its scripted
broker (src-tauri/src/brokers/scripted.rs) takes the Dhan (Sandbox) slot,
signs in from the saved keys in one action and quotes a small NIFTY market
(index 25,010, options around it, one lot is 65). With the trader signed
in to OpenAlgo, every step goes through the pages a trader uses:

1. Broker Configuration: the slot's keys are saved; Select Broker: "Login
   with Dhan (Sandbox)" opens the dashboard.
2. API Key: the key made with the account is there (the order form places
   through it).
3. The navigation bar: switch to Analyze mode.
4. Option Chain: the ATM row's call, its B button (shown on hover), then
   "Place BUY Order" in the order form (one lot, MARKET, NRML).
5. Order Book: the order's row with a sandbox order id (14 digits).
6. /api/v1/orderbook, as an API client sees it: mode "analyze", the keys of
   the web's analyze-mode order book (tests/fixtures/web/rest/orderbook/
   populated.json), the order complete at the scripted price.
7. The scripted broker's own count of what it received (the test build's
   GET /e2e/scripted-broker): calls (sign-in, master contract, quotes), and
   no order call. A count, not a missing log line: the order never went to
   the broker.
"""

from __future__ import annotations

import json
import os
import re
import urllib.request

SLOT = "dhan_sandbox"
LOT = 65
ATM_STRIKE = "25000"
# The ATM call's scripted last price and ask.
ATM_LTP = 130.0
ATM_ASK = 130.05
FIXTURE = os.path.join(
    os.path.dirname(os.path.abspath(__file__)),
    "..",
    "..",
    "tests",
    "fixtures",
    "web",
    "rest",
    "orderbook",
    "populated.json",
)

# The page's own fetch, with its session cookie: a GET answered as JSON.
FETCH_JSON = """
const done = arguments[arguments.length - 1];
fetch(arguments[0], {credentials: 'include', headers: {Accept: 'application/json'}})
  .then((r) => r.json())
  .then(done)
  .catch((e) => done({fetch_error: String(e)}));
"""


def keys(d: dict) -> list[str]:
    return sorted(d.keys())


def api_orderbook(base: str, apikey: str) -> dict:
    req = urllib.request.Request(
        base + "/api/v1/orderbook",
        data=json.dumps({"apikey": apikey}).encode(),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=15) as r:
        return json.loads(r.read().decode())


def shape_problems(book: dict, orderid: str, symbol: str) -> list[str]:
    """What in the API's order book differs from the web's analyze-mode
    shape or from the order placed."""
    with open(FIXTURE, encoding="utf-8") as f:
        web = json.load(f)["response"]["body"]
    out = []
    if keys(book) != keys(web):
        out.append(f"order book keys {keys(book)} are not the web's {keys(web)}")
    if book.get("mode") != "analyze":
        out.append(f"order book mode is {book.get('mode')!r}, not 'analyze'")
    data = book.get("data") or {}
    if keys(data.get("statistics") or {}) != keys(web["data"]["statistics"]):
        out.append(f"order book statistics keys are {keys(data.get('statistics') or {})}")
    orders = [o for o in data.get("orders") or [] if o.get("orderid") == orderid]
    if len(orders) != 1:
        out.append(f"order {orderid} is not in the API's order book once: {data.get('orders')}")
        return out
    o = orders[0]
    if keys(o) != keys(web["data"]["orders"][0]):
        out.append(f"order row keys {keys(o)} are not the web's {keys(web['data']['orders'][0])}")
    want = {
        "symbol": symbol,
        "exchange": "NFO",
        "action": "BUY",
        "quantity": LOT,
        "pricetype": "MARKET",
        "product": "NRML",
        "order_status": "complete",
        "strategy": "OptionChain",
    }
    for k, v in want.items():
        if o.get(k) != v:
            out.append(f"order {k} is {o.get(k)!r}, expected {v!r}")
    avg = o.get("average_price")
    if not isinstance(avg, float) or not ATM_LTP <= avg <= ATM_ASK + 1e-6:
        out.append(f"order average price {avg!r} is not the scripted price ({ATM_LTP} to {ATM_ASK})")
    for k in ("quantity", "filled_quantity", "pending_quantity"):
        if type(o.get(k)) is not int:
            out.append(f"order {k} is not an integer: {o.get(k)!r}")
    for k in ("price", "trigger_price", "average_price"):
        if type(o.get(k)) is not float:
            out.append(f"order {k} is not a number with a fraction: {o.get(k)!r}")
    return out


def run(session, wait, base: str, path) -> list[str]:
    """Steps 1-7; returns what failed. `path()` is the page's path."""
    from selenium.webdriver.common.action_chains import ActionChains
    from selenium.webdriver.common.by import By
    from selenium.webdriver.support import expected_conditions as ec
    from selenium.webdriver.support.ui import WebDriverWait

    problems: list[str] = []

    def field(element_id: str, value: str) -> None:
        el = wait.until(ec.element_to_be_clickable((By.ID, element_id)))
        el.clear()
        el.send_keys(value)

    def button(xpath: str):
        return wait.until(ec.element_to_be_clickable((By.XPATH, xpath)))

    def page_json(url: str) -> dict:
        return session.execute_async_script(FETCH_JSON, url)

    def text_of(el) -> str:
        # The DOM's text: WebKitWebDriver reads badge and truncated text as
        # empty.
        return (el.get_attribute("textContent") or "").strip()

    # 1. The slot's keys, then log in with them.
    session.get(f"{base}/broker/config?broker={SLOT}")
    field("broker-field-api_key", "1000000001")
    field("broker-field-api_secret", "scripted")
    button("//form//button[@type='submit']").click()
    wait.until(ec.visibility_of_element_located((By.CSS_SELECTOR, "[data-testid=saved-api_key]")))
    print("broker keys saved for Dhan (Sandbox), the scripted broker's slot")
    session.get(base + "/broker")
    card = wait.until(ec.visibility_of_element_located((By.CSS_SELECTOR, f"[data-testid=broker-card-{SLOT}]")))
    card.find_element(By.XPATH, ".//button[starts-with(normalize-space(), 'Login with')]").click()
    wait.until(lambda d: path() == "/dashboard")
    print("logged in to the scripted broker: on /dashboard")

    # 2. The API key the order form places with, made with the account
    # (Copy is enabled once the page has it).
    session.get(base + "/apikey")
    button("//button[normalize-space()='Copy']")
    print("API key page: the account's key is there")

    # 3. Analyzer mode.
    button("//button[@aria-label='Switch to Analyze mode']").click()
    wait.until(ec.presence_of_element_located((By.XPATH, "//button[@aria-label='Switch to Live mode']")))
    print("analyzer mode on")

    # Steps 4 to 6 report a failure and go on, so step 7 always says what
    # reached the broker.
    try:
        # 4. The order form, from the Option Chain's ATM call, once the master
        # contract the page lists from has loaded.
        wait.until(lambda d: page_json("/api/master-contract/ready").get("ready") is True)
        session.get(base + "/optionchain")
        row = wait.until(ec.visibility_of_element_located((By.CSS_SELECTOR, "tr[data-label=ATM]")))
        strike = row.get_attribute("data-strike") or ""
        if strike != ATM_STRIKE:
            problems.append(f"the ATM strike is {strike!r}, expected {ATM_STRIKE}")
        buy = row.find_element(By.XPATH, "./td[1]//button[normalize-space()='B']")
        # The B and S buttons show while the row is hovered, as for a
        # trader; the pointer reaches only what is in view.
        session.execute_script("arguments[0].scrollIntoView({block: 'center'})", row)
        strike_cell = row.find_element(By.XPATH, f"./td[normalize-space()='{ATM_STRIKE}']")
        try:
            ActionChains(session).move_to_element(strike_cell).perform()
            WebDriverWait(session, 5).until(lambda d: buy.is_displayed())
            buy.click()
            print("B pressed with the pointer on the hovered row")
        except Exception as e:  # noqa: BLE001 - say which way it went
            # A driver whose pointer does not hover the row (WebKitWebDriver
            # under Xvfb) leaves the hover-only button hidden to it; press
            # it by script. The order form and everything after it are
            # driven as before.
            print(f"the pointer did not show the B button ({type(e).__name__}: {str(e).strip()[:120]}); pressing it by script")
            session.execute_script("arguments[0].click()", buy)
        title = wait.until(ec.visibility_of_element_located((By.CSS_SELECTOR, "[role=dialog] h2")))
        symbol = text_of(title.find_elements(By.TAG_NAME, "span")[-1])
        if not (symbol.startswith("NIFTY") and symbol.endswith(f"{ATM_STRIKE}CE")):
            problems.append(f"the order form is for {symbol!r}, not the ATM call")
        print(f"order form open: BUY {symbol}")
        button("//*[@role='dialog']//button[normalize-space()='Place BUY Order']").click()
        try:
            wait.until(ec.invisibility_of_element_located((By.CSS_SELECTOR, "[role=dialog]")))
        except Exception:  # noqa: BLE001 - say what the page showed
            toasts = [t.text for t in session.find_elements(By.CSS_SELECTOR, "[data-sonner-toast]")]
            raise RuntimeError(f"the order form stayed open; the page said {toasts}") from None
        print("order placed from the form")

        # 5. The Order Book page.
        session.get(base + "/orderbook")
        cells_xpath = f"//tbody/tr[td[1][contains(normalize-space(), '{symbol}')]]/td"
        wait.until(ec.visibility_of_element_located((By.XPATH, cells_xpath)))
        cells = [text_of(c) for c in session.find_elements(By.XPATH, cells_xpath)]
        ids = [c for c in cells if re.fullmatch(r"\d{14}", c)]
        if len(ids) != 1:
            problems.append(f"the order book row has no sandbox order id: {cells}")
            orderid = ""
        else:
            orderid = ids[0]
        for want in ("NFO", "BUY", str(LOT), "MARKET", "NRML", "complete"):
            if want not in cells:
                problems.append(f"the order book row lacks {want!r}: {cells}")
        print(f"order book row: {cells}")

        # 6. The same order, as an API client reads it.
        apikey = page_json("/auth/session-status").get("api_key") or ""
        if not apikey:
            problems.append("the session has no API key")
        else:
            problems += shape_problems(api_orderbook(base, apikey), orderid, symbol)
            print("API order book: analyze mode, the web's keys, the order complete")
    except Exception as e:  # noqa: BLE001 - report it, then count
        problems.append(f"{type(e).__name__}: {e} (at {session.current_url})")
        shot = os.path.join(os.environ.get("RUNNER_TEMP", "."), "native-e2e-failure.png")
        session.save_screenshot(shot)
        print(f"screenshot: {shot}")

    # 7. What reached the broker, by its own count.
    counts = page_json("/e2e/scripted-broker")
    print(f"scripted broker counts: {counts}")
    if not isinstance(counts.get("calls"), int) or counts["calls"] < 3:
        problems.append(f"the scripted broker's count does not show the sign-in, master and quotes: {counts}")
    if counts.get("order_calls") != 0:
        problems.append(f"order calls reached the scripted broker: {counts}")
    return problems
