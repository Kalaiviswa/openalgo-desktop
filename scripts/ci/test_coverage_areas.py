#!/usr/bin/env python3
"""Tests of scripts/ci/coverage_areas.py on fabricated llvm-cov summaries.

    python3 scripts/ci/test_coverage_areas.py
"""

from __future__ import annotations

import io
import json
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stdout

sys.dont_write_bytecode = True  # leave no __pycache__ in the checkout
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import coverage_areas  # noqa: E402

BASE = "/home/runner/work/openalgo-desktop/openalgo-desktop/src-tauri/src/"


def summary(files: dict[str, tuple[int, int]]) -> dict:
    return {
        "data": [
            {
                "files": [
                    {"filename": name if name.startswith("/") or ":" in name else BASE + name, "summary": {"lines": {"count": c, "covered": v}}}
                    for name, (c, v) in files.items()
                ]
            }
        ]
    }


class Areas(unittest.TestCase):
    def run_check(self, files, areas, env_summary=False):
        d = tempfile.mkdtemp()
        s, a = os.path.join(d, "s.json"), os.path.join(d, "a.json")
        with open(s, "w", encoding="utf-8") as f:
            json.dump(summary(files), f)
        with open(a, "w", encoding="utf-8") as f:
            json.dump({"areas": areas}, f)
        os.environ.pop("GITHUB_STEP_SUMMARY", None)
        if env_summary:
            os.environ["GITHUB_STEP_SUMMARY"] = os.path.join(d, "summary.md")
        out = io.StringIO()
        with redirect_stdout(out):
            code = coverage_areas.main([s, a])
        return code, out.getvalue(), d

    def test_an_area_at_its_floor_passes(self):
        code, out, _ = self.run_check(
            {"sandbox/engine.rs": (100, 90), "sandbox/fill/mod.rs": (100, 92)},
            {"Sandbox": {"paths": ["sandbox/**"], "lines": 91.0}},
        )
        self.assertEqual(code, 0, out)
        self.assertIn("| Sandbox | 2 | 200 | 91.00% | 91.0% |", out)

    def test_an_area_below_its_floor_fails_though_the_rest_rises(self):
        code, out, _ = self.run_check(
            {"scalping/monitor.rs": (100, 80), "brokers/zerodha/mod.rs": (10_000, 10_000)},
            {"Scalping": {"paths": ["scalping/**"], "lines": 82.7}},
        )
        self.assertEqual(code, 1)
        self.assertIn("Scalping: line coverage 80.00% is below its floor 82.7%", out)

    def test_the_tolerance_is_a_tenth_of_a_point(self):
        files = {"risk/rules.rs": (1000, 885)}  # 88.5%
        self.assertEqual(self.run_check(files, {"Risk": {"paths": ["risk/**"], "lines": 88.6}})[0], 0)
        self.assertEqual(self.run_check(files, {"Risk": {"paths": ["risk/**"], "lines": 88.7}})[0], 1)

    def test_an_area_that_matches_nothing_fails(self):
        code, out, _ = self.run_check(
            {"strategy/mod.rs": (10, 10)},
            {"Moved": {"paths": ["strategies/**"], "lines": 50.0}},
        )
        self.assertEqual(code, 1)
        self.assertIn("Moved: no instrumented file matches its paths", out)

    def test_a_point_above_the_floor_asks_to_raise_it(self):
        code, out, _ = self.run_check(
            {"events/bus.rs": (100, 96)},
            {"Events": {"paths": ["events/**"], "lines": 94.0}},
        )
        self.assertEqual(code, 0)
        self.assertIn("::notice title=Coverage area::Events: line coverage 96.00%", out)

    def test_one_star_stays_in_its_folder_and_two_cross_folders(self):
        files = {
            "services/order_service.rs": (100, 100),
            "services/ui_order_service.rs": (100, 100),
            "services/order_reconciler/matching.rs": (100, 0),
            "services/apikey_service.rs": (100, 0),
        }
        code, out, _ = self.run_check(files, {"Orders": {"paths": ["services/*order*.rs"], "lines": 100.0}})
        self.assertEqual(code, 0, out)
        self.assertIn("| Orders | 2 | 200 | 100.00% |", out)
        code, out, _ = self.run_check(files, {"Orders": {"paths": ["services/**"], "lines": 50.0}})
        self.assertIn("| Orders | 4 | 400 | 50.00% |", out)

    def test_files_outside_the_crate_source_and_windows_paths(self):
        files = {
            "/home/runner/.cargo/registry/src/x/lib.rs": (100, 0),
            "D:\\a\\openalgo-desktop\\openalgo-desktop\\src-tauri\\src\\db\\sqlite\\mod.rs": (100, 95),
        }
        code, out, _ = self.run_check(files, {"Databases": {"paths": ["db/**"], "lines": 95.0}})
        self.assertEqual(code, 0, out)
        self.assertIn("| Databases | 1 | 100 | 95.00% |", out)

    def test_the_table_goes_to_the_job_summary(self):
        _, _, d = self.run_check({"feed/server.rs": (10, 9)}, {"Feed": {"paths": ["feed/**"], "lines": 90.0}}, env_summary=True)
        with open(os.path.join(d, "summary.md"), encoding="utf-8") as f:
            self.assertIn("| Feed | 1 | 10 | 90.00% | 90.0% |", f.read())

    def test_the_checked_in_floors_are_well_formed(self):
        with open(coverage_areas.DEFAULT_AREAS, encoding="utf-8") as f:
            areas = json.load(f)["areas"]
        self.assertGreaterEqual(len(areas), 10)
        for name, area in areas.items():
            self.assertTrue(area["paths"], name)
            self.assertTrue(0 < area["lines"] <= 100, name)


if __name__ == "__main__":
    unittest.main(verbosity=2)
