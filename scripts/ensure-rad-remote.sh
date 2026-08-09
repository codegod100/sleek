#!/usr/bin/env bash
# Ensure the working tree has a `rad` git remote for the sleek RID.
#
# Cloud / Codespace checkouts only get `origin` (GitHub). Radicle patch publish
# (`git push rad`, radicle MCP create_patch) needs remote `rad`.
#
# Idempotent: safe to run from install, start, or codespace-bootstrap.
set -euo pipefail

RADICLE_RID="${RADICLE_RID:-rad:z9mjPzpVK472QXaaP1picc5U9xBR}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! git -C "$ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "ensure-rad-remote: not a git repo: $ROOT" >&2
  exit 1
fi

if url="$(git -C "$ROOT" remote get-url rad 2>/dev/null || true)"; then
  if [[ "$url" == "$RADICLE_RID" ]]; then
    echo "ensure-rad-remote: rad → $url"
    exit 0
  fi
  echo "ensure-rad-remote: updating rad ($url → $RADICLE_RID)" >&2
  git -C "$ROOT" remote set-url rad "$RADICLE_RID"
else
  git -C "$ROOT" remote add rad "$RADICLE_RID"
  echo "ensure-rad-remote: added rad → $RADICLE_RID"
fi

git -C "$ROOT" remote get-url rad >/dev/null
