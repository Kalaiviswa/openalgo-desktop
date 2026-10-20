#!/usr/bin/env python3
"""Tests of scripts/ci/smoke_gate.py against fabricated GitHub API answers.

    python3 scripts/ci/test_smoke_gate.py

No network: every API path is answered from a temporary fixture folder
(SMOKE_GATE_FIXTURES), and a path with no fixture is refused.
"""

from __future__ import annotations

import io
import json
import os
import re
import sys
import tempfile
import unittest
from contextlib import redirect_stdout

sys.dont_write_bytecode = True  # leave no __pycache__ in the checkout
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import smoke_gate  # noqa: E402

REPO = "marketcalls/openalgo-desktop"
SHA = "6c804e51853ac878b95d82c622faa5bb49f0cd69"
OTHER_SHA = "aa73a473aec96ac273a5ed6a649f341a17e4d1f2"
GREEN = {name: "success" for name in smoke_gate.EXPECTED_JOBS}
RUNS = f"repos/{REPO}/actions/workflows/ci.yml/runs?branch=master&event=push&head_sha={SHA}&per_page=100"
NATIVE_GREEN = {name: "success" for name in smoke_gate.NATIVE_ORDER_JOBS}
NATIVE_RUNS = f"repos/{REPO}/actions/workflows/native-e2e-test-build.yml/runs?branch=master&head_sha={SHA}&per_page=100"


def ci_run(run_id, created, sha=SHA, conclusion="success", **over):
    run = {
        "id": run_id,
        "path": ".github/workflows/ci.yml",
        "event": "push",
        "head_branch": "master",
        "head_repository": {"full_name": REPO},
        "head_sha": sha,
        "status": "completed",
        "conclusion": conclusion,
        "created_at": created,
        "html_url": f"https://example.invalid/runs/{run_id}",
    }
    run.update(over)
    return run


def native_run(run_id, created, event="push", conclusion="success", **over):
    run = ci_run(run_id, created, conclusion=conclusion, event=event)
    run["path"] = ".github/workflows/native-e2e-test-build.yml"
    run.update(over)
    return run


class Gate(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp()
        os.environ["SMOKE_GATE_FIXTURES"] = self.dir
        os.environ["GITHUB_REPOSITORY"] = REPO
        # The native order journey passed, unless a test says otherwise.
        self.put_native([native_run(900, "2026-10-20T02:00:00Z")], {900: NATIVE_GREEN})

    def put_native(self, runs, jobs_by_run=None):
        self.put(NATIVE_RUNS, {"workflow_runs": runs})
        for run_id, jobs in (jobs_by_run or {}).items():
            self.put(
                f"repos/{REPO}/actions/runs/{run_id}/jobs?per_page=100",
                {"jobs": [{"name": n, "conclusion": c} for n, c in jobs.items()]},
            )

    def put(self, path, body):
        name = re.sub(r"[/?&=]", "_", path) + ".json"
        with open(os.path.join(self.dir, name), "w", encoding="utf-8") as f:
            json.dump(body, f)

    def put_runs(self, runs, jobs_by_run=None):
        self.put(RUNS, {"workflow_runs": runs})
        for run_id, jobs in (jobs_by_run or {}).items():
            self.put(
                f"repos/{REPO}/actions/runs/{run_id}/jobs?per_page=100",
                {"jobs": [{"name": n, "conclusion": c} for n, c in jobs.items()]},
            )

    def run_gate(self, *argv):
        out = io.StringIO()
        with redirect_stdout(out):
            code = smoke_gate.main(list(argv))
        return code, out.getvalue()

    def test_accepts_a_green_master_run_of_the_commit(self):
        self.put_runs([ci_run(100, "2026-10-20T01:00:00Z")], {100: GREEN})
        code, out = self.run_gate("release", SHA)
        self.assertEqual(code, 0, out)
        self.assertIn("every platform passed", out)
        self.assertIn("Native order journey of", out)

    def test_refuses_without_a_native_order_journey_of_the_commit(self):
        self.put_runs([ci_run(100, "2026-10-20T01:00:00Z")], {100: GREEN})
        self.put_native([])
        code, out = self.run_gate("release", SHA)
        self.assertEqual(code, 1)
        self.assertIn("no native order journey run of", out)
        self.assertIn("has a verdict (none)", out)

    def test_refuses_a_failed_or_missing_native_order_job(self):
        self.put_runs([ci_run(100, "2026-10-20T01:00:00Z")], {100: GREEN})
        for job, conclusion in (
            ("native e2e (windows-2025, test build)", "failure"),
            ("native e2e (linux x86_64, test build)", None),
        ):
            jobs = dict(NATIVE_GREEN)
            if conclusion is None:
                del jobs[job]
            else:
                jobs[job] = conclusion
            self.put_native([native_run(901, "2026-10-20T02:00:00Z")], {901: jobs})
            code, out = self.run_gate("release", SHA)
            self.assertEqual(code, 1, job)
            self.assertIn(f"{job}: {conclusion or 'absent'}", out)

    def test_the_newest_native_run_decides_and_a_run_by_hand_counts(self):
        self.put_runs([ci_run(100, "2026-10-20T01:00:00Z")], {100: GREEN})
        # The push run failed; a later run by hand on master passed.
        self.put_native(
            [
                native_run(902, "2026-10-20T02:00:00Z", conclusion="failure"),
                native_run(903, "2026-10-20T04:00:00Z", event="workflow_dispatch"),
                native_run(904, "2026-10-20T05:00:00Z", conclusion="cancelled"),
            ],
            {902: NATIVE_GREEN, 903: NATIVE_GREEN},
        )
        self.assertEqual(self.run_gate("release", SHA)[0], 0)
        # The other way round, the failure is newest.
        self.put_native(
            [
                native_run(903, "2026-10-20T02:00:00Z", event="workflow_dispatch"),
                native_run(902, "2026-10-20T04:00:00Z", conclusion="failure"),
            ],
            {902: NATIVE_GREEN, 903: NATIVE_GREEN},
        )
        code, out = self.run_gate("release", SHA)
        self.assertEqual(code, 1)
        self.assertIn("native order journey run of", out)
        self.assertIn("completed/failure", out)

    def test_a_pull_request_native_run_does_not_count(self):
        self.put_runs([ci_run(100, "2026-10-20T01:00:00Z")], {100: GREEN})
        self.put_native(
            [
                native_run(905, "2026-10-20T02:00:00Z", event="pull_request"),
                native_run(906, "2026-10-20T02:00:00Z", head_branch="pr/x"),
                native_run(907, "2026-10-20T02:00:00Z", path=".github/workflows/ci.yml"),
            ],
            {905: NATIVE_GREEN, 906: NATIVE_GREEN, 907: NATIVE_GREEN},
        )
        code, out = self.run_gate("release", SHA)
        self.assertEqual(code, 1)
        self.assertIn("has a verdict (none)", out)

    def test_refuses_when_a_newer_run_failed(self):
        self.put_runs(
            [
                ci_run(100, "2026-10-20T01:00:00Z"),
                ci_run(101, "2026-10-20T03:00:00Z", conclusion="failure"),
            ],
            {100: GREEN, 101: GREEN},
        )
        code, out = self.run_gate("release", SHA)
        self.assertEqual(code, 1)
        self.assertIn("completed/failure", out)

    def test_refuses_while_the_newest_run_is_still_running(self):
        self.put_runs(
            [
                ci_run(100, "2026-10-20T01:00:00Z"),
                ci_run(101, "2026-10-20T03:00:00Z", status="in_progress", conclusion=None),
            ],
            {100: GREEN},
        )
        code, out = self.run_gate("release", SHA)
        self.assertEqual(code, 1)
        self.assertIn("in_progress/None", out)

    def test_a_cancelled_run_has_no_verdict(self):
        # The newer run was stopped; the older green one decides.
        self.put_runs(
            [
                ci_run(100, "2026-10-20T01:00:00Z"),
                ci_run(101, "2026-10-20T03:00:00Z", conclusion="cancelled"),
            ],
            {100: GREEN},
        )
        self.assertEqual(self.run_gate("release", SHA)[0], 0)

    def test_refuses_when_every_run_was_cancelled(self):
        self.put_runs([ci_run(101, "2026-10-20T03:00:00Z", conclusion="cancelled")])
        code, out = self.run_gate("release", SHA)
        self.assertEqual(code, 1)
        self.assertIn("only cancelled ones; re-run one", out)

    def test_refuses_a_skipped_platform(self):
        jobs = dict(GREEN)
        jobs["install-smoke / windows (windows-2025)"] = "skipped"
        self.put_runs([ci_run(100, "2026-10-20T01:00:00Z")], {100: jobs})
        code, out = self.run_gate("release", SHA)
        self.assertEqual(code, 1)
        self.assertIn("install-smoke / windows (windows-2025): skipped", out)

    def test_refuses_a_run_missing_a_platform(self):
        jobs = {n: c for n, c in GREEN.items() if "macos" not in n}
        self.put_runs([ci_run(100, "2026-10-20T01:00:00Z")], {100: jobs})
        code, out = self.run_gate("release", SHA)
        self.assertEqual(code, 1)
        self.assertIn("install-smoke / macos (aarch64): absent", out)

    def test_refuses_a_failed_native_end_to_end_run(self):
        jobs = dict(GREEN)
        jobs["native-e2e / native e2e (linux x86_64)"] = "failure"
        self.put_runs([ci_run(100, "2026-10-20T01:00:00Z")], {100: jobs})
        self.assertEqual(self.run_gate("release", SHA)[0], 1)

    def test_a_pull_request_run_of_the_same_commit_does_not_count(self):
        # Even from a branch named master: only the event tells.
        self.put_runs(
            [ci_run(102, "2026-10-20T01:00:00Z", event="pull_request")],
            {102: GREEN},
        )
        code, out = self.run_gate("release", SHA)
        self.assertEqual(code, 1)
        self.assertIn("has a verdict (none)", out)

    def test_runs_of_another_commit_repository_workflow_or_branch_do_not_count(self):
        self.put_runs(
            [
                ci_run(103, "2026-10-20T01:00:00Z", sha=OTHER_SHA),
                ci_run(104, "2026-10-20T01:00:00Z", head_repository={"full_name": "someone/fork"}),
                ci_run(105, "2026-10-20T01:00:00Z", path=".github/workflows/soak.yml"),
                ci_run(106, "2026-10-20T01:00:00Z", head_branch="wip/x"),
                ci_run("107", "2026-10-20T01:00:00Z"),
            ],
            {103: GREEN, 104: GREEN, 105: GREEN, 106: GREEN},
        )
        code, out = self.run_gate("release", SHA)
        self.assertEqual(code, 1)
        self.assertIn("has a verdict (none)", out)

    def test_a_newer_green_run_of_a_later_commit_never_stands_in(self):
        # The tagged commit's own run decides, however green a newer run of
        # a later master commit is, in both workflows.
        later = [ci_run(108, "2026-10-20T05:00:00Z", sha=OTHER_SHA)]
        for own, want in (
            ([ci_run(100, "2026-10-20T01:00:00Z", conclusion="failure")], "completed/failure"),
            ([], "has a verdict (none)"),
        ):
            self.put_runs(own + later, {100: GREEN, 108: GREEN})
            code, out = self.run_gate("release", SHA)
            self.assertEqual(code, 1, want)
            self.assertIn(want, out)
        self.put_runs([ci_run(100, "2026-10-20T01:00:00Z")], {100: GREEN})
        later = [native_run(911, "2026-10-20T05:00:00Z", head_sha=OTHER_SHA)]
        for own, want in (
            ([native_run(910, "2026-10-20T02:00:00Z", conclusion="failure")], "completed/failure"),
            ([], "has a verdict (none)"),
        ):
            self.put_native(own + later, {910: NATIVE_GREEN, 911: NATIVE_GREEN})
            code, out = self.run_gate("release", SHA)
            self.assertEqual(code, 1, want)
            self.assertIn(f"native order journey run of {SHA}", out)
            self.assertIn(want, out)

    def test_refuses_a_short_or_odd_sha(self):
        # Green answers wait where an unchecked argument would lead; the
        # gate must refuse before it asks.
        for odd in ("6c804e5", SHA + "&per_page=1"):
            runs = f"repos/{REPO}/actions/workflows/ci.yml/runs?branch=master&event=push&head_sha={odd}&per_page=100"
            self.put(runs, {"workflow_runs": [ci_run(110, "2026-10-20T01:00:00Z", sha=odd)]})
            self.put(f"repos/{REPO}/actions/runs/110/jobs?per_page=100", {"jobs": [{"name": n, "conclusion": "success"} for n in GREEN]})
            code, out = self.run_gate("release", odd)
            self.assertEqual(code, 1, odd)
            self.assertIn("not a full commit sha", out)

    def test_the_expected_jobs_are_the_ones_the_workflows_define(self):
        here = os.path.dirname(os.path.abspath(__file__))
        wf = os.path.join(here, "..", "..", ".github", "workflows")

        def read(name):
            with open(os.path.join(wf, name), encoding="utf-8") as f:
                return f.read()

        ci = read("ci.yml")
        self.assertIn("    name: install-smoke\n    needs: tauri-build\n    uses: ./.github/workflows/install-smoke.yml", ci)
        self.assertIn("    name: native-e2e\n    needs: tauri-build\n    uses: ./.github/workflows/native-e2e.yml", ci)
        ok = ci[ci.index("  ci-ok:") :]
        self.assertIn("      - install-smoke\n", ok)
        self.assertIn("      - native-e2e\n", ok)
        smoke, native = read("install-smoke.yml"), read("native-e2e.yml")
        for os_name in ("ubuntu-22.04", "ubuntu-22.04-arm"):
            self.assertIn(f"os: {os_name}", smoke)
        self.assertIn("os: [windows-2022, windows-2025]", smoke)
        self.assertIn("name: linux (${{ matrix.name }})", smoke)
        self.assertIn("name: windows (${{ matrix.os }})", smoke)
        self.assertIn("name: macos (aarch64)", smoke)
        self.assertIn("name: native e2e (linux x86_64)", native)
        test_build = read("native-e2e-test-build.yml")
        for job in smoke_gate.NATIVE_ORDER_JOBS:
            self.assertIn(f"    name: {job}\n", test_build)
        self.assertEqual(test_build.count("\n    name: "), len(smoke_gate.NATIVE_ORDER_JOBS))
        self.assertIn("  push:\n    branches: [master]\n", test_build)
        self.assertIn("  workflow_dispatch:\n", test_build)


if __name__ == "__main__":
    unittest.main(verbosity=2)
