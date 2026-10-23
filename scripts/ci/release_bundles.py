#!/usr/bin/env python3
"""The bundles CI built and tested for a commit, which its release ships.

    python3 scripts/ci/release_bundles.py check <sha>
    python3 scripts/ci/release_bundles.py download <sha> --run ID --attempt N --out DIR
    python3 scripts/ci/release_bundles.py stage DIR --version 1.2.3 \\
        --config src-tauri/tauri.conf.json --out DIR --sign-list FILE

A tag's release attaches the installers CI built for the tagged commit and
the install smoke installed, instead of building them again
(docs/release/release-workflow.md). They come from the run the release gate
already relies on (smoke_gate.verdict): the newest run of ci.yml for a push
of the commit to master that was not cancelled, completed with success and
every installed-app job green.

check     refuses unless, in that run's attempt, ci-ok succeeded, every
          tauri-build job succeeded with each step of TAURI_BUILD_STEPS (the
          build with its check before bundling, the backstop that the
          bundled binary is the one checked, the unsigned update bundle, the
          upload), and for each platform exactly one bundles-<platform>
          artifact was created while that platform's tauri-build job of the
          attempt ran, which GitHub records as made by this run, of this
          commit, on master, in this repository, not expired, with a sha256
          digest. The run id and commit alone do not tie an artifact to the
          job whose steps were checked: a later job of the same run can
          upload one of the same name (upload-artifact's `overwrite`), and
          after "Re-run all jobs" GitHub lists one per attempt. It writes
          the run's id and attempt to GITHUB_OUTPUT (ci_run_id,
          ci_run_attempt).
download  reads the run the gate approved by its id and checks all of the
          above again, at the same attempt (a re-run since the gate is a run
          nobody gated; a newer push run of the same commit would need
          master rewritten, which the `master history` ruleset forbids),
          downloads each artifact, refuses one whose sha256 is not GitHub's
          digest, unpacks it under DIR/<platform>/, and checks the run once
          more at the end.
stage     copies the files a release publishes (PLATFORMS) out of DIR into
          --out under the names GitHub publishes them with, and writes the
          update bundles to sign to --sign-list, one path per line. It
          refuses a missing or doubled file, a name without this version,
          and any .sig: CI never holds the update key.

It reads the GitHub API through `gh api` (GH_TOKEN, GITHUB_REPOSITORY), as
smoke_gate.py does; SMOKE_GATE_FIXTURES answers it from files instead, and a
download from the same name with ".bin" (tests:
scripts/ci/test_release_bundles.py).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import zipfile
from dataclasses import dataclass
from datetime import datetime

sys.dont_write_bytecode = True  # leave no __pycache__ in the checkout
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import smoke_gate  # noqa: E402
from smoke_gate import Refused  # noqa: E402

CI = next(g for g in smoke_gate.GATES if g.workflow == "ci.yml")
OK_JOB = "ci-ok"
# The steps of ci.yml's tauri-build job, by name, that must have succeeded
# in the run whose bundles ship. Keep in step with ci.yml
# (test_release_workflow.py checks the names exist there).
TAURI_BUILD_STEPS = (
    "Build bundles (no publishing)",
    "The bundled binary is the one checked before bundling",
    "The update bundle is built, unsigned",
    "Upload bundles",
)
DIGEST = re.compile(r"^sha256:[0-9a-f]{64}$")
PUBLISHED_NAME = re.compile(r"^[A-Za-z0-9._-]+$")


@dataclass(frozen=True)
class Shipped:
    folder: str  # under target/<triple>/release/bundle/, as in the artifact
    suffix: str  # how the file's name ends
    update: bool  # an in-app update bundle, signed with the update key
    # A macOS update bundle is named after the product alone; the release
    # names it as tauri-action always did: <product>_<version><published>.
    published: str | None = None


@dataclass(frozen=True)
class Platform:
    name: str  # the matrix name in ci.yml and release.yml
    files: tuple[Shipped, ...]

    @property
    def artifact(self) -> str:
        return f"bundles-{self.name}"

    @property
    def build_job(self) -> str:
        return f"tauri-build ({self.name})"


# What each platform's release carries, as the rebuild in release.yml
# (`bundles` of its matrix) makes it.
PLATFORMS = (
    Platform("windows-x86_64", (Shipped("nsis", "_x64-setup.exe", True),)),
    Platform(
        "macos-aarch64",
        (Shipped("dmg", "_aarch64.dmg", False), Shipped("macos", ".app.tar.gz", True, "_aarch64.app.tar.gz")),
    ),
    Platform(
        "macos-x86_64",
        (Shipped("dmg", "_x64.dmg", False), Shipped("macos", ".app.tar.gz", True, "_x64.app.tar.gz")),
    ),
    Platform("linux-x86_64", (Shipped("appimage", "_amd64.AppImage", True), Shipped("deb", "_amd64.deb", True))),
    Platform("linux-aarch64", (Shipped("appimage", "_aarch64.AppImage", True), Shipped("deb", "_arm64.deb", True))),
)


def fetch(path: str, dest: str) -> None:
    """Download an API path's body (a redirect is followed) to `dest`."""
    fixtures = os.environ.get("SMOKE_GATE_FIXTURES")
    if fixtures:
        src = os.path.join(fixtures, re.sub(r"[/?&=]", "_", path) + ".bin")
        if not os.path.isfile(src):
            raise Refused(f"no such download: {path}")
        shutil.copyfile(src, dest)
        return
    with open(dest, "wb") as f:
        out = subprocess.run(["gh", "api", path], stdout=f, stderr=subprocess.PIPE)
    if out.returncode != 0:
        raise Refused(f"GitHub API {path}: {out.stderr.decode(errors='replace').strip()[:200]}")


def sha256_of(path: str) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def moment(value: object) -> datetime | None:
    """A GitHub timestamp (2026-10-22T20:53:15Z), or None."""
    try:
        when = datetime.fromisoformat(str(value).replace("Z", "+00:00"))
    except ValueError:
        return None
    return when if when.tzinfo else None


def attempt_jobs(r: str, run: dict) -> list[dict]:
    """The jobs of the run's attempt as read with the run, each with its
    steps and times. A job a re-run of failed jobs kept is listed under the
    new attempt with the times it ran."""
    path = f"repos/{r}/actions/runs/{run['id']}/attempts/{run.get('run_attempt')}/jobs?per_page=100"
    return smoke_gate.api(path).get("jobs", [])


def tested_build(run: dict, jobs: list[dict]) -> None:
    """The run's ci-ok and every platform's build, with its checks, passed."""
    url = run.get("html_url")
    by_name = {j.get("name"): j for j in jobs}
    if (by_name.get(OK_JOB) or {}).get("conclusion") != "success":
        raise Refused(f"ci-ok did not succeed in {url}")
    for p in PLATFORMS:
        job = by_name.get(p.build_job) or {}
        if job.get("conclusion") != "success":
            raise Refused(f"{p.build_job} is {job.get('conclusion', 'absent')} in {url}")
        steps = {s.get("name"): s.get("conclusion") for s in job.get("steps") or []}
        missing = [name for name in TAURI_BUILD_STEPS if steps.get(name) != "success"]
        if missing:
            raise Refused(
                f"{p.build_job} in {url} has no successful step {missing}: these bundles were not "
                "built to be signed (CI before release.yml shipped its bundles?); release with build: rebuild"
            )


def built_by(artifact: dict, run: dict, sha: str) -> str | None:
    """Why `artifact` is not this run's bundle of `sha`, or None."""
    made = artifact.get("workflow_run") or {}
    if artifact.get("expired"):
        return f"expired on {artifact.get('expires_at')} (CI keeps bundles 7 days); release with build: rebuild"
    if made.get("id") != run["id"]:
        return f"was made by run {made.get('id')}, not {run['id']}"
    if made.get("head_sha") != sha or made.get("head_branch") != smoke_gate.DEFAULT_BRANCH:
        return f"was made for {made.get('head_branch')}@{made.get('head_sha')}"
    if made.get("repository_id") is None or made.get("head_repository_id") != made.get("repository_id"):
        return "was made from another repository"
    if not DIGEST.match(artifact.get("digest") or ""):
        return f"has no sha256 digest ({artifact.get('digest')!r})"
    if not artifact.get("size_in_bytes"):
        return "is empty"
    return None


def made_by_build(found: list[dict], job: dict, run: dict) -> dict:
    """The one of `found` (the run's artifacts of one name) created while
    `job`, the platform's tauri-build of the run's attempt, ran."""
    url, attempt = run.get("html_url"), run.get("run_attempt")
    name, build = found[0].get("name"), job.get("name")
    start, end = moment(job.get("started_at")), moment(job.get("completed_at"))
    if start is None or end is None:
        raise Refused(f"{build} of attempt {attempt} in {url} has no start or end time")
    during = [a for a in found if (made := moment(a.get("created_at"))) is not None and start <= made <= end]
    if not during:
        created = ", ".join(str(a.get("created_at")) for a in found)
        raise Refused(
            f"no {name} of {url} was made by {build} of attempt {attempt}, which ran from "
            f"{job.get('started_at')} to {job.get('completed_at')}: created at {created}, so replaced "
            "after that build or made by another attempt"
        )
    if len(during) != 1:
        raise Refused(f"{len(during)} artifacts named {name} in {url} were made during {build} of attempt {attempt}")
    return during[0]


def bundles_of(r: str, sha: str, run: dict) -> dict[str, dict]:
    """The bundle artifact per platform that the run's attempt built, once
    its build passed; Refused when any is not as `check` requires."""
    jobs = attempt_jobs(r, run)
    tested_build(run, jobs)
    by_name = {j.get("name"): j for j in jobs}
    listed = smoke_gate.api(f"repos/{r}/actions/runs/{run['id']}/artifacts?per_page=100").get("artifacts", [])
    artifacts: dict[str, dict] = {}
    for p in PLATFORMS:
        found = [a for a in listed if a.get("name") == p.artifact]
        if not found:
            raise Refused(f"0 artifacts named {p.artifact} in {run.get('html_url')}")
        chosen = made_by_build(found, by_name[p.build_job], run)
        why = built_by(chosen, run, sha)
        if why:
            raise Refused(f"{p.artifact} of {run.get('html_url')} {why}")
        artifacts[p.name] = chosen
    return artifacts


def approved(r: str, sha: str) -> tuple[dict, dict[str, dict]]:
    """The CI run whose bundles a release of `sha` ships (the one the
    install smoke gate decides by), and its bundles."""
    run, _jobs = smoke_gate.verdict(CI, r, sha)
    return run, bundles_of(r, sha, run)


def gated(r: str, sha: str, run_id: int, attempt: int, when: str) -> tuple[dict, dict[str, dict]]:
    """The run the gate approved, read by its id, still as it was then."""
    run = smoke_gate.api(f"repos/{r}/actions/runs/{run_id}")
    if not smoke_gate.master_run_of(run, r, sha, CI):
        raise Refused(f"run {run_id} is not CI of a push of {sha} to {smoke_gate.DEFAULT_BRANCH} in {r}")
    if run.get("run_attempt") != attempt:
        raise Refused(
            f"{when}, run {run_id} is at attempt {run.get('run_attempt')}, not the gated attempt {attempt}: "
            "run the release again"
        )
    smoke_gate.finished(CI, run, sha)
    jobs = smoke_gate.run_jobs(r, run_id)
    smoke_gate.jobs_passed(CI, run, jobs, sha)
    return run, bundles_of(r, sha, run)


def output(**values: object) -> None:
    path = os.environ.get("GITHUB_OUTPUT")
    if path:
        with open(path, "a", encoding="utf-8") as f:
            for k, v in values.items():
                f.write(f"{k}={v}\n")


def check(r: str, sha: str) -> int:
    run, artifacts = approved(r, sha)
    until = min(a.get("expires_at") or "" for a in artifacts.values())
    print(
        f"CI's bundles of {sha}: {run.get('html_url')} (attempt {run.get('run_attempt')}), "
        f"tested and kept until {until}: {', '.join(a['name'] for a in artifacts.values())}"
    )
    output(ci_run_id=run["id"], ci_run_attempt=run.get("run_attempt"))
    return 0


def unpack(archive: str, dest: str) -> None:
    """Unpack a zip of plain files under `dest`, refusing any other entry."""
    with zipfile.ZipFile(archive) as z:
        for info in z.infolist():
            name = info.filename
            parts = name.split("/")
            if name.startswith("/") or "\\" in name or ".." in parts or ":" in parts[0]:
                raise Refused(f"unsafe entry {name!r} in {os.path.basename(archive)}")
            if info.is_dir():
                continue
            if stat.S_ISLNK(info.external_attr >> 16):
                raise Refused(f"symbolic link {name!r} in {os.path.basename(archive)}")
            target = os.path.join(dest, *parts)
            os.makedirs(os.path.dirname(target), exist_ok=True)
            with z.open(info) as src, open(target, "wb") as out:
                shutil.copyfileobj(src, out, 1 << 20)


def download(r: str, sha: str, run_id: int, attempt: int, out: str) -> int:
    _run, artifacts = gated(r, sha, run_id, attempt, "since the gate")
    os.makedirs(out, exist_ok=True)
    for platform, a in artifacts.items():
        archive = os.path.join(out, f"{a['name']}.zip")
        fetch(f"repos/{r}/actions/artifacts/{a['id']}/zip", archive)
        got = "sha256:" + sha256_of(archive)
        if got != a["digest"]:
            raise Refused(f"{a['name']} downloaded as {got}, GitHub recorded {a['digest']}")
        unpack(archive, os.path.join(out, platform))
        os.remove(archive)
        print(f"{a['name']}: {a['digest']} from run {run_id}")
    _run, now = gated(r, sha, run_id, attempt, "during the download")
    changed = [p for p in artifacts if (now[p]["id"], now[p]["digest"]) != (artifacts[p]["id"], artifacts[p]["digest"])]
    if changed:
        raise Refused(f"the bundles of {changed} changed during the download: run the release again")
    return 0


def product_name(config: str) -> str:
    with open(config, encoding="utf-8") as f:
        return json.load(f)["productName"]


def choose(folder: str, shipped: Shipped, product: str, version: str) -> tuple[str, str]:
    """The one file `shipped` names in `folder`, and its published name."""
    where = os.path.join(folder, shipped.folder)
    names = sorted(n for n in os.listdir(where) if n.endswith(shipped.suffix)) if os.path.isdir(where) else []
    if len(names) != 1:
        raise Refused(f"{len(names)} files ending {shipped.suffix} in {where}: {names}")
    name = names[0]
    if shipped.published:
        expected, published = f"{product}{shipped.suffix}", f"{product}_{version}{shipped.published}"
    else:
        expected, published = f"{product}_{version}{shipped.suffix}", name
    if name != expected:
        raise Refused(f"{where}/{name} is not {expected}: built from another version or product")
    published = published.replace(" ", ".")
    if not PUBLISHED_NAME.match(published):
        raise Refused(f"{published!r} is not a name GitHub keeps as it is")
    return os.path.join(where, name), published


def stage(src: str, version: str, config: str, out: str, sign_list: str) -> int:
    product = product_name(config)
    if os.path.exists(out) and os.listdir(out):
        raise Refused(f"{out} is not empty")
    os.makedirs(out, exist_ok=True)
    to_sign: list[str] = []
    for p in PLATFORMS:
        folder = os.path.join(src, p.name)
        for root, _dirs, files in os.walk(folder):
            signed = [f for f in files if f.endswith(".sig")]
            if signed:
                raise Refused(f"{root} carries {signed}: CI's bundles must be unsigned")
        for shipped in p.files:
            path, published = choose(folder, shipped, product, version)
            dest = os.path.join(out, published)
            if os.path.exists(dest):
                raise Refused(f"two files would be published as {published}")
            shutil.copyfile(path, dest)
            if shipped.update:
                to_sign.append(dest)
            print(f"{p.name}\t{published}\tsha256:{sha256_of(dest)}")
    with open(sign_list, "w", encoding="utf-8") as f:
        f.write("".join(f"{path}\n" for path in to_sign))
    return 0


def main(argv: list[str]) -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = p.add_subparsers(dest="command", required=True)
    c = sub.add_parser("check")
    c.add_argument("sha")
    d = sub.add_parser("download")
    d.add_argument("sha")
    d.add_argument("--run", type=int, required=True)
    d.add_argument("--attempt", type=int, required=True)
    d.add_argument("--out", required=True)
    s = sub.add_parser("stage")
    s.add_argument("src")
    s.add_argument("--version", required=True)
    s.add_argument("--config", required=True)
    s.add_argument("--out", required=True)
    s.add_argument("--sign-list", required=True)
    args = p.parse_args(argv)
    try:
        if args.command == "stage":
            return stage(args.src, args.version, args.config, args.out, args.sign_list)
        if not smoke_gate.SHA.match(args.sha):
            raise Refused(f"not a full commit sha: {args.sha!r}")
        r = smoke_gate.repo()
        if args.command == "check":
            return check(r, args.sha)
        return download(r, args.sha, args.run, args.attempt, args.out)
    except Refused as e:
        print(f"::error title=Release bundles::{e}")
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
