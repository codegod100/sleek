#!/usr/bin/env bash
# Ask boxci to merge a Radicle patch into main (CI must be a repo delegate).
#
# Usage:
#   ./scripts/boxci/dispatch-merge-patch.sh <patch-id>
#   ./scripts/boxci/dispatch-merge-patch.sh --patch <patch-id>
#   ./scripts/boxci/dispatch-merge-patch.sh --patch <patch-id> --dry-run
#   ./scripts/boxci/dispatch-merge-patch.sh --patch <patch-id> --sync
#
# Env:
#   BOXCI_URL              — default https://boxci.boxd.sh
#   BOXCI_WEBHOOK_SECRET   — optional X-Boxci-Secret header
#   SLEEK_REPO_ID          — default rad:z9mjPzpVK472QXaaP1picc5U9xBR
#   SLEEK_BRANCH           — default main
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

usage() {
  sed -n '2,16p' "$0"
  exit 2
}

PATCH_ID=""
DRY_RUN=0
SYNC=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --patch)
      [[ $# -ge 2 ]] || usage
      PATCH_ID="$2"
      shift 2
      ;;
    --dry-run) DRY_RUN=1; shift ;;
    --sync) SYNC=1; shift ;;
    -h | --help) usage ;;
    *)
      if [[ -z "$PATCH_ID" && "$1" =~ ^[0-9a-fA-F]{7,40}$ ]]; then
        PATCH_ID="$1"
        shift
      else
        echo "dispatch-merge-patch: unknown arg: $1" >&2
        usage
      fi
      ;;
  esac
done

[[ -n "$PATCH_ID" ]] || usage

BOXCI_URL="${BOXCI_URL:-https://boxci.boxd.sh}"
REPO_ID="${SLEEK_REPO_ID:-rad:z9mjPzpVK472QXaaP1picc5U9xBR}"
BRANCH="${SLEEK_BRANCH:-main}"

command -v curl >/dev/null 2>&1 || { echo "curl required" >&2; exit 1; }
command -v jq >/dev/null 2>&1 || { echo "jq required" >&2; exit 1; }

payload="$(jq -n \
  --arg repo "$REPO_ID" \
  --arg patch_id "$PATCH_ID" \
  --arg branch "$BRANCH" \
  --argjson dry_run "$DRY_RUN" \
  --argjson sync "$SYNC" \
  '{
    repo: $repo,
    patch_id: $patch_id,
    branch: $branch,
    dry_run: $dry_run,
    sync: $sync
  }')"

echo "dispatch-merge-patch: repo=$REPO_ID patch=${PATCH_ID:0:12} → $BOXCI_URL" >&2

hdr=(-H "Content-Type: application/json")
if [[ -n "${BOXCI_WEBHOOK_SECRET:-}" ]]; then
  hdr+=(-H "X-Boxci-Secret: ${BOXCI_WEBHOOK_SECRET}")
fi

# Show identity tip once so operators know the delegate DID.
curl -fsS "${BOXCI_URL%/}/api/identity" 2>/dev/null | jq -c '.identity | {did,nid,alias}' >&2 || true

resp="$(curl -sS -X POST "${BOXCI_URL%/}/api/patches/merge" \
  "${hdr[@]}" \
  -d "$payload")"
echo "$resp" | jq .
run_id="$(echo "$resp" | jq -r '.id // empty')"
if [[ -n "$run_id" ]]; then
  echo "dispatch-merge-patch: run https://${BOXCI_URL#https://}/runs/${run_id}" >&2
  echo "dispatch-merge-patch: poll  ${BOXCI_URL%/}/api/runs/${run_id}" >&2
fi
