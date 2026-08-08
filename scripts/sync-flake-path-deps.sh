#!/usr/bin/env bash
# Materialize sibling path deps from flake.lock inputs.
#
# android/Cargo.toml expects:
#   ../vidya   (Radicle Garden pin)
#   ../freeq/freeq-sdk
# relative to the sleek repo root (i.e. $WORKSPACES/vidya, $WORKSPACES/freeq).
#
# Usage:
#   bash scripts/sync-flake-path-deps.sh           # vidya + freeq
#   bash scripts/sync-flake-path-deps.sh vidya     # one dep
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORKSPACES="$(cd "$ROOT/.." && pwd)"

log() { echo "sync-flake-deps: $*" >&2; }

load_nix() {
  # shellcheck disable=SC1091
  if [[ -f /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh ]]; then
    . /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh
  elif [[ -f "$HOME/.nix-profile/etc/profile.d/nix.sh" ]]; then
    . "$HOME/.nix-profile/etc/profile.d/nix.sh"
  fi
  export PATH="/nix/var/nix/profiles/default/bin:${HOME}/.nix-profile/bin:${PATH}"
  if [[ -S /nix/var/nix/daemon-socket/socket ]]; then
    export NIX_REMOTE=daemon
  fi
}

flake_input_path() {
  local input="$1"
  (cd "$ROOT" && nix eval --impure --raw --expr "(builtins.getFlake (toString ./.)).inputs.${input}.outPath")
}

sync_input() {
  local name="$1" input="$2"
  local dest="$WORKSPACES/$name"
  local src rev
  src="$(flake_input_path "$input")"
  rev="$(jq -r ".nodes.${input}.locked.rev" "$ROOT/flake.lock")"
  log "syncing ${name} (${input} @ ${rev:0:7}) → ${dest}"
  mkdir -p "$dest"
  # Sibling dirs under / (e.g. /vidya on Codespaces) may not be removable; sync in place.
  find "$dest" -mindepth 1 -maxdepth 1 -exec rm -rf {} + 2>/dev/null || true
  cp -a "$src/." "$dest/"
  chmod -R u+w "$dest"
  if [[ "$input" == "vidya" && -f "$ROOT/patches/vidya-android-winit.patch" ]]; then
    # Best-effort: Radicle tip may not need / match this GitHub-era patch.
    if ! patch -p1 -d "$dest" --forward --batch \
      < "$ROOT/patches/vidya-android-winit.patch" >/dev/null; then
      log "vidya-android-winit.patch did not apply (ok if tip has no video/winit gap)"
      rm -f "$dest/Cargo.toml.rej" "$dest"/*.rej 2>/dev/null || true
    fi
  fi
}

want() {
  local name="$1"
  if [[ $# -eq 0 ]]; then
    return 0
  fi
  local arg
  for arg in "$@"; do
    [[ "$arg" == "$name" ]] && return 0
  done
  return 1
}

main() {
  if ! command -v nix >/dev/null 2>&1; then
    log "nix not on PATH"
    exit 1
  fi
  if ! command -v jq >/dev/null 2>&1; then
    log "jq not on PATH"
    exit 1
  fi
  load_nix

  if want vidya "$@"; then
    sync_input vidya vidya
  fi
  if want freeq "$@"; then
    sync_input freeq freeq
  fi

  if [[ -f "$WORKSPACES/vidya/Cargo.toml" && -f "$WORKSPACES/freeq/freeq-sdk/Cargo.toml" ]]; then
    log "ok (vidya + freeq under $WORKSPACES)"
  elif [[ -f "$WORKSPACES/vidya/Cargo.toml" ]]; then
    log "ok (vidya under $WORKSPACES)"
  elif [[ -f "$WORKSPACES/freeq/freeq-sdk/Cargo.toml" ]]; then
    log "ok (freeq under $WORKSPACES)"
  else
    log "sync finished but expected Cargo.toml files are missing"
    exit 1
  fi
}

main "$@"
