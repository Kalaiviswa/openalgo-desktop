#!/usr/bin/env bash
# Names the commit whose bundles an installed-app job tests, in the log and
# the job summary (.github/workflows/install-smoke.yml, native-e2e.yml).
# Those jobs run inside the CI run that built the bundles, so the commit is
# that run's: on a push, GITHUB_SHA; on a pull request, the head of the pull
# request (HEAD_SHA), built merged into master as GITHUB_SHA.
set -euo pipefail
head="${HEAD_SHA:-$GITHUB_SHA}"
if [ "$head" = "$GITHUB_SHA" ]; then
  what="commit \`${head}\`"
else
  what="pull request head \`${head}\`, tested merged into its base as \`${GITHUB_SHA}\`"
fi
line="Bundles built by CI run ${GITHUB_RUN_ID} (${GITHUB_EVENT_NAME}) from ${what}."
echo "$line"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  printf '### Commit under test\n\n%s\n' "$line" >> "$GITHUB_STEP_SUMMARY"
fi
