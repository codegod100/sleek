#!/usr/bin/env bash
# Poll for Radicle issue COBs and dispatch boxci issue-agent runs.
#
# Intended for BOXCI_TRIGGER=poll — POST /api/poll or systemd timer every 10m.
# Garden HTTPS clones expose issue COBs as:
#   refs/namespaces/*/refs/cobs/xyz.radicle.issue/<id>
#
# Dispatch: scripts/boxci/trigger-issue.sh (POST /api/runs/from-repo trigger=issue)
#
# Idempotency: skip issues that already have a remote branch issue/<short7>.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

# shellcheck source=../buildkite/lib.sh
source "$SCRIPT_DIR/../buildkite/lib.sh"

bk_require_cmd git

poll_remote() {
  if [[ -n "${RADICLE_POLL_REMOTE:-}" ]]; then
    echo "$RADICLE_POLL_REMOTE"
    return 0
  fi
  local url
  for candidate in origin rad; do
    url="$(git remote get-url "$candidate" 2>/dev/null || true)"
    [[ -n "$url" ]] || continue
    if [[ "$url" == *radicle.garden* || "$url" == https://* || "$url" == http://* ]]; then
      echo "$candidate"
      return 0
    fi
  done
  if url="$(git remote get-url rad 2>/dev/null || true)"; then
    if [[ "$url" =~ rad://([A-Za-z0-9]+) ]]; then
      echo "https://nandi.radicle.garden/${BASH_REMATCH[1]}.git"
      return 0
    fi
  fi
  if git remote get-url origin >/dev/null 2>&1; then
    echo origin
    return 0
  fi
  return 1
}

REMOTE="$(poll_remote)" || bk_die "no git remote to poll for issue COBs"
echo "poll: listing issue COBs via git ls-remote ($REMOTE)"

list_issue_ids() {
  git ls-remote "$REMOTE" 2>/dev/null \
    | awk '{print $2}' \
    | grep -E 'refs/cobs/xyz\.radicle\.issue/[0-9a-f]{40}$' \
    | sed -E 's|.*/||' \
    | sort -u
}

mapfile -t issue_ids < <(list_issue_ids || true)
if [[ "${#issue_ids[@]}" -eq 0 ]] && command -v rad >/dev/null 2>&1; then
  echo "poll: ls-remote found no COBs — trying rad cob list"
  rid="$(git remote get-url rad 2>/dev/null || true)"
  if [[ -n "$rid" ]]; then
    mapfile -t issue_ids < <(rad cob list --repo "$rid" --type xyz.radicle.issue 2>/dev/null || true)
  fi
fi

if [[ "${#issue_ids[@]}" -eq 0 ]]; then
  echo "poll: no issue COBs found"
  exit 0
fi

echo "poll: ${#issue_ids[@]} issue COB(s)"

branch_exists() {
  local short=$1
  git ls-remote "$REMOTE" 2>/dev/null | grep -E "/refs/heads/issue/${short}$|refs/heads/issue/${short}$" >/dev/null
}

pending=()
for id in "${issue_ids[@]}"; do
  short="$(bk_short_id "$id")"
  if branch_exists "$short"; then
    echo "poll: skip $short — remote branch issue/${short} exists"
    continue
  fi
  pending+=("$id")
done

if [[ "${#pending[@]}" -eq 0 ]]; then
  echo "poll: nothing to dispatch"
  exit 0
fi

echo "poll: ${#pending[@]} issue(s) to dispatch"

for id in "${pending[@]}"; do
  short="$(bk_short_id "$id")"
  echo "poll: dispatch $short ($id)"
  if [[ "${RADICLE_AGENT_DRY_RUN:-0}" == "1" ]]; then
    "$SCRIPT_DIR/trigger-issue.sh" --dry-run "$id"
  else
    "$SCRIPT_DIR/trigger-issue.sh" "$id"
  fi
done

echo "poll: done"
