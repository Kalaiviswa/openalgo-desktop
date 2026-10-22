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

   It needs `cargo-deny` and `cargo-audit`
   (`cargo install --locked cargo-deny cargo-audit`), `npm`, and for the last
   check the `gh` CLI signed in; without `gh` that check is skipped, not
   failed. It never tags, pushes or starts a workflow.
3. Optionally, the gate itself on GitHub, with nothing built or published:
   Actions > Release > Run workflow, with the tag name.
4. Tag and push (`version-bump` skill, "The tag is the maintainer's
   decision").

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
