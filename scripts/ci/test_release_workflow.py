#!/usr/bin/env python3
"""Tests of the release workflow's wiring, read from the workflow files.

    python3 scripts/ci/test_release_workflow.py

- CI's E2E test build stays out of a release: the release build's binary
  is checked (scripts/ci/check_no_e2e_build.py --tauri-hook) inside the
  tauri-action build, as Tauri's beforeBundleCommand
  (src-tauri/release-checks.conf.json): after compiling and before
  bundling, so the binary checked is the one bundled, and a refused one
  stops the build before anything is bundled or uploaded. Right after
  tauri-action, a backstop compares the bundled binary with the digest the
  check recorded.
- The advisory scans: the release gate, the daily security.yml run and
  scripts/release/preflight.sh all run scripts/ci/advisory_scan.sh, which
  accepts exactly the advisories .github/deny.toml ignores.

Read as text, so no YAML library is needed.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import unittest

sys.dont_write_bytecode = True  # leave no __pycache__ in the checkout

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..")
RELEASE = os.path.join(ROOT, ".github", "workflows", "release.yml")
SECURITY = os.path.join(ROOT, ".github", "workflows", "security.yml")
PREFLIGHT = os.path.join(ROOT, "scripts", "release", "preflight.sh")
SCAN = os.path.join(ROOT, "scripts", "ci", "advisory_scan.sh")
DENY = os.path.join(ROOT, ".github", "deny.toml")
CHECKS = os.path.join(ROOT, "src-tauri", "release-checks.conf.json")
HOOK = "python scripts/ci/check_no_e2e_build.py --tauri-hook"


def read(path: str) -> str:
    with open(path, encoding="utf-8") as f:
        return f.read()


def job_steps(text: str, job: str) -> list[str]:
    """The steps of `job`, each as its text, in order."""
    start = text.index(f"\n  {job}:\n")
    end = re.search(r"\n  [a-z][a-z0-9_-]*:\n", text[start + 1 :])
    body = text[start : start + 1 + end.start()] if end else text[start:]
    steps = body[body.index("\n    steps:\n") :]
    return re.split(r"\n      - ", steps)[1:]


def index(steps: list[str], pred) -> int:
    return next((i for i, s in enumerate(steps) if pred(s)), -1)


def run_line(step: str) -> str:
    """A one-line `run:` command of a step."""
    m = re.search(r"\n\s+run: (.+)", step)
    return m.group(1).strip() if m else ""


def step_named(steps: list[str], name: str) -> str:
    """The one step whose `name:` is `name`."""
    found = [s for s in steps if re.match(rf"name: {re.escape(name)}\n", s)]
    if len(found) != 1:
        raise AssertionError(f"{len(found)} steps named {name!r}")
    return found[0]


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
        text = read(RELEASE)
        for name in ("macos-aarch64", "macos-x86_64"):
            entry = text[text.index(f"\n          - name: {name}\n") :]
            bundles = re.search(r"\n            bundles: (.+)", entry).group(1)
            self.assertIn("app", bundles.split(","), name)

    def test_the_backstop_runs_right_after_the_upload_on_every_target(self):
        backstop = self.steps[self.action + 1]
        self.assertIn("scripts/ci/check_no_e2e_build.py --same-as-checked", backstop)
        self.assertIn('"src-tauri/target/${TARGET}/release/openalgo-desktop${EXE}"', backstop)
        self.assertNotIn("\n        if:", backstop)


class AdvisoryScans(unittest.TestCase):
    """One script runs the three scans for the gate, the daily scan and the
    preflight, so none of them can pass what another fails."""

    GATE = {
        "No unaccepted advisory, license or source (cargo-deny, today's database)": "cargo-deny",
        "No unaccepted vulnerability (cargo-audit, today's database)": "cargo-audit",
        "No high or critical advisory in the shipped JavaScript (npm audit)": "npm-audit",
    }
    DAILY = {
        "cargo-audit": ("Fail on vulnerabilities (ignores come from .github/deny.toml)", "cargo-audit"),
        "cargo-deny": ("Check advisories, bans, licenses, sources", "cargo-deny"),
        "npm-audit": ("Fail on high or critical", "npm-audit"),
    }

    def test_the_release_gate_runs_the_script(self):
        steps = job_steps(read(RELEASE), "verify")
        for name, scan in self.GATE.items():
            self.assertEqual(run_line(step_named(steps, name)), f"scripts/ci/advisory_scan.sh {scan}")

    def test_the_daily_scan_runs_the_script_every_day(self):
        text = read(SECURITY)
        self.assertRegex(text, r'\n    - cron: "0 2 \* \* \*"')
        for job, (name, scan) in self.DAILY.items():
            self.assertEqual(run_line(step_named(job_steps(text, job), name)), f"scripts/ci/advisory_scan.sh {scan}")

    def test_the_preflight_runs_the_script(self):
        self.assertIn('check "advisories" scripts/ci/advisory_scan.sh all\n', read(PREFLIGHT))

    def test_no_workflow_gates_on_its_own_copy_of_a_scan(self):
        # Only report steps (JSON or SARIF, never failing) call the tools
        # directly; a gating command lives in the script alone.
        for path in (RELEASE, SECURITY):
            # Continued lines joined, names and comments left out.
            for line in read(path).replace("\\\n", " ").splitlines():
                command = line.strip()
                if re.match(r"(- )?name:|#", command):
                    continue
                if re.search(r"\b(cargo (deny|audit)|npm audit)\b", command):
                    self.assertRegex(command, r"--json|--format sarif", f"{os.path.basename(path)}: {command}")

    def test_the_accepted_advisories_are_deny_toml_ignores(self):
        out = subprocess.run(["bash", SCAN, "accepted"], capture_output=True, text=True, check=True).stdout
        ignore = read(DENY).split("\n[advisories]\n", 1)[1].split("\n[", 1)[0]
        ids = re.findall(r'\{ id = "(RUSTSEC-\d{4}-\d{4})"', ignore)
        self.assertTrue(ids)
        self.assertEqual(out.split(), sorted(set(ids)))


if __name__ == "__main__":
    unittest.main(verbosity=2)
