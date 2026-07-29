#!/usr/bin/env bash
# nix build with optional auto-push to Cachix (codegod100 by default).
#
# When CACHIX_AUTH_TOKEN is set or ~/.config/cachix/cachix.dhall exists and
# cachix is on PATH, wraps the build in `cachix watch-exec` so every new store
# path is uploaded as it appears.
#
# Opt out:
#   SLEEK_CACHIX_PUSH=0 ./scripts/nix-build-push.sh .#android
#   just android   # respects SLEEK_CACHIX_PUSH
#
# Usage:
#   ./scripts/nix-build-push.sh .#android -L --out-link result-android
#   ./scripts/nix-build-push.sh .#default
set -euo pipefail

CACHIX_CACHE="${CACHIX_CACHE:-codegod100}"

cachix_can_push() {
  command -v cachix >/dev/null 2>&1 || return 1
  if [[ -n "${CACHIX_AUTH_TOKEN:-}" ]]; then
    return 0
  fi
  [[ -f "${HOME}/.config/cachix/cachix.dhall" ]]
}

if [[ "${SLEEK_CACHIX_PUSH:-1}" == "0" ]] || [[ "${SLEEK_CACHIX_PUSH:-}" == "no" ]]; then
  echo "sleek: Cachix push disabled (SLEEK_CACHIX_PUSH=${SLEEK_CACHIX_PUSH:-0})" >&2
  exec nix build "$@"
fi

if cachix_can_push; then
  # Prefer env token when present (Codespaces secret); never print it.
  if [[ -n "${CACHIX_AUTH_TOKEN:-}" ]]; then
    printf '%s' "$CACHIX_AUTH_TOKEN" | cachix authtoken --stdin >/dev/null 2>&1 || true
  fi
  echo "sleek: auto-push → ${CACHIX_CACHE}.cachix.org (cachix watch-exec)" >&2
  exec cachix watch-exec "$CACHIX_CACHE" -- nix build "$@"
fi

echo "sleek: no Cachix auth — build only (set CACHIX_AUTH_TOKEN to auto-push)" >&2
exec nix build "$@"
