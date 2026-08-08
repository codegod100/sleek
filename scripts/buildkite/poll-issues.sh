#!/usr/bin/env bash
# Poll for new open Radicle issues and trigger Buildkite agent builds.
# Run from a scheduled Buildkite pipeline on main (e.g. every 5 minutes).
#
# Requires:
#   BUILDKITE_ORG, BUILDKITE_PIPELINE, BUILDKITE_API_TOKEN
# Optional:
#   RADICLE_ISSUE_POLL_STATE_FILE — path to last-seen issue ids (default: .buildkite/.issue-poll-state)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

bk_require_cmd rad
bk_require_rad_repo

STATE_FILE="${RADICLE_ISSUE_POLL_STATE_FILE:-$REPO_ROOT/.buildkite/.issue-poll-state}"
mkdir -p "$(dirname "$STATE_FILE")"
touch "$STATE_FILE"

mapfile -t known < "$STATE_FILE"

current=()
while IFS= read -r line; do
  [[ -n "$line" ]] && current+=("$line")
done < <(rad cob list --type xyz.radicle.issue 2>/dev/null || true)

new_ids=()
for id in "${current[@]}"; do
  found=0
  for old in "${known[@]}"; do
    if [[ "$id" == "$old" ]]; then
      found=1
      break
    fi
  done
  if [[ "$found" -eq 0 ]]; then
    new_ids+=("$id")
  fi
done

printf '%s\n' "${current[@]}" >"$STATE_FILE"

if [[ "${#new_ids[@]}" -eq 0 ]]; then
  echo "poll: no new issues"
  exit 0
fi

echo "poll: ${#new_ids[@]} new issue(s)"
for id in "${new_ids[@]}"; do
  echo "  triggering build for $id"
  "$SCRIPT_DIR/trigger-issue-build.sh" "$id"
done
