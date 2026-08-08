#!/usr/bin/env bash
# Trigger boxci sleek-merge pipeline (APK + Flatpak after merge to main).
#
# Usage:
#   ./scripts/boxci/dispatch-merge.sh
#   ./scripts/boxci/dispatch-merge.sh --sha abc1234
#
# Env:
#   BOXCI_URL          — default https://boxci.boxd.sh
#   SLEEK_BRANCH       — default main
#   SLEEK_REPO_URL     — passed to boxci (default: Radicle Garden clone URL)
#   OPENBAO_TOKEN      — optional; not required for trigger itself
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

usage() {
  sed -n '2,12p' "$0"
  exit 2
}

SHA=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --sha)
      [[ $# -ge 2 ]] || usage
      SHA="$2"
      shift 2
      ;;
    -h | --help)
      usage
      ;;
    *)
      if [[ -z "$SHA" && "$1" =~ ^[0-9a-fA-F]{7,40}$ ]]; then
        SHA="$1"
        shift
      else
        echo "dispatch-merge: unknown arg: $1" >&2
        usage
      fi
      ;;
  esac
done

BOXCI_URL="${BOXCI_URL:-https://boxci.boxd.sh}"
BRANCH="${SLEEK_BRANCH:-main}"
REPO_URL="${SLEEK_REPO_URL:-https://nandi.radicle.garden/z9mjPzpVK472QXaaP1picc5U9xBR.git}"

command -v curl >/dev/null 2>&1 || { echo "curl required" >&2; exit 1; }
command -v jq >/dev/null 2>&1 || { echo "jq required" >&2; exit 1; }

if [[ -z "$SHA" ]]; then
  command -v git >/dev/null 2>&1 || { echo "git required (or pass --sha)" >&2; exit 1; }
  SHA="$(git rev-parse "$BRANCH")"
fi

payload="$(jq -n \
  --arg pipeline "sleek-merge.yml" \
  --arg trigger "merge" \
  --arg sha "$SHA" \
  --arg branch "$BRANCH" \
  --arg repo_url "$REPO_URL" \
  '{
    pipeline: $pipeline,
    env: {
      BOXCI_TRIGGER: $trigger,
      GIT_SHA: $sha,
      SLEEK_BRANCH: $branch,
      SLEEK_REPO_URL: $repo_url
    }
  }')"

url="${BOXCI_URL%/}/api/runs"
echo "POST $url"
echo "  pipeline=sleek-merge.yml BOXCI_TRIGGER=merge GIT_SHA=${SHA:0:7} branch=$BRANCH"

response="$(curl -fsSL -X POST "$url" \
  -H "Content-Type: application/json" \
  -d "$payload")"

run_id="$(echo "$response" | jq -r '.id // empty')"
status="$(echo "$response" | jq -r '.status // empty')"

if [[ -n "$run_id" ]]; then
  echo "run $run_id: $status"
  echo "$response" | jq '{id, status, pipeline, duration_s, steps: [.steps[] | {key, label, status}]}'
else
  echo "$response" | jq .
fi
