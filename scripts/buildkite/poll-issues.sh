#!/usr/bin/env bash
# Poll for Radicle issue COBs and dispatch cursor-agent steps.
#
# Intended for RADICLE_TRIGGER=poll builds — the 10m Buildkite schedule, or a
# manual dispatch (UI New Build / ./scripts/buildkite/dispatch-poll.sh).
# Garden HTTPS clones expose issue COBs as:
#   refs/namespaces/*/refs/cobs/xyz.radicle.issue/<id>
# so this does not require a local `rad` install on agents.
#
# Dispatch strategy (first match):
#   1. If BUILDKITE_API_TOKEN is available → trigger-issue-build.sh (separate builds)
#   2. Else if buildkite-agent is available → pipeline-upload agent steps into this build
#
# Idempotency: skip issues that already have a remote branch issue/<short7>.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

bk_require_cmd git

# Soft-load API token from cluster secrets when running on an agent.
if [[ -z "${BUILDKITE_API_TOKEN:-}" ]] && command -v buildkite-agent >/dev/null 2>&1; then
  if tok="$(buildkite-agent secret get BUILDKITE_API_TOKEN 2>/dev/null || true)"; then
    [[ -n "$tok" ]] && export BUILDKITE_API_TOKEN="$tok"
  fi
fi

# Prefer a Git remote that advertises COB refs (Garden HTTPS). Plain `rad://`
# ls-remote often omits namespaces/*/refs/cobs/*.
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
  # Derive Garden URL from rad RID when only rad:// is configured (local nodes).
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

# Fallback: rad cob list when ls-remote sees no COBs (some remotes hide them).
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

dispatch_via_api() {
  local id=$1
  BUILDKITE_ORG="${BUILDKITE_ORG:-nandi}" \
  BUILDKITE_PIPELINE="${BUILDKITE_PIPELINE:-sleek-5u9xbr}" \
    "$SCRIPT_DIR/trigger-issue-build.sh" "$id"
}

dispatch_via_upload() {
  local id=$1
  local short
  short="$(bk_short_id "$id")"
  local yaml
  yaml="$(cat <<EOF
steps:
  - label: ":radicle: Issue ${short} → :cursor: agent"
    key: "radicle-issue-agent-${short}"
    command: bash scripts/buildkite/run-issue-agent.sh
    timeout_in_minutes: 90
    secrets:
      - CURSOR_API_KEY
      - RADICLE_SECRET_KEY
      - RADICLE_PUBLIC_KEY
    env:
      RADICLE_ISSUE_ID: "${id}"
      RADICLE_TRIGGER: "issue"
      CURSOR_API_KEY: "\${CURSOR_API_KEY}"
EOF
)"
  echo "poll: pipeline-upload agent step for $short"
  echo "$yaml" | buildkite-agent pipeline upload
}

for id in "${pending[@]}"; do
  short="$(bk_short_id "$id")"
  echo "poll: dispatch $short ($id)"
  if [[ -n "${BUILDKITE_API_TOKEN:-}" ]]; then
    dispatch_via_api "$id"
  elif command -v buildkite-agent >/dev/null 2>&1; then
    dispatch_via_upload "$id"
  else
    echo "poll: would dispatch $short (no BUILDKITE_API_TOKEN / buildkite-agent here)"
  fi
done

echo "poll: done"
