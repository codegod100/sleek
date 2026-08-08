#!/usr/bin/env bash
# Trigger boxci issue agent for one Radicle issue COB id.
#
# Usage:
#   ./scripts/boxci/trigger-issue.sh <issue-id>
#   ./scripts/boxci/trigger-issue.sh --dry-run <issue-id>
#
# Env:
#   BOXCI_URL          — default https://boxci.boxd.sh
#   SLEEK_BRANCH       — default main
#   SLEEK_REPO_URL     — Garden clone URL
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

# shellcheck source=../buildkite/lib.sh
source "$SCRIPT_DIR/../buildkite/lib.sh"

usage() {
  sed -n '2,12p' "$0"
  exit 2
}

DRY_RUN=0
ISSUE_ID=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    -h | --help)
      usage
      ;;
    *)
      if [[ -z "$ISSUE_ID" && "$1" =~ ^[0-9a-fA-F]{7,40}$ ]]; then
        ISSUE_ID="$1"
        shift
      else
        echo "trigger-issue: unknown arg: $1" >&2
        usage
      fi
      ;;
  esac
done

[[ -n "$ISSUE_ID" ]] || usage

BOXCI_URL="${BOXCI_URL:-https://boxci.boxd.sh}"
BRANCH="${SLEEK_BRANCH:-main}"
REPO_URL="${SLEEK_REPO_URL:-https://nandi.radicle.garden/z9mjPzpVK472QXaaP1picc5U9xBR.git}"
REPO_ID="${SLEEK_REPO_ID:-rad:z9mjPzpVK472QXaaP1picc5U9xBR}"

bk_require_cmd curl
bk_require_cmd jq

if ! bk_issue_cob_exists "$ISSUE_ID"; then
  bk_die "not a valid issue: $ISSUE_ID"
fi

payload="$(jq -n \
  --arg repo_url "$REPO_URL" \
  --arg repo "$REPO_ID" \
  --arg trigger "issue" \
  --arg branch "$BRANCH" \
  --arg issue_id "$ISSUE_ID" \
  --argjson dry_run "$DRY_RUN" \
  '{
    repo_url: $repo_url,
    repo: $repo,
    trigger: $trigger,
    branch: $branch,
    issue_id: $issue_id,
    dry_run: $dry_run
  }')"

url="${BOXCI_URL%/}/api/runs/from-repo"
echo "POST $url"
echo "  trigger=issue issue=${ISSUE_ID:0:7} branch=$BRANCH"

response="$(curl -fsSL -X POST "$url" \
  -H "Content-Type: application/json" \
  -d "$payload")"

run_id="$(echo "$response" | jq -r '.id // empty')"
status="$(echo "$response" | jq -r '.status // empty')"

if [[ -n "$run_id" ]]; then
  echo "run $run_id: $status"
  echo "$response" | jq '{id, status, pipeline, issue_id, env: {BOXCI_TRIGGER, RADICLE_ISSUE_ID}}'
else
  echo "$response" | jq .
fi
