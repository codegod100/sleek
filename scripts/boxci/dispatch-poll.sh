#!/usr/bin/env bash
# Manually dispatch a Radicle issue-poll boxci run (same as POST /api/poll).
#
# Usage:
#   ./scripts/boxci/dispatch-poll.sh
#   ./scripts/boxci/dispatch-poll.sh --issue <issue-id>   # skip poll; trigger one issue
#   ./scripts/boxci/dispatch-poll.sh --dry-run
#
# Env:
#   BOXCI_URL          — default https://boxci.boxd.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

usage() {
  sed -n '2,12p' "$0"
  exit 2
}

ISSUE_ID=""
DRY_RUN=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --issue)
      [[ $# -ge 2 ]] || usage
      ISSUE_ID="$2"
      shift 2
      ;;
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
        echo "dispatch-poll: unknown arg: $1" >&2
        usage
      fi
      ;;
  esac
done

if [[ -n "$ISSUE_ID" ]]; then
  args=()
  [[ "$DRY_RUN" == "1" ]] && args+=(--dry-run)
  exec "$SCRIPT_DIR/trigger-issue.sh" "${args[@]}" "$ISSUE_ID"
fi

BOXCI_URL="${BOXCI_URL:-https://boxci.boxd.sh}"
REPO_URL="${SLEEK_REPO_URL:-https://nandi.radicle.garden/z9mjPzpVK472QXaaP1picc5U9xBR.git}"
REPO_ID="${SLEEK_REPO_ID:-rad:z9mjPzpVK472QXaaP1picc5U9xBR}"

command -v curl >/dev/null 2>&1 || { echo "curl required" >&2; exit 1; }
command -v jq >/dev/null 2>&1 || { echo "jq required" >&2; exit 1; }

payload="$(jq -n \
  --arg repo_url "$REPO_URL" \
  --arg repo "$REPO_ID" \
  --argjson dry_run "$DRY_RUN" \
  '{
    repo_url: $repo_url,
    repo: $repo,
    dry_run: $dry_run
  }')"

url="${BOXCI_URL%/}/api/poll"
echo "POST $url"
echo "  trigger=poll repo=$REPO_URL"

response="$(curl -fsSL -X POST "$url" \
  -H "Content-Type: application/json" \
  -d "$payload")"

run_id="$(echo "$response" | jq -r '.id // empty')"
status="$(echo "$response" | jq -r '.status // empty')"

if [[ -n "$run_id" ]]; then
  echo "run $run_id: $status"
  echo "$response" | jq '{id, status, pipeline, steps: [.steps[]? | {key, label, status}]}'
else
  echo "$response" | jq .
fi
