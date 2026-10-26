#!/usr/bin/env python3
"""Whether a release may rely on the installed-app checks of its commit.

    python3 scripts/ci/smoke_gate.py release <commit sha>

Two workflows' runs of the tagged commit on master decide:

- CI (ci.yml): the install smoke on every platform and the native end-to-end
  run of the .deb are jobs of it (ci.yml calls install-smoke.yml and
  native-e2e.yml after tauri-build, and ci-ok needs them). Only runs for a
  push to master count. A pull request's run of the same commit does not:
  it built the pull request merged into the master of its day.
- The native order journey (native-e2e-test-build.yml): a sandbox order
  through the app's own window on Linux and Windows, on the E2E test build.
  It is not part of ci-ok, so the release reads it here. Runs for a push to
  master and runs by hand on master both count (by hand is how a cancelled
  or missed commit gets its verdict); a pull request's does not.

For each, among those runs the newest one that was not cancelled decides
(a master run stopped because the same commit was already tested on its
pull request, or because a newer push followed, has no verdict; re-run it).
It must have completed with success, and every job the workflow must have
(EXPECTED_JOBS, NATIVE_ORDER_JOBS) must have succeeded; a skipped or
missing job does not count.

GitHub's filtered list of runs sometimes leaves out a run it lists a few
seconds later (seen on 2026-10-22 with every filter combination), so a list
with no run of the commit is asked again before "none" counts.

It reads the GitHub API through `gh api` (GH_TOKEN, GITHUB_REPOSITORY).
SMOKE_GATE_FIXTURES=<dir> answers each API path from <dir> instead, for
the tests: the file name is the path with "/", "?", "&" and "=" replaced by
"_", plus ".json".
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import time
from dataclasses import dataclass

DEFAULT_BRANCH = "master"
# The installed-app jobs as the API names them: the calling job in ci.yml,
# then the job of install-smoke.yml or native-e2e.yml. Keep in step with
# those three files.
EXPECTED_JOBS = (
    "install-smoke / linux (x86_64)",
    "install-smoke / linux (aarch64)",
    "install-smoke / windows (windows-2022)",
    "install-smoke / windows (windows-2025)",
    "install-smoke / macos (aarch64)",
    "native-e2e / native e2e (linux x86_64)",
)
# The jobs of native-e2e-test-build.yml.
NATIVE_ORDER_JOBS = (
    "clippy and catalogue tests (e2e-broker)",
    "native e2e (linux x86_64, test build)",
    "native e2e (windows-2025, test build)",
)
SHA = re.compile(r"^[0-9a-f]{40}$")
# An empty list of runs is asked again this many times in all, this many
# seconds apart.
LIST_ATTEMPTS = 5
LIST_PAUSE = 3.0


@dataclass(frozen=True)
class Gate:
    what: str
    workflow: str
    events: tuple[str, ...]
    jobs: tuple[str, ...]
    passed: str


GATES = (
    Gate("CI", "ci.yml", ("push",), EXPECTED_JOBS, "Installed-app checks of {sha}: every platform passed in {url}"),
    Gate(
        "native order journey",
        "native-e2e-test-build.yml",
        ("push", "workflow_dispatch"),
        NATIVE_ORDER_JOBS,
        "Native order journey of {sha}: a sandbox order through the window on Linux and Windows passed in {url}",
    ),
)


class Refused(Exception):
    pass


def api(path: str):
    fixtures = os.environ.get("SMOKE_GATE_FIXTURES")
    if fixtures:
        name = re.sub(r"[/?&=]", "_", path) + ".json"
        try:
            with open(os.path.join(fixtures, name), encoding="utf-8") as f:
                return json.load(f)
        except FileNotFoundError:
            raise Refused(f"no such API object: {path}") from None
    out = subprocess.run(["gh", "api", path], capture_output=True, text=True)
    if out.returncode != 0:
        raise Refused(f"GitHub API {path}: {out.stderr.strip()[:200]}")
    return json.loads(out.stdout)


def repo() -> str:
    r = os.environ.get("GITHUB_REPOSITORY", "")
    if not re.match(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$", r):
        raise Refused(f"GITHUB_REPOSITORY is not owner/name: {r!r}")
    return r


def master_run_of(run: dict, r: str, sha: str, gate: Gate) -> bool:
    """The run is this repository's `gate` workflow on master for `sha`,
    started by one of the gate's events."""
    return (
        run.get("path") == f".github/workflows/{gate.workflow}"
        and run.get("event") in gate.events
        and run.get("head_branch") == DEFAULT_BRANCH
        and (run.get("head_repository") or {}).get("full_name") == r
        and run.get("head_sha") == sha
        and isinstance(run.get("id"), int)
    )


def master_runs(gate: Gate, r: str, sha: str) -> list[dict]:
    """The gate's runs of `sha` on master, asking again while none is listed."""
    event = f"&event={gate.events[0]}" if len(gate.events) == 1 else ""
    path = f"repos/{r}/actions/workflows/{gate.workflow}/runs?branch={DEFAULT_BRANCH}{event}&head_sha={sha}&per_page=100"
    for attempt in range(LIST_ATTEMPTS):
        if attempt:
            time.sleep(LIST_PAUSE)
        runs = [run for run in api(path).get("workflow_runs", []) if master_run_of(run, r, sha, gate)]
        if runs:
            return runs
    return []


def finished(gate: Gate, run: dict, sha: str) -> None:
    """Refused unless `run` completed with success."""
    if run.get("status") != "completed" or run.get("conclusion") != "success":
        raise Refused(
            f"the newest {gate.what} run of {sha} on {DEFAULT_BRANCH} is "
            f"{run.get('status')}/{run.get('conclusion')}: {run.get('html_url')}"
        )


def jobs_passed(gate: Gate, run: dict, jobs: list[dict], sha: str) -> None:
    """Refused unless every job the gate needs succeeded in `run`."""
    by_name = {j.get("name"): j.get("conclusion") for j in jobs}
    missing = [f"{name}: {by_name.get(name, 'absent')}" for name in gate.jobs if by_name.get(name) != "success"]
    if missing:
        raise Refused(f"the {gate.what} checks of {sha} did not all pass ({', '.join(missing)}): {run.get('html_url')}")


def run_jobs(r: str, run_id: int) -> list[dict]:
    """The jobs of a run's latest attempt, each with its steps."""
    return api(f"repos/{r}/actions/runs/{run_id}/jobs?per_page=100").get("jobs", [])


def verdict(gate: Gate, r: str, sha: str) -> tuple[dict, list[dict]]:
    """The run that decides `gate` for `sha` and its jobs, once it passed;
    Refused otherwise. release_bundles.py ships the bundles of this run."""
    runs = master_runs(gate, r, sha)
    decided = [run for run in runs if run.get("conclusion") != "cancelled"]
    if not decided:
        what = "only cancelled ones; re-run one" if runs else "none"
        raise Refused(f"no {gate.what} run of {sha} on {DEFAULT_BRANCH} has a verdict ({what})")
    newest = max(decided, key=lambda run: (run.get("created_at") or "", run["id"]))
    finished(gate, newest, sha)
    jobs = run_jobs(r, newest["id"])
    jobs_passed(gate, newest, jobs, sha)
    return newest, jobs


def check(gate: Gate, r: str, sha: str) -> str:
    """The gate's verdict on `sha`: what passed, or Refused."""
    run, _jobs = verdict(gate, r, sha)
    return gate.passed.format(sha=sha, url=run.get("html_url"))


def release(sha: str) -> int:
    if not SHA.match(sha or ""):
        raise Refused(f"not a full commit sha: {sha!r}")
    r = repo()
    for gate in GATES:
        print(check(gate, r, sha))
    return 0


def main(argv: list[str]) -> int:
    if len(argv) != 2 or argv[0] != "release":
        print(__doc__)
        return 2
    try:
        return release(argv[1])
    except Refused as e:
        print(f"::error title=Install smoke gate::{e}")
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
