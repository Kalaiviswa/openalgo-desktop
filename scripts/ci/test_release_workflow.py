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
- A tag push ships CI's tested bundles: CI's tauri-build makes them the way
  the release build did (the update bundles, unsigned; the check before
  bundling and its backstop), and release.yml's ci-bundles job signs and
  attaches them without building; the rebuild runs only by hand; the update
  key reaches only the steps that sign.

Read as text, so no YAML library is needed.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True  # leave no __pycache__ in the checkout

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..")
RELEASE = os.path.join(ROOT, ".github", "workflows", "release.yml")
CI = os.path.join(ROOT, ".github", "workflows", "ci.yml")
SECURITY = os.path.join(ROOT, ".github", "workflows", "security.yml")
PREFLIGHT = os.path.join(ROOT, "scripts", "release", "preflight.sh")
SCAN = os.path.join(ROOT, "scripts", "ci", "advisory_scan.sh")
DENY = os.path.join(ROOT, ".github", "deny.toml")
CHECKS = os.path.join(ROOT, "src-tauri", "release-checks.conf.json")
HOOK = "python scripts/ci/check_no_e2e_build.py --tauri-hook"


def read(path: str) -> str:
    with open(path, encoding="utf-8") as f:
        return f.read()


def job_text(text: str, job: str) -> str:
    """The whole of `job`, its header included."""
    start = text.index(f"\n  {job}:\n")
    end = re.search(r"\n  [a-z][a-z0-9_-]*:\n", text[start + 1 :])
    return text[start : start + 1 + end.start()] if end else text[start:]


def job_steps(text: str, job: str) -> list[str]:
    """The steps of `job`, each as its text, in order."""
    body = job_text(text, job)
    steps = body[body.index("\n    steps:\n") :]
    return re.split(r"\n      - ", steps)[1:]


def job_if(text: str, job: str) -> str:
    m = re.search(r"\n    if: (.+)", job_text(text, job))
    return m.group(1).strip() if m else ""


def script(step: str) -> str:
    """The `run: |` block of a step, unindented."""
    body = step[step.index("run: |\n") + len("run: |\n") :]
    return "\n".join(line[4:] for line in body.splitlines())


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


class CiBuildsWhatShips(unittest.TestCase):
    """CI's tauri-build makes the files a release ships, so it builds them as
    the release build did, with the same checks."""

    def setUp(self):
        self.text = read(CI)
        self.steps = job_steps(self.text, "tauri-build")
        self.action = index(self.steps, lambda s: "tauri-apps/tauri-action@" in s)

    def test_the_update_bundles_are_built_unsigned_with_the_check_before_bundling(self):
        args = re.search(r"\n\s+args: (.*)", self.steps[self.action]).group(1)
        for flag in (
            "--no-sign",
            "--config src-tauri/updater-artifacts.conf.json",
            "--config src-tauri/release-checks.conf.json",
        ):
            self.assertIn(flag, args)
        with open(os.path.join(ROOT, "src-tauri", "updater-artifacts.conf.json"), encoding="utf-8") as f:
            self.assertEqual(json.load(f), {"bundle": {"createUpdaterArtifacts": True}})
        self.assertGreaterEqual(index(self.steps, lambda s: "actions/setup-python@" in s), 0)
        self.assertLess(index(self.steps, lambda s: "actions/setup-python@" in s), self.action)

    def test_the_backstop_runs_right_after_the_build_on_every_target(self):
        backstop = self.steps[self.action + 1]
        self.assertIn("scripts/ci/check_no_e2e_build.py --same-as-checked", backstop)
        self.assertIn('"src-tauri/target/${TARGET}/release/openalgo-desktop${EXE}"', backstop)
        self.assertNotIn("\n        if:", backstop)

    def test_the_steps_a_release_requires_are_there_and_unconditional(self):
        sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
        import release_bundles

        for name in release_bundles.TAURI_BUILD_STEPS:
            step = step_named(self.steps, name)
            self.assertNotIn("\n        if:", step, name)
            self.assertNotIn("continue-on-error", step, name)

    def test_the_macos_update_bundle_is_uploaded(self):
        upload = step_named(self.steps, "Upload bundles")
        self.assertIn("src-tauri/target/${{ matrix.target }}/release/bundle/macos/*.app.tar.gz", upload)

    def test_ci_never_holds_the_update_key(self):
        self.assertNotIn("TAURI_SIGNING_PRIVATE_KEY", self.text)


class ReleaseShipsWhatCiTested(unittest.TestCase):
    def setUp(self):
        self.text = read(RELEASE)

    def mode(self, event, build="", ref="refs/heads/master", tag="v1.2.3"):
        """Run the gate's first step as the workflow would."""
        step = step_named(job_steps(self.text, "verify"), "The tag name is a release tag, and what this run does")
        with tempfile.NamedTemporaryFile("r", suffix=".out") as out:
            env = dict(os.environ, TAG=tag, EVENT=event, BUILD=build, GITHUB_REF=ref, GITHUB_OUTPUT=out.name)
            r = subprocess.run(["bash", "-c", script(step)], env=env, capture_output=True, text=True)
            return r.returncode, dict(line.split("=", 1) for line in out.read().split())

    def test_a_tag_push_ships_ci_bundles_and_a_run_by_hand_does_what_it_says(self):
        self.assertEqual(self.mode("push", ref="refs/tags/v1.2.3"), (0, {"tag": "v1.2.3", "mode": "ci-bundles"}))
        self.assertEqual(self.mode("workflow_dispatch", "none")[1]["mode"], "none")
        self.assertEqual(self.mode("workflow_dispatch", "rebuild")[1]["mode"], "rebuild")
        self.assertEqual(self.mode("workflow_dispatch", "ci-bundles")[1]["mode"], "ci-bundles")
        # A gate-only run may come from any branch; a release only from master.
        self.assertEqual(self.mode("workflow_dispatch", "none", ref="refs/heads/x")[0], 0)
        self.assertEqual(self.mode("workflow_dispatch", "rebuild", ref="refs/heads/x")[0], 1)
        self.assertEqual(self.mode("workflow_dispatch", "nonsense")[0], 1)
        self.assertEqual(self.mode("push", tag="1.2.3")[0], 1)

    def test_each_path_runs_only_in_its_mode(self):
        self.assertEqual(job_if(self.text, "create-release"), "needs.verify.outputs.mode != 'none'")
        self.assertEqual(job_if(self.text, "ci-bundles"), "needs.verify.outputs.mode == 'ci-bundles'")
        self.assertEqual(job_if(self.text, "build"), "needs.verify.outputs.mode == 'rebuild'")
        checksums = job_text(self.text, "checksums")
        self.assertIn("needs: [verify, create-release, ci-bundles, build]", checksums)
        self.assertIn("needs.ci-bundles.result == 'success' || needs.build.result == 'success'", checksums)
        self.assertIn("--minisign minisign", checksums)

    def test_the_gate_checks_ci_bundles_unless_rebuilding(self):
        steps = job_steps(self.text, "verify")
        bundles = step_named(steps, "CI's tested bundles of this commit are there to sign")
        self.assertIn('run: python3 scripts/ci/release_bundles.py check "$SHA"', bundles)
        self.assertIn("if: steps.mode.outputs.mode != 'rebuild'", bundles)
        self.assertNotIn("continue-on-error", bundles)
        signing = step_named(steps, "No code signing is configured that CI's installers lack")
        self.assertIn("if: steps.mode.outputs.mode != 'rebuild'", signing)
        self.assertLess(
            index(steps, lambda s: "scripts/ci/smoke_gate.py release" in s), steps.index(bundles)
        )

    def test_the_fast_path_never_builds_and_ships_the_gated_run(self):
        job = job_text(self.text, "ci-bundles")
        for building in ("tauri-apps/tauri-action@", "tauri build", "cargo ", "linux-tauri-deps"):
            self.assertNotIn(building, job)
        steps = job_steps(self.text, "ci-bundles")
        download = step_named(steps, "Download the bundles of the CI run the gate approved")
        self.assertIn('release_bundles.py download "$SHA" --run "$RUN_ID" --attempt "$RUN_ATTEMPT"', download)
        self.assertIn("RUN_ID: ${{ needs.verify.outputs.ci_run_id }}", job)
        self.assertIn("RUN_ATTEMPT: ${{ needs.verify.outputs.ci_run_attempt }}", job)
        self.assertIn("SHA: ${{ needs.verify.outputs.sha }}", job)
        sign = step_named(steps, "Sign the update bundles with the update key")
        self.assertIn('tauri signer sign --app-version "${TAG#v}" "$bundle" < /dev/null', sign)
        self.assertLess(steps.index(download), steps.index(sign))
        self.assertLess(index(steps, lambda s: "npm ci --ignore-scripts" in s), steps.index(sign))

    def test_every_job_after_the_gate_checks_out_the_gated_commit(self):
        for job in ("ci-bundles", "build", "checksums"):
            checkout = next(s for s in job_steps(self.text, job) if s.startswith("uses: actions/checkout@"))
            self.assertIn("ref: ${{ needs.verify.outputs.sha }}", checkout, job)

    def test_the_update_key_reaches_only_the_steps_that_sign(self):
        allowed = {
            ("verify", "The update signing key is configured"),
            ("build", "Export configured signing secrets"),
            ("ci-bundles", "Sign the update bundles with the update key"),
        }
        found = set()
        for job in ("verify", "create-release", "ci-bundles", "build", "checksums"):
            self.assertNotRegex(job_text(self.text, job).split("\n    steps:\n")[0], "TAURI_SIGNING", job)
            for step in job_steps(self.text, job):
                if "secrets.TAURI_SIGNING" in step:
                    found.add((job, re.match(r"(?:name: |uses: )(.+)", step).group(1)))
        self.assertEqual(found, allowed)


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
