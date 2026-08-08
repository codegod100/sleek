#!/usr/bin/env bash
# Translate a Radicle CI broker push/patch webhook payload into a boxci merge build
# (APK + Flatpak on main) via POST /api/webhooks/garden.
#
# Intended for the Garden webhooks adapter when you cannot register boxci directly.
# Prefer registering https://boxci.boxd.sh/api/webhooks/garden on Garden instead.
#
# Example (Garden after merge to main):
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

payload="$(cat)"
commit="$(echo "$payload" | jq -r '.commit // .Commit // .head // empty')"
repo="$(echo "$payload" | jq -r '.repo // .repository // .repo_id // empty')"
branch="$(echo "$payload" | jq -r '.branch // .ref // .refs/heads // empty' | sed 's|^refs/heads/||')"

if [[ -z "$commit" ]]; then
  echo "webhook-to-boxci: no commit in payload — ignoring" >&2
  exit 0
fi

echo "webhook-to-boxci: repo=${repo:-unknown} branch=${branch:-unknown} commit=${commit:0:7}"

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
    echo "webhook-to-boxci: issue COB root — ignoring (use webhook-to-buildkite.sh)" >&2
    exit 0
  fi
fi

BOXCI_URL="${BOXCI_URL:-https://boxci.boxd.sh}"
url="${BOXCI_URL%/}/api/webhooks/garden"

# Re-post the broker payload; boxci resolves repo URL and runs .boxci/pipeline.yml.
echo "POST $url"
response="$(curl -fsSL -X POST "$url" \
  -H "Content-Type: application/json" \
  -d "$payload")"

echo "$response" | jq .
