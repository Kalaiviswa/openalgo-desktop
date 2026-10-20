#!/usr/bin/env python3
"""Whether a release may rely on the installed-app checks of its commit.

    python3 scripts/ci/smoke_gate.py release <commit sha>

The installed-app checks (the install smoke on every platform and the
native end-to-end run) are jobs of the CI workflow: ci.yml calls
install-smoke.yml and native-e2e.yml after tauri-build, and ci-ok needs
them. A release (the `verify` job of release.yml) needs them green on the
bundles of the tagged commit itself, so only this repository's CI runs for
a push to master of <commit sha> count. A pull request's run of the same
commit does not: it built the pull request merged into the master of its
day.

Among those runs the newest one that was not cancelled decides (a master
run stopped because the same commit was already tested on its pull
request has no verdict; re-run it). It must have completed with success,
and every job in EXPECTED_JOBS must have succeeded; a skipped or missing
job does not count.

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

DEFAULT_BRANCH = "master"
CI_WORKFLOW = ".github/workflows/ci.yml"
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
SHA = re.compile(r"^[0-9a-f]{40}$")


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


def master_push_of(run: dict, r: str, sha: str) -> bool:
    """The run is this repository's CI for a push of `sha` to master."""
    return (
        run.get("path") == CI_WORKFLOW
        and run.get("event") == "push"
        and run.get("head_branch") == DEFAULT_BRANCH
        and (run.get("head_repository") or {}).get("full_name") == r
        and run.get("head_sha") == sha
        and isinstance(run.get("id"), int)
    )


def release(sha: str) -> int:
    if not SHA.match(sha or ""):
        raise Refused(f"not a full commit sha: {sha!r}")
    r = repo()
    listed = api(
        f"repos/{r}/actions/workflows/ci.yml/runs?branch={DEFAULT_BRANCH}&event=push&head_sha={sha}&per_page=100"
    )
    runs = [run for run in listed.get("workflow_runs", []) if master_push_of(run, r, sha)]
    decided = [run for run in runs if run.get("conclusion") != "cancelled"]
    if not decided:
        what = "only cancelled ones; re-run one" if runs else "none"
        raise Refused(f"no CI run of a push of {sha} to {DEFAULT_BRANCH} has a verdict ({what})")
    newest = max(decided, key=lambda run: (run.get("created_at") or "", run["id"]))
    url = newest.get("html_url")
    if newest.get("status") != "completed" or newest.get("conclusion") != "success":
        raise Refused(f"the newest CI run of {sha} on {DEFAULT_BRANCH} is {newest.get('status')}/{newest.get('conclusion')}: {url}")
    jobs = {j.get("name"): j.get("conclusion") for j in api(f"repos/{r}/actions/runs/{newest['id']}/jobs?per_page=100").get("jobs", [])}
    missing = [f"{name}: {jobs.get(name, 'absent')}" for name in EXPECTED_JOBS if jobs.get(name) != "success"]
    if missing:
        raise Refused(f"the installed-app checks of {sha} did not all pass ({', '.join(missing)}): {url}")
    print(f"Installed-app checks of {sha}: every platform passed in {url}")
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
