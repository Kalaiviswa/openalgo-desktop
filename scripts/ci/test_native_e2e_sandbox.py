#!/usr/bin/env python3
"""Tests of the order book check in scripts/ci/native_e2e_sandbox.py.

    python3 scripts/ci/test_native_e2e_sandbox.py

The native E2E run (after pushes to master) trusts this check to tell a
sandbox order in the web's analyze-mode shape from anything else; these
tests show it refuses each way the answer could be wrong.
"""

from __future__ import annotations

import copy
import json
import os
import sys
import unittest

sys.dont_write_bytecode = True  # leave no __pycache__ in the checkout
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import native_e2e_sandbox as sandbox  # noqa: E402

ORDERID = "26102012345678"
SYMBOL = "NIFTY27OCT2625000CE"


def good_book() -> dict:
    """The web's recorded analyze-mode order book holding the order the run
    places."""
    with open(sandbox.FIXTURE, encoding="utf-8") as f:
        book = json.load(f)["response"]["body"]
    row = copy.deepcopy(book["data"]["orders"][0])
    row.update(
        orderid=ORDERID,
        symbol=SYMBOL,
        exchange="NFO",
        action="BUY",
        quantity=sandbox.LOT,
        filled_quantity=sandbox.LOT,
        pending_quantity=0,
        pricetype="MARKET",
        product="NRML",
        order_status="complete",
        strategy="OptionChain",
        price=sandbox.ATM_ASK,
        average_price=sandbox.ATM_ASK,
        trigger_price=0.0,
    )
    book["data"]["orders"] = [row]
    return book


def problems(book: dict) -> list[str]:
    return sandbox.shape_problems(book, ORDERID, SYMBOL)


class OrderBookCheck(unittest.TestCase):
    def test_the_order_in_the_webs_shape_passes(self):
        self.assertEqual(problems(good_book()), [])

    def test_a_live_answer_is_refused(self):
        book = good_book()
        del book["mode"]
        self.assertTrue(any("order book keys" in p for p in problems(book)))
        book = good_book()
        book["mode"] = "live"
        self.assertEqual(problems(book), ["order book mode is 'live', not 'analyze'"])

    def test_a_missing_or_extra_row_key_is_refused(self):
        book = good_book()
        del book["data"]["orders"][0]["strategy"]
        self.assertTrue(any("row keys" in p for p in problems(book)))
        book = good_book()
        book["data"]["orders"][0]["broker_ref"] = "x"
        self.assertTrue(any("row keys" in p for p in problems(book)))

    def test_another_order_is_not_the_one_placed(self):
        book = good_book()
        book["data"]["orders"][0]["orderid"] = "MOCK-1"
        self.assertTrue(any("not in the API's order book" in p for p in problems(book)))

    def test_a_wrong_value_or_number_type_is_refused(self):
        for field, value in (
            ("order_status", "open"),
            ("product", "MIS"),
            ("quantity", 130),
            ("quantity", 65.0),
            ("filled_quantity", "65"),
            ("average_price", 131.0),
            ("price", 130),
        ):
            book = good_book()
            book["data"]["orders"][0][field] = value
            self.assertNotEqual(problems(book), [], f"{field}={value!r}")


if __name__ == "__main__":
    unittest.main()
