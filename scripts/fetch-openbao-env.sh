#!/usr/bin/env bash
# Fetch KV v2 secrets from OpenBao and emit KEY=VALUE lines (systemd/dotenv style).
#
# Defaults match eve on boxd:
#   OPENBAO_ADDR=https://openbao.boxd.sh
#   OPENBAO_SECRET_PATH=secret/data/ai-api-keys
#
# Usage:
#   eval "$(./scripts/fetch-openbao-env.sh)"
#   ./scripts/fetch-openbao-env.sh --github-env   # append to $GITHUB_ENV (CI)
#   ./scripts/fetch-openbao-env.sh --keys CACHIX_AUTH_TOKEN,CACHIX_CACHE
set -euo pipefail

OPENBAO_ADDR="${OPENBAO_ADDR:-https://openbao.boxd.sh}"
OPENBAO_SECRET_PATH="${OPENBAO_SECRET_PATH:-secret/data/ai-api-keys}"
# Optional extra paths (comma-separated), tried after the primary path.
OPENBAO_EXTRA_PATHS="${OPENBAO_EXTRA_PATHS:-secret/data/cachix}"

MODE=export
KEYS=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --github-env) MODE=github-env; shift ;;
    --export) MODE=export; shift ;;
    --keys)
      KEYS="${2:-}"
      shift 2
      ;;
    -h | --help)
      sed -n '2,14p' "$0"
      exit 0
      ;;
    *)
      echo "fetch-openbao-env: unknown arg: $1" >&2
      exit 2
      ;;
  esac
done

if [[ -z "${OPENBAO_TOKEN:-}" ]]; then
  echo "fetch-openbao-env: OPENBAO_TOKEN must be set" >&2
  exit 1
fi

ADDR="${OPENBAO_ADDR%/}"
# Normalize path: accept secret/ai-api-keys or secret/data/ai-api-keys
normalize_path() {
  local p="$1"
  p="${p#/}"
  if [[ "$p" == secret/data/* ]]; then
    printf '%s' "$p"
  elif [[ "$p" == secret/* ]]; then
    printf 'secret/data/%s' "${p#secret/}"
  else
    printf 'secret/data/%s' "$p"
  fi
}

declare -A VALUES=()

fetch_path() {
  local path="$1"
  local quiet="${2:-0}"
  local url resp http
  path="$(normalize_path "$path")"
  url="$ADDR/v1/$path"
  http="$(curl -sS -o /tmp/fetch-openbao-env.body -w '%{http_code}' \
    -H "X-Vault-Token: $OPENBAO_TOKEN" "$url" || true)"
  if [[ "$http" != "200" ]]; then
    if [[ "$quiet" != "1" ]]; then
      echo "fetch-openbao-env: failed to read $url (HTTP ${http:-000})" >&2
    fi
    return 1
  fi
  resp="$(cat /tmp/fetch-openbao-env.body)"
  rm -f /tmp/fetch-openbao-env.body
  while IFS= read -r line; do
    [[ -z "$line" ]] && continue
    local key="${line%%=*}"
    local value="${line#*=}"
    # First path wins for a given key.
    if [[ -z "${VALUES[$key]+x}" ]]; then
      VALUES["$key"]="$value"
    fi
  done < <(jq -r '.data.data | to_entries[] | "\(.key)=\(.value|tostring)"' <<<"$resp")
}

PRIMARY="$(normalize_path "$OPENBAO_SECRET_PATH")"
fetch_path "$PRIMARY" || {
  echo "fetch-openbao-env: primary path failed: $PRIMARY" >&2
  exit 1
}

IFS=',' read -r -a extra <<<"$OPENBAO_EXTRA_PATHS"
for p in "${extra[@]}"; do
  p="$(echo "$p" | xargs)"
  [[ -z "$p" ]] && continue
  [[ "$(normalize_path "$p")" == "$PRIMARY" ]] && continue
  # Optional extras: missing path is fine (e.g. secret/data/cachix absent).
  fetch_path "$p" 1 || true
done

want_key() {
  local key="$1"
  if [[ -z "$KEYS" ]]; then
    return 0
  fi
  [[ ",$KEYS," == *",$key,"* ]]
}

emit() {
  local key="$1" value="$2"
  case "$MODE" in
    github-env)
      # Mask in Actions logs; append to GITHUB_ENV without printing the value.
      if [[ -n "${GITHUB_ENV:-}" ]]; then
        if [[ -n "${GITHUB_ACTIONS:-}" ]]; then
          echo "::add-mask::$value"
        fi
        {
          echo "${key}<<EOF"
          echo "$value"
          echo "EOF"
        } >>"$GITHUB_ENV"
      else
        echo "fetch-openbao-env: GITHUB_ENV unset" >&2
        exit 1
      fi
      echo "fetch-openbao-env: set $key" >&2
      ;;
    export)
      # Shell-safe for eval "$(...)"
      printf 'export %s=%q\n' "$key" "$value"
      ;;
  esac
}

found=0
for key in "${!VALUES[@]}"; do
  want_key "$key" || continue
  emit "$key" "${VALUES[$key]}"
  found=$((found + 1))
done

if [[ -n "$KEYS" ]]; then
  IFS=',' read -r -a need <<<"$KEYS"
  for key in "${need[@]}"; do
    key="$(echo "$key" | xargs)"
    [[ -z "$key" ]] && continue
    if [[ -z "${VALUES[$key]+x}" ]]; then
      echo "fetch-openbao-env: missing required key '$key' in OpenBao paths" >&2
      exit 1
    fi
  done
fi

if [[ "$found" -eq 0 ]]; then
  echo "fetch-openbao-env: no keys emitted from OpenBao" >&2
  exit 1
fi
