# Cutting a release: before the tag and after it

This page is for the maintainer. It says what to check before pushing a `v*`
tag, so the release gate does not stop the release after the tag, and what
the release workflow (`.github/workflows/release.yml`) then does. The gate's
rules are in [Branch and tag protection](branch-protection.md); what the
release produces for in-app updates is in [In-app updates](updates.md).

## Before tagging

1. The commit to tag is on `master` and its CI run for the push to master
   finished green: `ci-ok`, the install smoke and the native E2E inside it,
   and the native order journey (`native-e2e-test-build.yml`) of the same
   commit. A run cancelled because the pull request already tested the
   commit has no verdict: re-run it and wait.
2. In a checkout of that commit with no uncommitted changes, run:

   ```bash
   scripts/release/preflight.sh            # or: scripts/release/preflight.sh v1.0.1
   ```

   It runs every check and then lists the ones that failed:

   | Check | What it runs | Same as the gate's step |
   | --- | --- | --- |
   | clean checkout | `git diff --quiet HEAD` | (the gate checks out the tag) |
   | versions | `.claude/skills/version-bump/check_versions.py --tag <tag>` | "The tag equals the app version everywhere" |
   | on master | `git fetch origin master`, then `git merge-base --is-ancestor` | "The tag points at a commit on master" |
   | advisories | `scripts/ci/advisory_scan.sh all`: cargo-deny, cargo-audit and npm audit against today's databases | the three advisory steps, which run the same script |
   | CI on GitHub | `scripts/ci/smoke_gate.py release <sha>` through `gh`, read-only | "Install smoke and the native order journey passed" |
   | CI's bundles | `scripts/ci/release_bundles.py check <sha>` through `gh`, read-only | "CI's tested bundles of this commit are there to sign" |

   It needs `cargo-deny` and `cargo-audit`
   (`cargo install --locked cargo-deny cargo-audit`), `npm`, and for the last
   two checks the `gh` CLI signed in; without `gh` they are skipped, not
   failed. It never tags, pushes or starts a workflow.
3. Optionally, the gate itself on GitHub, with nothing built or published:
   Actions > Release > Run workflow, from `master`, with the tag name and
   `build: none` (the default).
4. Tag and push (`version-bump` skill, "The tag is the maintainer's
   decision"), within a week of that CI run: CI keeps its bundles 7 days.

The advisory scans are the step most likely to change between a green CI run
and the tag: they read the RustSec database and the npm registry as they are
that day. On 2026-10-22 an advisory published the same day stopped the 1.0.0
release at the gate twice, each time after a two-hour CI run. The preflight
shows it before the tag; the daily scan below shows it the morning it
appears.

## Advisories are scanned every day

`.github/workflows/security.yml` runs every day at 02:00 UTC (07:30 IST) on
`master`, besides on any change to a lockfile, a manifest or the scanning
configuration. Its `cargo-deny`, `cargo-audit` and `npm audit` jobs run
`scripts/ci/advisory_scan.sh`, the script the release gate and the preflight
run, so a red daily scan is the release gate failing early. GitHub mails a
failed scheduled run to whoever last changed the schedule in that file.

When one fails, before the next release:

- Update the dependency if a fixed version exists (a lockfile change, which
  runs the scan again on its pull request).
- Otherwise accept it only when the vulnerable code provably never runs
  (`CLAUDE.md`, "Security work"): an `ignore` with the reason and a review
  date in `.github/deny.toml`, the same id in `.trivyignore`, and the
  Dependabot alert dismissed with the same reason. `advisory_scan.sh` reads
  the accepted ids from `deny.toml`, so the gate, the daily scan and the
  preflight accept the same list.

Either way the fix is a commit, and a release needs that commit's CI, so
fixing it the day the scan turns red keeps it off release day.

## What a tag push does

The release ships the installers CI built for the tagged commit, the very
files its install smoke installed, started and signed in with. It does not
build them again. Times are estimates from the runs of 2026-10-22.

| Step | Job | About |
| --- | --- | --- |
| The gate: tag on master, `ci-ok`, install smoke and native order journey, CI's bundles, versions, advisory scans, the update key | `verify` | 2 to 3 min |
| Create (or reuse) the draft release | `create-release` | under 1 min |
| Download the bundles of the CI run the gate approved, check each against the sha256 GitHub recorded, pick the release files, sign the 7 update bundles, attach everything and compare each attached file with its source | `ci-bundles` | 4 to 7 min |
| Download the draft's assets, verify every update signature against the app's key, attach `latest.json`, `SHA256SUMS.txt` and the code-signing notes | `checksums` | 2 to 4 min |

From the tag to a draft ready for review: about 10 to 15 minutes. The
rebuild took about 45 to 50 minutes for its build step alone, Windows being
the slowest, plus the same gate and `checksums`.

The files on the draft, per platform: the NSIS `-setup.exe` (Windows), the
`.dmg` and the `.app.tar.gz` update bundle (macOS Apple Silicon and Intel),
the AppImage and the `.deb` (Linux x64 and arm64), and a `.sig` beside each
update bundle and `.deb`.

### What each guarantee rests on

- **The exact commit.** `verify` resolves the tag to its commit and requires
  it on `master`. The CI run must be a push of that commit to `master` in
  this repository (`smoke_gate.py`), and every bundle artifact must be
  recorded by GitHub as made by that run, of that commit, on `master`
  (`release_bundles.py check`). `ci-bundles` reads the run again by its id
  and requires the same attempt, so a re-run after the gate is refused, and
  checks out the tagged commit for the product name, scripts and key.
- **Tests passed on that commit.** As before: the newest `ci-ok` check run
  on the commit, and the CI run's install smoke and native E2E jobs and the
  native order journey all succeeded. Now also that run's own `ci-ok` job
  and each `tauri-build` job, with the check before bundling, its backstop
  and the unsigned update bundle among its steps. The files attached are
  the ones that run's install smoke downloaded: the same artifacts, each
  download checked against the sha256 GitHub recorded when CI uploaded it,
  copied unchanged (only the macOS update bundle is renamed), and each
  attached file compared by size and, where GitHub reports it, sha256.
- **The E2E test build stays out.** CI's `tauri-build` now runs the release
  build's check before bundling (`release-checks.conf.json`) and its
  backstop (`check_no_e2e_build.py --same-as-checked`), and the gate
  requires both steps to have succeeded in the run whose files ship. The
  install smoke checks the installed binaries too, as before.
- **Signatures against the app's key.** CI never holds the update key (a
  test pins that `ci.yml` never names it); `ci-bundles` signs with
  `tauri signer sign --app-version <version>`, and `checksums` refuses any
  signature that does not verify with `minisign` against
  `plugins.updater.pubkey` in the tagged `tauri.conf.json`, or that names
  another version.
- **`latest.json`.** Unchanged: `scripts/ci/updater_manifest.py` builds it
  from the draft's assets as published, one update bundle per platform.
- **A draft, published by a human.** Unchanged: the workflow only creates
  or reuses a draft; publishing it is what makes the update visible.

## The rebuild, by hand

Actions > Release > Run workflow, **Use workflow from: `master`**, the tag,
and `build: rebuild`. It runs the gate without the bundles check, then
builds every platform again with the update key, as every release did
before (the `build` job), then `checksums`. Use it when:

- CI's bundles have expired (more than 7 days after the CI run) or the CI
  run predates CI making the update bundles (the gate says
  "release with build: rebuild");
- Apple or Windows code signing is configured: the signature goes inside
  the app at build time, and the gate refuses to ship CI's unsigned
  installers while those secrets exist.

The rebuilt installers are not the files the install smoke tested, as was
the case for every release before this path existed. `build: ci-bundles`
runs the default path by hand, for instance after a failed attempt; a draft
an earlier run created is reused, and its files are replaced.

## If the release stops

| Message | Meaning and fix |
| --- | --- |
| `no CI run of <sha> on master has a verdict` | CI has not finished, or its master run was cancelled: re-run it and wait. |
| `... has no successful step [...] release with build: rebuild` | The CI run predates the update bundles, or a check failed in it: rebuild. |
| `... expired on ... release with build: rebuild` | The bundles are gone: rebuild. |
| `run <id> is at attempt <n>, not the gated attempt` | The CI run was re-run after the gate passed: run the release again (`build: ci-bundles`). |
| `Apple or Windows code signing is configured` | Rebuild. |
| An advisory step failed | See "Advisories are scanned every day" above. |

