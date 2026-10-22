#!/usr/bin/env python3
"""Tests of scripts/ci/release_bundles.py against fabricated GitHub answers.

    python3 scripts/ci/test_release_bundles.py

No network: every API path is answered from a temporary fixture folder
(SMOKE_GATE_FIXTURES, as for smoke_gate.py), downloads from zip files
written there, and a path with no fixture is refused.
"""

from __future__ import annotations

import hashlib
import io
import json
import os
import re
import shutil
import sys
import tempfile
import unittest
import zipfile
from contextlib import redirect_stdout

sys.dont_write_bytecode = True  # leave no __pycache__ in the checkout
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import release_bundles as rb  # noqa: E402
import smoke_gate  # noqa: E402
import updater_manifest  # noqa: E402

smoke_gate.LIST_PAUSE = 0

REPO = "marketcalls/openalgo-desktop"
REPO_ID = 1136999312
SHA = "6c804e51853ac878b95d82c622faa5bb49f0cd69"
OTHER_SHA = "aa73a473aec96ac273a5ed6a649f341a17e4d1f2"
VERSION = "1.2.3"
PRODUCT = "OpenAlgo Desktop"
RUNS = f"repos/{REPO}/actions/workflows/ci.yml/runs?branch=master&event=push&head_sha={SHA}&per_page=100"
NATIVE_RUNS = f"repos/{REPO}/actions/workflows/native-e2e-test-build.yml/runs?branch=master&head_sha={SHA}&per_page=100"

# What CI's tauri-build puts in each bundles-<platform> artifact, the
# installers a release does not ship included.
BUNDLES = {
    "windows-x86_64": [
        f"nsis/{PRODUCT}_{VERSION}_x64-setup.exe",
        f"msi/{PRODUCT}_{VERSION}_x64_en-US.msi",
    ],
    "macos-aarch64": [f"dmg/{PRODUCT}_{VERSION}_aarch64.dmg", f"macos/{PRODUCT}.app.tar.gz"],
    "macos-x86_64": [f"dmg/{PRODUCT}_{VERSION}_x64.dmg", f"macos/{PRODUCT}.app.tar.gz"],
    "linux-x86_64": [
        f"appimage/{PRODUCT}_{VERSION}_amd64.AppImage",
        f"deb/{PRODUCT}_{VERSION}_amd64.deb",
        f"rpm/{PRODUCT}-{VERSION}-1.x86_64.rpm",
    ],
    "linux-aarch64": [
        f"appimage/{PRODUCT}_{VERSION}_aarch64.AppImage",
        f"deb/{PRODUCT}_{VERSION}_arm64.deb",
        f"rpm/{PRODUCT}-{VERSION}-1.aarch64.rpm",
    ],
}
# What a release publishes, each file under the name GitHub gives it.
PUBLISHED = {
    "OpenAlgo.Desktop_1.2.3_x64-setup.exe",
    "OpenAlgo.Desktop_1.2.3_aarch64.dmg",
    "OpenAlgo.Desktop_1.2.3_aarch64.app.tar.gz",
    "OpenAlgo.Desktop_1.2.3_x64.dmg",
    "OpenAlgo.Desktop_1.2.3_x64.app.tar.gz",
    "OpenAlgo.Desktop_1.2.3_amd64.AppImage",
    "OpenAlgo.Desktop_1.2.3_amd64.deb",
    "OpenAlgo.Desktop_1.2.3_aarch64.AppImage",
    "OpenAlgo.Desktop_1.2.3_arm64.deb",
}
SIGNED = {
    "OpenAlgo.Desktop_1.2.3_x64-setup.exe",
    "OpenAlgo.Desktop_1.2.3_aarch64.app.tar.gz",
    "OpenAlgo.Desktop_1.2.3_x64.app.tar.gz",
    "OpenAlgo.Desktop_1.2.3_amd64.AppImage",
    "OpenAlgo.Desktop_1.2.3_amd64.deb",
    "OpenAlgo.Desktop_1.2.3_aarch64.AppImage",
    "OpenAlgo.Desktop_1.2.3_arm64.deb",
}


def fixture_name(path: str) -> str:
    return re.sub(r"[/?&=]", "_", path)


def ci_run(run_id=100, sha=SHA, attempt=1, **over):
    run = {
        "id": run_id,
        "path": ".github/workflows/ci.yml",
        "event": "push",
        "head_branch": "master",
        "head_repository": {"full_name": REPO},
        "head_sha": sha,
        "status": "completed",
        "conclusion": "success",
        "run_attempt": attempt,
        "created_at": "2026-10-20T01:00:00Z",
        "html_url": f"https://example.invalid/runs/{run_id}",
    }
    run.update(over)
    return run


def green_jobs():
    jobs = [{"name": n, "conclusion": "success"} for n in smoke_gate.EXPECTED_JOBS]
    jobs.append({"name": "ci-ok", "conclusion": "success"})
    for p in rb.PLATFORMS:
        steps = [{"name": "Set up job", "conclusion": "success"}]
        steps += [{"name": s, "conclusion": "success"} for s in rb.TAURI_BUILD_STEPS]
        jobs.append({"name": p.build_job, "conclusion": "success", "steps": steps})
    return jobs


def zip_of(files: dict[str, bytes]) -> bytes:
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as z:
        for name, data in files.items():
            z.writestr(name, data)
    return buf.getvalue()


def bundle_files(platform: str) -> dict[str, bytes]:
    return {name: f"{platform}:{name}".encode() for name in BUNDLES[platform]}


class Fixtures(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.dir)
        self.env = dict(os.environ)
        self.addCleanup(self.restore_env)
        os.environ["SMOKE_GATE_FIXTURES"] = os.path.join(self.dir, "api")
        os.environ["GITHUB_REPOSITORY"] = REPO
        os.environ.pop("GITHUB_OUTPUT", None)
        os.makedirs(os.environ["SMOKE_GATE_FIXTURES"])
        self.put(NATIVE_RUNS, {"workflow_runs": [ci_run(900, path=".github/workflows/native-e2e-test-build.yml")]})
        self.put(
            f"repos/{REPO}/actions/runs/900/jobs?per_page=100",
            {"jobs": [{"name": n, "conclusion": "success"} for n in smoke_gate.NATIVE_ORDER_JOBS]},
        )
        self.green()

    def restore_env(self):
        os.environ.clear()
        os.environ.update(self.env)

    def put(self, path, body):
        with open(os.path.join(os.environ["SMOKE_GATE_FIXTURES"], fixture_name(path) + ".json"), "w") as f:
            json.dump(body, f)

    def put_download(self, path, data: bytes):
        with open(os.path.join(os.environ["SMOKE_GATE_FIXTURES"], fixture_name(path) + ".bin"), "wb") as f:
            f.write(data)

    def green(self, run=None, jobs=None, artifacts=None, zips=None):
        """A green CI run 100 of SHA with every bundle, unless overridden."""
        run = run or ci_run()
        self.put(RUNS, {"workflow_runs": [run]})
        self.put(f"repos/{REPO}/actions/runs/{run['id']}", run)
        self.put(f"repos/{REPO}/actions/runs/{run['id']}/jobs?per_page=100", {"jobs": jobs or green_jobs()})
        zips = zips or {p.name: zip_of(bundle_files(p.name)) for p in rb.PLATFORMS}
        if artifacts is None:
            artifacts = []
            for i, p in enumerate(rb.PLATFORMS):
                data = zips[p.name]
                artifacts.append(self.artifact(p.name, 500 + i, data, run["id"]))
        for a in artifacts:
            if a["name"].removeprefix("bundles-") in zips:
                self.put_download(f"repos/{REPO}/actions/artifacts/{a['id']}/zip", zips[a["name"].removeprefix("bundles-")])
        self.put(f"repos/{REPO}/actions/runs/{run['id']}/artifacts?per_page=100", {"artifacts": artifacts})
        self.artifacts = artifacts

    def artifact(self, platform, artifact_id, data, run_id=100, **over):
        a = {
            "id": artifact_id,
            "name": f"bundles-{platform}",
            "size_in_bytes": len(data),
            "expired": False,
            "expires_at": "2026-10-27T01:00:00Z",
            "digest": "sha256:" + hashlib.sha256(data).hexdigest(),
            "workflow_run": {
                "id": run_id,
                "repository_id": REPO_ID,
                "head_repository_id": REPO_ID,
                "head_branch": "master",
                "head_sha": SHA,
            },
        }
        a.update(over)
        return a

    def run_cli(self, *argv):
        out = io.StringIO()
        with redirect_stdout(out):
            code = rb.main(list(argv))
        return code, out.getvalue()


class Check(Fixtures):
    def test_accepts_the_gated_run_and_names_it_for_the_next_job(self):
        output = os.path.join(self.dir, "github_output")
        os.environ["GITHUB_OUTPUT"] = output
        code, out = self.run_cli("check", SHA)
        self.assertEqual(code, 0, out)
        self.assertIn("kept until 2026-10-27T01:00:00Z", out)
        with open(output) as f:
            self.assertEqual(f.read(), "ci_run_id=100\nci_run_attempt=1\n")

    def test_refuses_what_the_install_smoke_gate_refuses(self):
        jobs = [j for j in green_jobs() if j["name"] != "install-smoke / macos (aarch64)"]
        self.green(jobs=jobs)
        code, out = self.run_cli("check", SHA)
        self.assertEqual(code, 1)
        self.assertIn("install-smoke / macos (aarch64): absent", out)

    def test_refuses_without_a_green_ci_ok(self):
        jobs = [dict(j, conclusion="failure") if j["name"] == "ci-ok" else j for j in green_jobs()]
        self.green(jobs=jobs)
        code, out = self.run_cli("check", SHA)
        self.assertEqual(code, 1)
        self.assertIn("ci-ok did not succeed", out)

    def test_refuses_a_build_without_its_checks_as_an_older_ci_ran_it(self):
        for missing in rb.TAURI_BUILD_STEPS:
            jobs = green_jobs()
            for j in jobs:
                if j["name"] == "tauri-build (linux-aarch64)":
                    j["steps"] = [s for s in j["steps"] if s["name"] != missing]
            self.green(jobs=jobs)
            code, out = self.run_cli("check", SHA)
            self.assertEqual(code, 1, missing)
            self.assertIn("tauri-build (linux-aarch64)", out)
            self.assertIn(repr(missing), out)
            self.assertIn("release with build: rebuild", out)

    def test_refuses_a_failed_check_step(self):
        jobs = green_jobs()
        for j in jobs:
            if j["name"] == "tauri-build (windows-x86_64)":
                for s in j["steps"]:
                    if s["name"] == "The bundled binary is the one checked before bundling":
                        s["conclusion"] = "failure"
        self.green(jobs=jobs)
        code, out = self.run_cli("check", SHA)
        self.assertEqual(code, 1)
        self.assertIn("The bundled binary is the one checked before bundling", out)

    def test_refuses_expired_bundles_and_says_to_rebuild(self):
        arts = [a if a["name"] != "bundles-macos-x86_64" else dict(a, expired=True) for a in self.artifacts]
        self.put(f"repos/{REPO}/actions/runs/100/artifacts?per_page=100", {"artifacts": arts})
        code, out = self.run_cli("check", SHA)
        self.assertEqual(code, 1)
        self.assertIn("bundles-macos-x86_64", out)
        self.assertIn("expired", out)
        self.assertIn("release with build: rebuild", out)

    def test_refuses_bundles_made_elsewhere(self):
        cases = {
            "another run": ({"id": 101}, "was made by run 101"),
            "another commit": ({"head_sha": OTHER_SHA}, f"made for master@{OTHER_SHA}"),
            "another branch": ({"head_branch": "feature"}, f"made for feature@{SHA}"),
            "a fork": ({"head_repository_id": 7}, "another repository"),
        }
        for what, (made, want) in cases.items():
            self.green()
            arts = []
            for a in self.artifacts:
                if a["name"] == "bundles-linux-x86_64":
                    a = dict(a, workflow_run=dict(a["workflow_run"], **made))
                arts.append(a)
            self.put(f"repos/{REPO}/actions/runs/100/artifacts?per_page=100", {"artifacts": arts})
            code, out = self.run_cli("check", SHA)
            self.assertEqual(code, 1, what)
            self.assertIn(want, out, what)

    def test_refuses_a_missing_doubled_or_undigested_artifact(self):
        self.green()
        first = self.artifacts[0]
        for arts, want in (
            (self.artifacts[1:], f"0 artifacts named {first['name']}"),
            (self.artifacts + [dict(first, id=999)], f"2 artifacts named {first['name']}"),
            ([dict(first, digest=None)] + self.artifacts[1:], "has no sha256 digest"),
            ([dict(first, size_in_bytes=0)] + self.artifacts[1:], "is empty"),
        ):
            self.put(f"repos/{REPO}/actions/runs/100/artifacts?per_page=100", {"artifacts": arts})
            code, out = self.run_cli("check", SHA)
            self.assertEqual(code, 1, want)
            self.assertIn(want, out)

    def test_refuses_a_short_sha(self):
        code, out = self.run_cli("check", SHA[:12])
        self.assertEqual(code, 1)
        self.assertIn("not a full commit sha", out)


class Download(Fixtures):
    def download(self, run_id=100, attempt=1):
        out = os.path.join(self.dir, "ci-bundles")
        return out, self.run_cli("download", SHA, "--run", str(run_id), "--attempt", str(attempt), "--out", out)

    def test_downloads_checks_and_unpacks_every_platform(self):
        out, (code, log) = self.download()
        self.assertEqual(code, 0, log)
        for p in rb.PLATFORMS:
            for name, data in bundle_files(p.name).items():
                with open(os.path.join(out, p.name, *name.split("/")), "rb") as f:
                    self.assertEqual(f.read(), data)
        self.assertEqual(sorted(os.listdir(out)), sorted(p.name for p in rb.PLATFORMS))

    def test_refuses_a_file_that_is_not_the_one_github_recorded(self):
        self.put_download(f"repos/{REPO}/actions/artifacts/502/zip", zip_of({"dmg/x.dmg": b"swapped"}))
        _out, (code, log) = self.download()
        self.assertEqual(code, 1)
        self.assertIn("GitHub recorded sha256:", log)

    def test_refuses_a_run_re_run_since_the_gate(self):
        self.green(run=ci_run(attempt=2))
        _out, (code, log) = self.download(attempt=1)
        self.assertEqual(code, 1)
        self.assertIn("since the gate, run 100 is at attempt 2, not the gated attempt 1", log)

    def test_refuses_a_run_that_is_not_ci_of_this_commit_on_master(self):
        for over in ({"head_sha": OTHER_SHA}, {"event": "pull_request"}, {"head_branch": "feature"}, {"path": ".github/workflows/x.yml"}):
            run = ci_run(**over)
            self.put(f"repos/{REPO}/actions/runs/100", run)
            _out, (code, log) = self.download()
            self.assertEqual(code, 1, over)
            self.assertIn("is not CI of a push of", log)

    def test_refuses_a_run_that_did_not_pass(self):
        self.put(f"repos/{REPO}/actions/runs/100", ci_run(conclusion="failure"))
        _out, (code, log) = self.download()
        self.assertEqual(code, 1)
        self.assertIn("completed/failure", log)

    def test_refuses_unsafe_entries(self):
        for name in ("../escape.exe", "/abs.exe", "nsis\\..\\x.exe", "C:/x.exe"):
            data = zip_of({name: b"x"})
            zips = {p.name: zip_of(bundle_files(p.name)) for p in rb.PLATFORMS}
            zips["windows-x86_64"] = data
            self.green(zips=zips)
            shutil.rmtree(os.path.join(self.dir, "ci-bundles"), ignore_errors=True)
            _out, (code, log) = self.download()
            self.assertEqual(code, 1, name)
            self.assertIn("unsafe entry", log)
        link = io.BytesIO()
        with zipfile.ZipFile(link, "w") as z:
            info = zipfile.ZipInfo("nsis/link.exe")
            info.external_attr = (0o120777 << 16)
            z.writestr(info, "/etc/passwd")
        zips = {p.name: zip_of(bundle_files(p.name)) for p in rb.PLATFORMS}
        zips["windows-x86_64"] = link.getvalue()
        self.green(zips=zips)
        _out, (code, log) = self.download()
        self.assertEqual(code, 1)
        self.assertIn("symbolic link", log)


class Stage(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.dir)
        self.src = os.path.join(self.dir, "ci-bundles")
        for platform, files in BUNDLES.items():
            for name in files:
                path = os.path.join(self.src, platform, *name.split("/"))
                os.makedirs(os.path.dirname(path), exist_ok=True)
                with open(path, "wb") as f:
                    f.write(f"{platform}:{name}".encode())
        self.config = os.path.join(self.dir, "tauri.conf.json")
        with open(self.config, "w") as f:
            json.dump({"productName": PRODUCT, "version": VERSION}, f)
        self.out = os.path.join(self.dir, "release")
        self.sign_list = os.path.join(self.dir, "to-sign.txt")

    def stage(self, version=VERSION):
        out = io.StringIO()
        with redirect_stdout(out):
            code = rb.main(
                ["stage", self.src, "--version", version, "--config", self.config, "--out", self.out, "--sign-list", self.sign_list]
            )
        return code, out.getvalue()

    def test_publishes_the_release_files_under_github_names_and_lists_the_update_bundles(self):
        code, log = self.stage()
        self.assertEqual(code, 0, log)
        self.assertEqual(set(os.listdir(self.out)), PUBLISHED)
        with open(self.sign_list) as f:
            listed = f.read().splitlines()
        self.assertEqual({os.path.basename(p) for p in listed}, SIGNED)
        self.assertEqual(len(listed), len(SIGNED))
        # The bytes are CI's, untouched: the macOS update bundle is renamed only.
        with open(os.path.join(self.out, "OpenAlgo.Desktop_1.2.3_x64.app.tar.gz"), "rb") as f:
            self.assertEqual(f.read(), f"macos-x86_64:macos/{PRODUCT}.app.tar.gz".encode())
        with open(os.path.join(self.out, "OpenAlgo.Desktop_1.2.3_arm64.deb"), "rb") as f:
            self.assertEqual(f.read(), f"linux-aarch64:deb/{PRODUCT}_{VERSION}_arm64.deb".encode())

    def test_the_update_manifest_finds_every_platform_once_signed(self):
        self.assertEqual(self.stage()[0], 0)
        with open(self.sign_list) as f:
            for path in f.read().splitlines():
                with open(path + ".sig", "w") as sig:
                    sig.write("x")
        self.assertEqual(sorted(updater_manifest.collect(self.out)), sorted(updater_manifest.PLATFORMS))

    def test_refuses_another_version(self):
        code, log = self.stage(version="1.2.4")
        self.assertEqual(code, 1)
        self.assertIn("is not OpenAlgo Desktop_1.2.4_", log)

    def test_refuses_a_missing_or_doubled_file(self):
        os.remove(os.path.join(self.src, "linux-x86_64", "deb", f"{PRODUCT}_{VERSION}_amd64.deb"))
        code, log = self.stage()
        self.assertEqual(code, 1)
        self.assertIn("0 files ending _amd64.deb", log)
        shutil.rmtree(self.out)
        with open(os.path.join(self.src, "linux-x86_64", "deb", f"{PRODUCT}_{VERSION}_amd64.deb"), "w") as f:
            f.write("a")
        with open(os.path.join(self.src, "linux-x86_64", "deb", f"{PRODUCT}_1.2.2_amd64.deb"), "w") as f:
            f.write("b")
        code, log = self.stage()
        self.assertEqual(code, 1)
        self.assertIn("2 files ending _amd64.deb", log)

    def test_refuses_a_signature_from_ci(self):
        with open(os.path.join(self.src, "macos-aarch64", "macos", f"{PRODUCT}.app.tar.gz.sig"), "w") as f:
            f.write("x")
        code, log = self.stage()
        self.assertEqual(code, 1)
        self.assertIn("must be unsigned", log)

    def test_refuses_a_folder_already_used(self):
        os.makedirs(self.out)
        with open(os.path.join(self.out, "left-over"), "w") as f:
            f.write("x")
        code, log = self.stage()
        self.assertEqual(code, 1)
        self.assertIn("is not empty", log)


class Matrices(unittest.TestCase):
    def test_the_platforms_are_the_release_matrix(self):
        root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..")
        with open(os.path.join(root, ".github", "workflows", "release.yml")) as f:
            release = f.read()
        with open(os.path.join(root, ".github", "workflows", "ci.yml")) as f:
            ci = f.read()
        for p in rb.PLATFORMS:
            self.assertIn(f"\n          - name: {p.name}\n", release)
            self.assertIn(f"\n          - name: {p.name}\n", ci)
            self.assertIn(f"bundles-${{{{ matrix.name }}}}", ci)
        self.assertEqual(len(re.findall(r"\n          - name: ", release)), len(rb.PLATFORMS))


if __name__ == "__main__":
    unittest.main(verbosity=2)
