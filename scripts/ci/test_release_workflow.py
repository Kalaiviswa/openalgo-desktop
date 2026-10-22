#!/usr/bin/env python3
"""Tests of how release.yml keeps CI's E2E test build out of a release.

    python3 scripts/ci/test_release_workflow.py

The release build's binary is checked (scripts/ci/check_no_e2e_build.py
--tauri-hook) inside the tauri-action build, as Tauri's
beforeBundleCommand (src-tauri/release-checks.conf.json): after compiling
and before bundling, so the binary checked is the one bundled, and a
refused one stops the build before anything is bundled or uploaded. Right
after tauri-action, a backstop compares the bundled binary with the
digest the check recorded. Read as text, so no YAML library is needed.
"""

from __future__ import annotations

import json
import os
import re
import sys
import unittest

sys.dont_write_bytecode = True  # leave no __pycache__ in the checkout

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..")
RELEASE = os.path.join(ROOT, ".github", "workflows", "release.yml")
CHECKS = os.path.join(ROOT, "src-tauri", "release-checks.conf.json")
HOOK = "python scripts/ci/check_no_e2e_build.py --tauri-hook"


def job_steps(text: str, job: str) -> list[str]:
    """The steps of `job`, each as its text, in order."""
    start = text.index(f"\n  {job}:\n")
    end = re.search(r"\n  [a-z][a-z0-9_-]*:\n", text[start + 1 :])
    body = text[start : start + 1 + end.start()] if end else text[start:]
    steps = body[body.index("\n    steps:\n") :]
    return re.split(r"\n      - ", steps)[1:]


def index(steps: list[str], pred) -> int:
    return next((i for i, s in enumerate(steps) if pred(s)), -1)


def uploads(step: str) -> bool:
    return "tauri-apps/tauri-action@" in step or "gh release upload" in step or "upload_url" in step


class ReleaseBuild(unittest.TestCase):
    def setUp(self):
        with open(RELEASE, encoding="utf-8") as f:
            self.steps = job_steps(f.read(), "build")
        self.action = index(self.steps, lambda s: "tauri-apps/tauri-action@" in s)

    def test_the_only_upload_is_tauri_action_and_it_runs_the_check_before_bundling(self):
        self.assertGreaterEqual(self.action, 0)
        self.assertEqual(index(self.steps, uploads), self.action)
        args = re.search(r"\n\s+args: (.*)", self.steps[self.action]).group(1)
        self.assertIn("--config src-tauri/release-checks.conf.json", args)
        with open(CHECKS, encoding="utf-8") as f:
            self.assertEqual(json.load(f), {"build": {"beforeBundleCommand": HOOK}})

    def test_python_is_there_for_the_check(self):
        python = index(self.steps, lambda s: "actions/setup-python@" in s)
        self.assertGreaterEqual(python, 0)
        self.assertLess(python, self.action)

    def test_the_macos_rebuild_asks_for_the_app_so_its_update_bundle_is_made(self):
        # tauri-bundler 2.12 archives the .app into the .app.tar.gz only
        # when `app` is among the requested bundles.
        with open(RELEASE, encoding="utf-8") as f:
            text = f.read()
        for name in ("macos-aarch64", "macos-x86_64"):
            entry = text[text.index(f"\n          - name: {name}\n") :]
            bundles = re.search(r"\n            bundles: (.+)", entry).group(1)
            self.assertIn("app", bundles.split(","), name)

    def test_the_backstop_runs_right_after_the_upload_on_every_target(self):
        backstop = self.steps[self.action + 1]
        self.assertIn("scripts/ci/check_no_e2e_build.py --same-as-checked", backstop)
        self.assertIn('"src-tauri/target/${TARGET}/release/openalgo-desktop${EXE}"', backstop)
        self.assertNotIn("\n        if:", backstop)


if __name__ == "__main__":
    unittest.main(verbosity=2)
