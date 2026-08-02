#!/usr/bin/env bash
# Merge one KEY=VALUE into an OpenBao KV v2 secret (read-modify-write).
#
# Usage:
#   OPENBAO_TOKEN=… ./scripts/openbao-put-key.sh GH_TOKEN < token.txt
#   OPENBAO_TOKEN=… ./scripts/openbao-put-key.sh GH_TOKEN --from-gh
#   OPENBAO_TOKEN=… ./scripts/openbao-put-key.sh CACHIX_AUTH_TOKEN --value "$TOKEN"
#
# Defaults: OPENBAO_ADDR=https://openbao.boxd.sh
#           OPENBAO_SECRET_PATH=secret/data/ai-api-keys
set -euo pipefail

OPENBAO_ADDR="${OPENBAO_ADDR:-https://openbao.boxd.sh}"
OPENBAO_SECRET_PATH="${OPENBAO_SECRET_PATH:-secret/data/ai-api-keys}"

if [[ $# -lt 1 ]]; then
  sed -n '2,12p' "$0" >&2
  exit 2
fi

KEY="$1"
shift

if [[ -z "${OPENBAO_TOKEN:-}" ]]; then
  echo "openbao-put-key: OPENBAO_TOKEN must be set" >&2
  exit 1
fi

VALUE=""
case "${1:-}" in
  --from-gh)
    VALUE="$(gh auth token)"
    ;;
  --value)
    VALUE="${2:-}"
    ;;
  "")
    # Read from stdin (no trailing newline required).
    VALUE="$(cat)"
    VALUE="${VALUE%$'\n'}"
    ;;
  *)
    echo "openbao-put-key: unknown arg: $1" >&2
    exit 2
    ;;
esac

if [[ -z "$VALUE" ]]; then
  echo "openbao-put-key: empty value for $KEY" >&2
  exit 1
fi

ADDR="${OPENBAO_ADDR%/}"
PATH_NORM="${OPENBAO_SECRET_PATH#/}"
if [[ "$PATH_NORM" != secret/data/* ]]; then
  if [[ "$PATH_NORM" == secret/* ]]; then
    PATH_NORM="secret/data/${PATH_NORM#secret/}"
  else
    PATH_NORM="secret/data/${PATH_NORM}"
  fi
fi

URL="$ADDR/v1/$PATH_NORM"
CURRENT="$(curl -fsS -H "X-Vault-Token: $OPENBAO_TOKEN" "$URL")"
MERGED="$(jq --arg k "$KEY" --arg v "$VALUE" '.data.data + {($k): $v}' <<<"$CURRENT")"
BODY="$(jq -n --argjson data "$MERGED" '{data: $data}')"

curl -fsS -X POST \
  -H "X-Vault-Token: $OPENBAO_TOKEN" \
  -H "Content-Type: application/json" \
  -d "$BODY" \
  "$URL" >/dev/null

echo "openbao-put-key: wrote $KEY → $PATH_NORM" >&2
