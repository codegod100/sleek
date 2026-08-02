#!/usr/bin/env bash
# Configure the GitHub CLI from a PAT stored in OpenBao.
#
# Replaces the limited Cursor/Codespaces integration token so `gh` can
# manage Actions secrets, etc.
#
# Requires:
#   OPENBAO_TOKEN  — OpenBao read token
#   OpenBao key GH_TOKEN under secret/data/ai-api-keys (or OPENBAO_SECRET_PATH)
#
# Usage:
#   ./scripts/configure-gh-from-openbao.sh
#   OPENBAO_ADDR=https://openbao.boxd.sh ./scripts/configure-gh-from-openbao.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OPENBAO_ADDR="${OPENBAO_ADDR:-https://openbao.boxd.sh}"
OPENBAO_SECRET_PATH="${OPENBAO_SECRET_PATH:-secret/data/ai-api-keys}"

if [[ -z "${OPENBAO_TOKEN:-}" ]]; then
  echo "configure-gh: OPENBAO_TOKEN unset — skipping gh reconfigure" >&2
  exit 0
fi

if ! command -v gh >/dev/null 2>&1; then
  echo "configure-gh: gh not on PATH" >&2
  exit 1
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "configure-gh: jq required" >&2
  exit 1
fi

exports=""
if ! exports="$("$ROOT/scripts/fetch-openbao-env.sh" --export --keys GH_TOKEN 2>/dev/null)"; then
  echo "configure-gh: GH_TOKEN missing from OpenBao ($OPENBAO_SECRET_PATH)" >&2
  echo "configure-gh: from a machine with gh logged in + OPENBAO_TOKEN:" >&2
  echo "  ./scripts/openbao-put-key.sh GH_TOKEN --from-gh" >&2
  exit 1
fi
# shellcheck disable=SC1091
eval "$exports"
if [[ -z "${GH_TOKEN:-}" ]]; then
  echo "configure-gh: GH_TOKEN empty after fetch" >&2
  exit 1
fi

# GH_TOKEN env overrides hosts.yml — keep the OpenBao PAT for this process and
# persist it via gh auth so child shells work too.
export GH_TOKEN
printf '%s' "$GH_TOKEN" | gh auth login --hostname github.com --with-token >/dev/null

login="$(gh api user --jq .login 2>/dev/null || echo unknown)"
echo "configure-gh: gh authenticated as ${login}" >&2

# Smoke: Actions secrets API should be reachable with a real PAT.
if gh api "repos/codegod100/sleek/actions/secrets/public-key" --jq .key_id >/dev/null 2>&1; then
  echo "configure-gh: Actions secrets API OK" >&2
else
  echo "configure-gh: warning: secrets API still denied — PAT may lack repo/admin scope" >&2
fi
