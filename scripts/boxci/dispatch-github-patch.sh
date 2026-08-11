#!/usr/bin/env bash
# Ask boxci to open a Radicle patch from a GitHub commit (sleek).
#
# Usage:
#   ./scripts/boxci/dispatch-github-patch.sh
#   ./scripts/boxci/dispatch-github-patch.sh --sha abc1234
#   ./scripts/boxci/dispatch-github-patch.sh --sha abc1234 --title "…" --description "…"
#   ./scripts/boxci/dispatch-github-patch.sh --dry-run
#
# Env:
#   BOXCI_URL              — default https://boxci.boxd.sh
#   BOXCI_WEBHOOK_SECRET   — optional X-Boxci-Secret header
#   SLEEK_REPO_ID          — default rad:z9mjPzpVK472QXaaP1picc5U9xBR
#   SLEEK_GITHUB_REPO_URL  — default https://github.com/codegod100/sleek.git
#   SLEEK_BRANCH           — Radicle base branch (default main)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

usage() {
  sed -n '2,16p' "$0"
  exit 2
}

SHA=""
TITLE=""
DESCRIPTION=""
DRY_RUN=0
SYNC=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --sha)
      [[ $# -ge 2 ]] || usage
      SHA="$2"
      shift 2
      ;;
    --title)
      [[ $# -ge 2 ]] || usage
      TITLE="$2"
      shift 2
      ;;
    --description)
      [[ $# -ge 2 ]] || usage
      DESCRIPTION="$2"
      shift 2
      ;;
    --dry-run) DRY_RUN=1; shift ;;
    --sync) SYNC=1; shift ;;
    -h | --help) usage ;;
    *)
      if [[ -z "$SHA" && "$1" =~ ^[0-9a-fA-F]{7,40}$ ]]; then
        SHA="$1"
        shift
      else
        echo "dispatch-github-patch: unknown arg: $1" >&2
        usage
      fi
      ;;
  esac
done

BOXCI_URL="${BOXCI_URL:-https://boxci.boxd.sh}"
REPO_ID="${SLEEK_REPO_ID:-rad:z9mjPzpVK472QXaaP1picc5U9xBR}"
GITHUB_REPO_URL="${SLEEK_GITHUB_REPO_URL:-https://github.com/codegod100/sleek.git}"
BRANCH="${SLEEK_BRANCH:-main}"

command -v curl >/dev/null 2>&1 || { echo "curl required" >&2; exit 1; }
command -v jq >/dev/null 2>&1 || { echo "jq required" >&2; exit 1; }

if [[ -z "$SHA" ]]; then
  command -v git >/dev/null 2>&1 || { echo "git required (or pass --sha)" >&2; exit 1; }
  SHA="$(git rev-parse HEAD)"
fi

if [[ -z "$TITLE" ]] && command -v git >/dev/null 2>&1; then
  TITLE="$(git log -1 --format=%s "$SHA" 2>/dev/null || true)"
fi
if [[ -z "$DESCRIPTION" ]] && command -v git >/dev/null 2>&1; then
  DESCRIPTION="$(git log -1 --format=%b "$SHA" 2>/dev/null || true)"
fi

# git push -o patch.message=… rejects embedded newlines; flatten to one line.
TITLE="$(printf '%s' "$TITLE" | tr '\n' ' ' | sed 's/  */ /g; s/^ //; s/ $//')"
DESCRIPTION="$(printf '%s' "$DESCRIPTION" | tr '\n' ' ' | sed 's/  */ /g; s/^ //; s/ $//')"

payload="$(jq -n \
  --arg repo "$REPO_ID" \
  --arg github_repo_url "$GITHUB_REPO_URL" \
  --arg github_commit "$SHA" \
  --arg branch "$BRANCH" \
  --arg title "$TITLE" \
  --arg description "$DESCRIPTION" \
  --argjson dry_run "$DRY_RUN" \
  --argjson sync "$SYNC" \
  '{
    repo: $repo,
    github_repo_url: $github_repo_url,
    github_commit: $github_commit,
    branch: $branch,
    dry_run: $dry_run,
    sync: $sync
  }
  + (if $title != "" then {title: $title} else {} end)
  + (if $description != "" then {description: $description} else {} end)')"

echo "dispatch-github-patch: repo=$REPO_ID commit=${SHA:0:12} → $BOXCI_URL" >&2

hdr=(-H "Content-Type: application/json")
if [[ -n "${BOXCI_WEBHOOK_SECRET:-}" ]]; then
  hdr+=(-H "X-Boxci-Secret: ${BOXCI_WEBHOOK_SECRET}")
fi

resp="$(curl -sS -X POST "${BOXCI_URL%/}/api/patches/from-github" \
  "${hdr[@]}" \
  -d "$payload")"
echo "$resp" | jq .
run_id="$(echo "$resp" | jq -r '.id // empty')"
if [[ -n "$run_id" ]]; then
  echo "dispatch-github-patch: run https://${BOXCI_URL#https://}/runs/${run_id}" >&2
  echo "dispatch-github-patch: poll  ${BOXCI_URL%/}/api/runs/${run_id}" >&2
fi
