#!/usr/bin/env bash
# Before tagging a release: the checks release.yml's gate makes, made here
# first, so a tag is pushed only when the gate will pass.
#
#   scripts/release/preflight.sh [v<version>]
#
# Run it in a checkout of the commit you will tag (normally the newest
# commit on master, once its CI finished green), with no uncommitted
# changes. The tag defaults to v<the version in src-tauri/tauri.conf.json>.
# Every check runs, then the failed ones are listed:
#
#   clean checkout  no uncommitted change: the checks read the working tree
#   versions        the tag is v<version> in every file that carries it
#                   (.claude/skills/version-bump/check_versions.py --tag)
#   on master       the commit is on origin/master (after a fetch)
#   advisories      cargo-deny, cargo-audit and npm audit with today's
#                   databases: scripts/ci/advisory_scan.sh, the very commands
#                   the gate runs, with the advisories .github/deny.toml
#                   accepts
#   CI on GitHub    with the gh CLI signed in, read-only: the commit's
#                   install smoke and native order journey on master
#                   (scripts/ci/smoke_gate.py, the gate's own check)
#   CI's bundles    the same way: the bundles a tag push will sign and
#                   attach are there, from that CI run, not expired
#                   (scripts/ci/release_bundles.py check); if they are
#                   not, release with Actions > Release > Run workflow,
#                   build: rebuild
#
# It never tags, pushes or starts a workflow. Needs git, python3, npm and
# cargo with cargo-deny and cargo-audit
# (cargo install --locked cargo-deny cargo-audit); gh is optional.
# docs/release/release-workflow.md says when to run it.
set -euo pipefail
cd "$(dirname "$0")/../.."

version=$(python3 -c 'import json; print(json.load(open("src-tauri/tauri.conf.json"))["version"])')
tag="${1:-v${version}}"
sha=$(git rev-parse HEAD)
failed=()

check() {
  local name="$1"
  shift
  echo
  echo "== ${name}"
  if "$@"; then
    echo "-- ${name}: passed"
  else
    echo "-- ${name}: FAILED"
    failed+=("$name")
  fi
}

# The functions below run as an `if` condition, where set -e does not
# apply: each returns its failure itself.
clean_checkout() {
  if ! git diff --quiet HEAD --; then
    echo "Uncommitted changes: the scans would read files the tag does not carry."
    git status --short --untracked-files=no
    return 1
  fi
  echo "Checking ${sha} for ${tag}."
}

on_master() {
  git fetch --quiet origin master || return 1
  if ! git merge-base --is-ancestor "$sha" FETCH_HEAD; then
    echo "${sha} is not on origin/master. Tag only a commit on master whose CI is green."
    return 1
  fi
  echo "${sha} is on origin/master."
}

# Runs a gate script against GitHub, read-only, when gh is signed in.
on_github() {
  if ! command -v gh >/dev/null 2>&1 || ! gh auth status >/dev/null 2>&1; then
    echo "Skipped: the gh CLI is not installed or not signed in."
    return 0
  fi
  local repo
  repo=$(gh repo view --json nameWithOwner --jq .nameWithOwner) || return 1
  GITHUB_REPOSITORY="$repo" python3 "$@"
}

check "clean checkout" clean_checkout
check "versions" python3 .claude/skills/version-bump/check_versions.py --tag "$tag"
check "on master" on_master
check "advisories" scripts/ci/advisory_scan.sh all
check "CI on GitHub" on_github scripts/ci/smoke_gate.py release "$sha"
check "CI's bundles" on_github scripts/ci/release_bundles.py check "$sha"

echo
if [ "${#failed[@]}" -ne 0 ]; then
  echo "Not ready to tag ${tag}. Failed: ${failed[*]}."
  exit 1
fi
echo "Ready: ${sha} passes the gate's checks for ${tag}. Tagging is the maintainer's step:"
echo "  git tag -a ${tag} -m \"OpenAlgo Desktop ${tag#v}\" ${sha}"
echo "  git push origin ${tag}"
