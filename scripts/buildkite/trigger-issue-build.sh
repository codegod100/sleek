#!/usr/bin/env bash
# Trigger a Buildkite build on main for a Radicle issue.
#
# The Garden Buildkite adapter only auto-triggers push/patch events and skips issue
# COB commits (no .buildkite/pipeline.yml at that commit). Call this from a Garden
# webhook, cron, or manually after opening an issue.
#
# For a full poll of pending issues (schedule-equivalent), use instead:
#   ./scripts/buildkite/dispatch-poll.sh
#   # or Buildkite UI New Build with env RADICLE_TRIGGER=poll
#
# Usage:
#   BUILDKITE_API_TOKEN=... \
#   BUILDKITE_ORG=nandi \
#   BUILDKITE_PIPELINE=sleek \
#     ./scripts/buildkite/trigger-issue-build.sh 2ac9c51b5df8d3b67fb0a40c738e073e9ff301be
#   # also: ./scripts/buildkite/dispatch-poll.sh --issue <issue-id>
#
# Optional:
#   BUILDKITE_BRANCH=main
#   BUILDKITE_API_URL=https://api.buildkite.com/v2
set -euo pipefail

ISSUE_ID="${1:-}"
if [[ -z "$ISSUE_ID" ]]; then
  echo "usage: $0 <issue-id>" >&2
  exit 2
fi

ORG="${BUILDKITE_ORG:-nandi}"
PIPELINE="${BUILDKITE_PIPELINE:-sleek}"
TOKEN="${BUILDKITE_API_TOKEN:-}"
BRANCH="${BUILDKITE_BRANCH:-main}"
API="${BUILDKITE_API_URL:-https://api.buildkite.com/v2}"

[[ -n "$TOKEN" ]] || { echo "BUILDKITE_API_TOKEN required" >&2; exit 1; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

bk_require_cmd git
bk_require_cmd curl
bk_require_cmd jq

# Prefer validating via rad when available; otherwise accept the id (poll already
# filtered to xyz.radicle.issue COB refs).
if command -v rad >/dev/null 2>&1 && git remote get-url rad >/dev/null 2>&1; then
  if ! rad issue show "$ISSUE_ID" --header >/dev/null 2>&1; then
    bk_die "not a valid issue: $ISSUE_ID"
  fi
fi

COMMIT="$(git rev-parse "$BRANCH")"
MESSAGE="Radicle issue opened: ${ISSUE_ID:0:7}"

payload="$(jq -n \
  --arg commit "$COMMIT" \
  --arg branch "$BRANCH" \
  --arg message "$MESSAGE" \
  --arg issue_id "$ISSUE_ID" \
  '{
    commit: $commit,
    branch: $branch,
    message: $message,
    ignore_pipeline_branch_filters: true,
    env: {
      RADICLE_ISSUE_ID: $issue_id,
      RADICLE_TRIGGER: "issue"
    }
  }')"

url="${API%/}/organizations/${ORG}/pipelines/${PIPELINE}/builds"
echo "POST $url"
echo "  branch=$BRANCH commit=${COMMIT:0:7} issue=${ISSUE_ID:0:7}"

response="$(curl -fsSL -X POST "$url" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d "$payload")"

build_url="$(echo "$response" | jq -r '.web_url // .url // empty')"
build_number="$(echo "$response" | jq -r '.number // empty')"

if [[ -n "$build_url" ]]; then
  echo "build #$build_number: $build_url"
else
  echo "$response" | jq .
fi
