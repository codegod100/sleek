#!/usr/bin/env bash
# Trigger boxci check build from sleek's repo-local .boxci/pipeline.yml.
#
# Usage:
#   ./scripts/boxci/dispatch-check.sh
#   ./scripts/boxci/dispatch-check.sh --sha abc1234
#   ./scripts/boxci/dispatch-check.sh --sha abc1234 --wait
#
# Env:
#   BOXCI_URL          — default https://boxci.boxd.sh
#   SLEEK_BRANCH       — default main
#   SLEEK_REPO_URL     — Garden clone URL (default below)
#   SLEEK_GITHUB_REPO_URL — default https://github.com/codegod100/sleek.git
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

usage() {
  sed -n '2,14p' "$0"
  exit 2
}

SHA=""
WAIT=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --sha)
      [[ $# -ge 2 ]] || usage
      SHA="$2"
      shift 2
      ;;
    --wait) WAIT=1; shift ;;
    -h | --help)
      usage
      ;;
    *)
      if [[ -z "$SHA" && "$1" =~ ^[0-9a-fA-F]{7,40}$ ]]; then
        SHA="$1"
        shift
      else
        echo "dispatch-check: unknown arg: $1" >&2
        usage
      fi
      ;;
  esac
done

BOXCI_URL="${BOXCI_URL:-https://boxci.boxd.sh}"
BRANCH="${SLEEK_BRANCH:-main}"
REPO_URL="${SLEEK_REPO_URL:-https://nandi.radicle.garden/z9mjPzpVK472QXaaP1picc5U9xBR.git}"
REPO_ID="${SLEEK_REPO_ID:-rad:z9mjPzpVK472QXaaP1picc5U9xBR}"
GITHUB_REPO_URL="${SLEEK_GITHUB_REPO_URL:-https://github.com/codegod100/sleek.git}"

command -v curl >/dev/null 2>&1 || { echo "curl required" >&2; exit 1; }
command -v jq >/dev/null 2>&1 || { echo "jq required" >&2; exit 1; }

if [[ -z "$SHA" ]]; then
  command -v git >/dev/null 2>&1 || { echo "git required (or pass --sha)" >&2; exit 1; }
  SHA="$(git rev-parse HEAD)"
fi

payload="$(jq -n \
  --arg repo_url "$REPO_URL" \
  --arg repo "$REPO_ID" \
  --arg trigger "check" \
  --arg sha "$SHA" \
  --arg branch "$BRANCH" \
  --arg github_repo_url "$GITHUB_REPO_URL" \
  '{
    repo_url: $repo_url,
    repo: $repo,
    trigger: $trigger,
    sha: $sha,
    branch: $branch,
    github_repo_url: $github_repo_url
  }')"

url="${BOXCI_URL%/}/api/runs/from-repo"
echo "POST $url"
echo "  trigger=check GIT_SHA=${SHA:0:7} branch=$BRANCH repo=$REPO_URL"

response="$(curl -fsSL -X POST "$url" \
  -H "Content-Type: application/json" \
  -d "$payload")"

run_id="$(echo "$response" | jq -r '.id // empty')"
status="$(echo "$response" | jq -r '.status // empty')"
pipeline="$(echo "$response" | jq -r '.pipeline // empty')"

if [[ -z "$run_id" ]]; then
  echo "$response" | jq .
  exit 1
fi

echo "run $run_id: $status"
[[ -n "$pipeline" ]] && echo "  pipeline=$pipeline"
echo "  url=${BOXCI_URL%/}/runs/${run_id}"
echo "$response" | jq '{id, status, pipeline, duration_s, steps: [.steps[]? | {key, label, status}]}'

if [[ "$WAIT" -eq 1 ]]; then
  echo "dispatch-check: waiting for terminal status…" >&2
  for _ in $(seq 1 480); do
    resp="$(curl -fsSL "${BOXCI_URL%/}/api/runs/${run_id}")"
    status="$(echo "$resp" | jq -r '.status // empty')"
    case "$status" in
      passed|failed|error|cancelled)
        echo "$resp" | jq '{id, status, pipeline, duration_s, steps: [.steps[]? | {key, label, status, exit_code}]}'
        [[ "$status" == "passed" ]]
        exit $?
        ;;
      *) sleep 15 ;;
    esac
  done
  echo "dispatch-check: timed out waiting for $run_id" >&2
  exit 1
fi
