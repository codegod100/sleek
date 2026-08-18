#!/usr/bin/env bash
# Translate a Radicle CI broker webhook payload into a boxci build via Garden webhooks.
#
# Routes issue COB roots to /api/webhooks/garden/issue; merge/patch to /api/webhooks/garden.
#
# Example:
#   echo "$payload" | ./scripts/boxci/webhook-to-boxci.sh
#
# Env:
#   BOXCI_URL          — default https://boxci.boxd.sh
#   RAD_REPO_PATH      — optional checkout for issue-COB filtering
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../buildkite/lib.sh
source "$SCRIPT_DIR/../buildkite/lib.sh"

bk_require_cmd jq
bk_require_cmd curl

payload="$(cat)"
commit="$(echo "$payload" | jq -r '.commit // .Commit // .head // .after // empty')"
repo="$(echo "$payload" | jq -r '.repo // .repo_id // .repository.id // empty')"
branch="$(echo "$payload" | jq -r '.branch // .ref // .refs/heads // .repository.default_branch // empty' | sed 's|^refs/heads/||; s|.*/refs/heads/||')"

if [[ -z "$commit" ]]; then
  # Patch / branch-delete Garden events — boxci ignores these too.
  if echo "$payload" | jq -e '.patch // .deleted // empty' >/dev/null 2>&1; then
    echo "webhook-to-boxci: patch/delete event — ignoring" >&2
    exit 0
  fi
  echo "webhook-to-boxci: no commit in payload — ignoring" >&2
  exit 0
fi

echo "webhook-to-boxci: repo=${repo:-unknown} branch=${branch:-unknown} commit=${commit:0:7}"

BOXCI_URL="${BOXCI_URL:-https://boxci.boxd.sh}"

if [[ -n "$branch" && "$branch" != "main" ]]; then
  echo "webhook-to-boxci: not main — ignoring" >&2
  exit 0
fi

export RAD_HOME="${RAD_HOME:-$HOME/.radicle}"
if command -v rad >/dev/null 2>&1; then
  if [[ -n "${RAD_REPO_PATH:-}" && -d "$RAD_REPO_PATH" ]]; then
    cd "$RAD_REPO_PATH"
  fi
  if bk_commit_is_new_issue "$commit" 2>/dev/null; then
    echo "webhook-to-boxci: issue COB root — routing to /api/webhooks/garden/issue"
    url="${BOXCI_URL%/}/api/webhooks/garden/issue"
    echo "POST $url"
    response="$(curl -fsSL -X POST "$url" \
      -H "Content-Type: application/json" \
      -d "$payload")"
    echo "$response" | jq .
    exit 0
  fi
fi

url="${BOXCI_URL%/}/api/webhooks/garden"
echo "POST $url"
response="$(curl -fsSL -X POST "$url" \
  -H "Content-Type: application/json" \
  -d "$payload")"

echo "$response" | jq .
