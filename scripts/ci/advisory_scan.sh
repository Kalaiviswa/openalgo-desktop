#!/usr/bin/env bash
# The dependency advisory scans that gate a release, in one place.
#
#   scripts/ci/advisory_scan.sh cargo-deny | cargo-audit | npm-audit | all
#   scripts/ci/advisory_scan.sh accepted      the accepted RUSTSEC ids
#
# Three callers run exactly these commands, so a scan cannot pass in one
# and fail in another:
#   - release.yml's gate, on the tagged commit;
#   - security.yml, every day on master and on any dependency change;
#   - scripts/release/preflight.sh, on the maintainer's machine before
#     tagging.
#
#   cargo-deny   advisories, bans, licenses and sources per .github/deny.toml
#   cargo-audit  any vulnerability in src-tauri/Cargo.lock, except the
#                advisories accepted in .github/deny.toml (every RUSTSEC id
#                on a line that is not a comment: its [advisories] ignore)
#   npm-audit    a high or critical advisory in the production dependencies
#                (--omit=dev: the build tools never ship in the installer)
#
# cargo-deny and cargo-audit fetch today's RustSec database and npm audit
# asks the registry, so a commit that passed yesterday can fail today: an
# advisory published since is the point. `all` runs the three and fails if
# any failed. Needs cargo-deny and cargo-audit
# (cargo install --locked cargo-deny cargo-audit) and npm.
set -euo pipefail
cd "$(dirname "$0")/../.."

accepted() {
  grep -vE '^[[:space:]]*#' .github/deny.toml | grep -oE 'RUSTSEC-[0-9]{4}-[0-9]{4}' | sort -u || true
}

cargo_deny() {
  cargo deny --manifest-path src-tauri/Cargo.toml --config .github/deny.toml --locked \
    check advisories bans licenses sources
}

cargo_audit() {
  local id
  local ignore_args=()
  while IFS= read -r id; do
    ignore_args+=(--ignore "$id")
  done < <(accepted)
  echo "Advisories accepted in deny.toml: $(( ${#ignore_args[@]} / 2 ))"
  # The empty-array form keeps bash 3.2 (macOS) happy under set -u.
  cargo audit --file src-tauri/Cargo.lock ${ignore_args[@]+"${ignore_args[@]}"}
}

npm_audit() {
  npm audit --omit=dev --audit-level=high
}

case "${1:-}" in
  cargo-deny) cargo_deny ;;
  cargo-audit) cargo_audit ;;
  npm-audit) npm_audit ;;
  accepted) accepted ;;
  all)
    failed=()
    for scan in cargo-deny cargo-audit npm-audit; do
      echo "== ${scan}"
      if "${scan//-/_}"; then
        echo "-- ${scan}: passed"
      else
        echo "-- ${scan}: FAILED"
        failed+=("$scan")
      fi
    done
    if [ "${#failed[@]}" -ne 0 ]; then
      echo "Failed: ${failed[*]}"
      exit 1
    fi
    ;;
  *)
    sed -n '2,5p' "$0"
    exit 2
    ;;
esac
