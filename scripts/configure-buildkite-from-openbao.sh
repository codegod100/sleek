#!/usr/bin/env bash
# Configure Buildkite API auth from a token stored in OpenBao.
#
# Reads BUILDKITE_API_KEY from secret/data/ai-api-keys (same KV as GH_TOKEN)
# and exports BUILDKITE_API_TOKEN for the Buildkite REST API / `bk` CLI.
# The OpenBao key was provisioned for baogui / "bao" API access (org nandi).
#
# Requires:
#   OPENBAO_TOKEN  — OpenBao read token
#   OpenBao key BUILDKITE_API_KEY under secret/data/ai-api-keys (or OPENBAO_SECRET_PATH)
#
# Usage:
#   eval "$(./scripts/configure-buildkite-from-openbao.sh)"
#   ./scripts/configure-buildkite-from-openbao.sh --print-env   # KEY=VALUE lines
#   ./scripts/configure-buildkite-from-openbao.sh --check       # smoke org list
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OPENBAO_ADDR="${OPENBAO_ADDR:-https://openbao.boxd.sh}"
OPENBAO_SECRET_PATH="${OPENBAO_SECRET_PATH:-secret/data/ai-api-keys}"
BUILDKITE_ORG="${BUILDKITE_ORG:-nandi}"

MODE=export
while [[ $# -gt 0 ]]; do
  case "$1" in
    --print-env) MODE=print-env; shift ;;
    --check) MODE=check; shift ;;
    --export) MODE=export; shift ;;
    -h | --help)
      sed -n '2,18p' "$0"
      exit 0
      ;;
    *)
      echo "configure-buildkite: unknown arg: $1" >&2
      exit 2
      ;;
  esac
done

if [[ -z "${OPENBAO_TOKEN:-}" ]]; then
  echo "configure-buildkite: OPENBAO_TOKEN unset — skipping" >&2
  exit 0
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "configure-buildkite: jq required" >&2
  exit 1
fi

exports=""
if ! exports="$("$ROOT/scripts/fetch-openbao-env.sh" --export --keys BUILDKITE_API_KEY 2>/dev/null)"; then
  echo "configure-buildkite: BUILDKITE_API_KEY missing from OpenBao ($OPENBAO_SECRET_PATH)" >&2
  echo "configure-buildkite: store a Buildkite API token (org nandi) as BUILDKITE_API_KEY:" >&2
  echo "  ./scripts/openbao-put-key.sh BUILDKITE_API_KEY --value \"\$TOKEN\"" >&2
  exit 1
fi
# shellcheck disable=SC1091
eval "$exports"
if [[ -z "${BUILDKITE_API_KEY:-}" ]]; then
  echo "configure-buildkite: BUILDKITE_API_KEY empty after fetch" >&2
  exit 1
fi

export BUILDKITE_API_TOKEN="$BUILDKITE_API_KEY"

case "$MODE" in
  print-env)
    printf 'BUILDKITE_API_TOKEN=%s\n' "$BUILDKITE_API_TOKEN"
    printf 'BUILDKITE_API_KEY=%s\n' "$BUILDKITE_API_KEY"
    ;;
  export)
    printf 'export BUILDKITE_API_TOKEN=%q\n' "$BUILDKITE_API_TOKEN"
    printf 'export BUILDKITE_API_KEY=%q\n' "$BUILDKITE_API_KEY"
    echo "configure-buildkite: BUILDKITE_API_TOKEN set (org default: $BUILDKITE_ORG)" >&2
    ;;
  check)
    echo "configure-buildkite: BUILDKITE_API_TOKEN set — checking org $BUILDKITE_ORG" >&2
    resp="$(curl -sS -H "Authorization: Bearer $BUILDKITE_API_TOKEN" \
      "https://api.buildkite.com/v2/organizations/$BUILDKITE_ORG/pipelines?per_page=5")"
    if ! jq -e 'type == "array"' >/dev/null <<<"$resp"; then
      echo "configure-buildkite: API error:" >&2
      echo "$resp" | jq . >&2 || echo "$resp" >&2
      exit 1
    fi
    count="$(jq 'length' <<<"$resp")"
    echo "configure-buildkite: OK — listed $count pipeline(s) in $BUILDKITE_ORG" >&2
    jq -r '.[] | "  - \(.slug)  \(.web_url)"' <<<"$resp" >&2 || true
    printf 'export BUILDKITE_API_TOKEN=%q\n' "$BUILDKITE_API_TOKEN"
    printf 'export BUILDKITE_API_KEY=%q\n' "$BUILDKITE_API_KEY"
    ;;
esac
