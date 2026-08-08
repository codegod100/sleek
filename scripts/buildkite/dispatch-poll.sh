#!/usr/bin/env bash
# Manually dispatch a Radicle issue-poll Buildkite build (same as the schedule).
#
# The 10m Buildkite schedule already sets RADICLE_TRIGGER=poll. Use this when you
# want an immediate poll without waiting for the next tick.
#
# Buildkite UI (no script needed):
#   https://buildkite.com/nandi/sleek → New Build
#   Branch: main (or tip you want)
#   Environment variables: RADICLE_TRIGGER=poll
#   Create Build → runs radicle-issue-poll → poll-issues.sh
#
# CLI:
#   ./scripts/buildkite/dispatch-poll.sh
#   # loads BUILDKITE_API_TOKEN from OpenBao when OPENBAO_TOKEN is set:
#   #   eval "$(./scripts/configure-buildkite-from-openbao.sh)"
#   # or set BUILDKITE_API_TOKEN yourself.
#
# Specific issue (skip poll; run agent for one id):
#   ./scripts/buildkite/dispatch-poll.sh --issue <issue-id>
#   # same as: ./scripts/buildkite/trigger-issue-build.sh <issue-id>
#
# Optional env:
#   BUILDKITE_ORG=nandi
#   BUILDKITE_PIPELINE=sleek
#   BUILDKITE_BRANCH=main
#   BUILDKITE_API_URL=https://api.buildkite.com/v2
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

usage() {
  sed -n '2,28p' "$0"
  exit 2
}

ISSUE_ID=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --issue)
      [[ $# -ge 2 ]] || usage
      ISSUE_ID="$2"
      shift 2
      ;;
    -h | --help)
      usage
      ;;
    *)
      # Positional issue id → same as --issue (discoverability).
      if [[ -z "$ISSUE_ID" && "$1" =~ ^[0-9a-fA-F]{7,40}$ ]]; then
        ISSUE_ID="$1"
        shift
      else
        echo "dispatch-poll: unknown arg: $1" >&2
        usage
      fi
      ;;
  esac
done

ORG="${BUILDKITE_ORG:-nandi}"
PIPELINE="${BUILDKITE_PIPELINE:-sleek}"
BRANCH="${BUILDKITE_BRANCH:-main}"
API="${BUILDKITE_API_URL:-https://api.buildkite.com/v2}"

# Soft-load API token from OpenBao when unset (local/manual use).
if [[ -z "${BUILDKITE_API_TOKEN:-}" ]]; then
  if [[ -n "${OPENBAO_TOKEN:-}" && -x "$REPO_ROOT/scripts/configure-buildkite-from-openbao.sh" ]]; then
    # shellcheck disable=SC1091
    eval "$("$REPO_ROOT/scripts/configure-buildkite-from-openbao.sh")"
  fi
fi

TOKEN="${BUILDKITE_API_TOKEN:-}"
[[ -n "$TOKEN" ]] || {
  echo "BUILDKITE_API_TOKEN required (or set OPENBAO_TOKEN and use configure-buildkite-from-openbao.sh)" >&2
  exit 1
}
export BUILDKITE_API_TOKEN="$TOKEN"

if [[ -n "$ISSUE_ID" ]]; then
  exec "$SCRIPT_DIR/trigger-issue-build.sh" "$ISSUE_ID"
fi

bk_require_cmd git
bk_require_cmd curl
bk_require_cmd jq

COMMIT="$(git rev-parse "$BRANCH")"
MESSAGE="Manual Radicle issue poll (RADICLE_TRIGGER=poll)"

payload="$(jq -n \
  --arg commit "$COMMIT" \
  --arg branch "$BRANCH" \
  --arg message "$MESSAGE" \
  '{
    commit: $commit,
    branch: $branch,
    message: $message,
    ignore_pipeline_branch_filters: true,
    env: {
      RADICLE_TRIGGER: "poll"
    }
  }')"

url="${API%/}/organizations/${ORG}/pipelines/${PIPELINE}/builds"
echo "POST $url"
echo "  branch=$BRANCH commit=${COMMIT:0:7} RADICLE_TRIGGER=poll"

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
