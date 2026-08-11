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

expected_marker() {
  local name="$1" dest="$2"
  case "$name" in
    vidya) [[ -f "$dest/Cargo.toml" ]] ;;
    freeq) [[ -f "$dest/freeq-sdk/Cargo.toml" ]] ;;
    *) [[ -e "$dest" ]] ;;
  esac
}

# Install a staged tree at $dest. Shared CI siblings (e.g. /home/boxd/freeq)
# may contain root-owned files that block in-place cp; renaming the directory
# only needs write on the parent, so stage → mv aside → mv in.
install_sibling() {
  local name="$1" dest="$2" staging="$3"
  local parent tmp old

  parent="$(dirname "$dest")"
  mkdir -p "$parent"

  if [[ -L "$dest" ]]; then
    rm -f "$dest"
  fi

  if [[ ! -e "$dest" ]]; then
    cp -a "$staging" "$dest"
    chmod -R u+w "$dest" 2>/dev/null || true
    return 0
  fi

  # Fast path: fully writable tree — wipe and copy in place.
  if [[ -d "$dest" && -w "$dest" ]] && touch "$dest/.sync-write-test" 2>/dev/null; then
    rm -f "$dest/.sync-write-test"
    if find "$dest" -mindepth 1 -maxdepth 1 -exec rm -rf {} + 2>/dev/null \
      && cp -a "$staging/." "$dest/" \
      && chmod -R u+w "$dest" 2>/dev/null; then
      return 0
    fi
    log "in-place sync of ${name} failed; trying atomic replace"
  fi

  # Atomic replace via rename (works when nested files aren't writable but the
  # parent directory is).
  if [[ -w "$parent" ]]; then
    tmp="${dest}.new.$$"
    old="${dest}.old.$$"
    rm -rf "$tmp" "$old"
    cp -a "$staging" "$tmp"
    if mv "$dest" "$old" 2>/dev/null; then
      if mv "$tmp" "$dest"; then
        rm -rf "$old" 2>/dev/null || log "warning: left behind ${old} (could not delete)"
        chmod -R u+w "$dest" 2>/dev/null || true
        return 0
      fi
      mv "$old" "$dest" 2>/dev/null || true
      rm -rf "$tmp"
    else
      rm -rf "$tmp"
    fi
  fi

  if expected_marker "$name" "$dest"; then
    log "warning: ${dest} not fully writable; reusing existing checkout"
    return 0
  fi

  log "cannot sync ${name} → ${dest} (not writable and no usable checkout)"
  return 1
}

sync_input() {
  local name="$1" input="$2"
  local dest="$WORKSPACES/$name"
  local src rev staging
  src="$(flake_input_path "$input")"
  rev="$(jq -r ".nodes.${input}.locked.rev" "$ROOT/flake.lock")"
  log "syncing ${name} (${input} @ ${rev:0:7}) → ${dest}"

  staging="$ROOT/.deps-sync/${name}"
  rm -rf "$staging"
  mkdir -p "$staging"
  cp -a "$src/." "$staging/"
  chmod -R u+w "$staging"

  if [[ "$input" == "vidya" && -f "$ROOT/patches/vidya-android-winit.patch" ]]; then
    # Best-effort: Radicle tip may not need / match this GitHub-era patch.
    if ! patch -p1 -d "$staging" --forward --batch \
      < "$ROOT/patches/vidya-android-winit.patch" >/dev/null; then
      log "vidya-android-winit.patch did not apply (ok if tip has no video/winit gap)"
      rm -f "$staging/Cargo.toml.rej" "$staging"/*.rej 2>/dev/null || true
    fi
  fi

  install_sibling "$name" "$dest" "$staging"
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
