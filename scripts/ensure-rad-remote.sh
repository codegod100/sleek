#!/usr/bin/env bash
# Ensure the working tree has a `rad` git remote for the sleek RID.
#
# Cloud / Codespace checkouts only get `origin` (GitHub). Radicle patch publish
# (`git push rad`, radicle MCP create_patch) needs remote `rad`.
#
# Idempotent: safe to run from install, start, or codespace-bootstrap.
set -euo pipefail

# RID form is rad:<id>; git-remote-rad / patch publish use rad://<id>.
RADICLE_RID="${RADICLE_RID:-rad:z9mjPzpVK472QXaaP1picc5U9xBR}"
rid_naked="${RADICLE_RID#rad://}"
rid_naked="${rid_naked#rad:}"
RADICLE_REMOTE_URL="${RADICLE_REMOTE_URL:-rad://${rid_naked}}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! git -C "$ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "ensure-rad-remote: not a git repo: $ROOT" >&2
  exit 1
fi

if url="$(git -C "$ROOT" remote get-url rad 2>/dev/null || true)"; then
  if [[ "$url" == "$RADICLE_REMOTE_URL" || "$url" == "rad:${rid_naked}" || "$url" == "$RADICLE_RID" ]]; then
    # Normalize to rad:// so git-remote-rad and radicle MCP agree.
    if [[ "$url" != "$RADICLE_REMOTE_URL" ]]; then
      git -C "$ROOT" remote set-url rad "$RADICLE_REMOTE_URL"
      echo "ensure-rad-remote: normalized rad → $RADICLE_REMOTE_URL"
    else
      echo "ensure-rad-remote: rad → $url"
    fi
    exit 0
  fi
  echo "ensure-rad-remote: updating rad ($url → $RADICLE_REMOTE_URL)" >&2
  git -C "$ROOT" remote set-url rad "$RADICLE_REMOTE_URL"
else
  git -C "$ROOT" remote add rad "$RADICLE_REMOTE_URL"
  echo "ensure-rad-remote: added rad → $RADICLE_REMOTE_URL"
fi

git -C "$ROOT" remote get-url rad >/dev/null
