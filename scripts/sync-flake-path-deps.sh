#!/usr/bin/env bash
# Materialize sibling path deps from flake.lock inputs.
#
# android/Cargo.toml expects:
#   ../vidya   (Radicle Garden pin)
#   ../freeq/freeq-sdk
# relative to the sleek repo root (i.e. $WORKSPACES/vidya, $WORKSPACES/freeq).
#
# Destinations are always next to the repo: parent of $BOXCI_REPO_ROOT when set,
# otherwise parent of this checkout. Shared CI siblings may be root-owned; we
# stage + rename so only the parent dir needs to be writable (e.g. boxci's
# $BOXCI_ROOT/workspaces/).
#
# freeq-sdk also path-depends on freeq-oauth + freeq-ssrf in the same freeq
# tree — those crates must be present or cargo check fails.
#
# Requires jq + patch on PATH (and nix). boxci CF Containers images lack them;
# .boxci/pipeline.yml wraps this script in `nix shell nixpkgs#jq nixpkgs#gnupatch …`.
# Locally you can do the same, or install jq/patch on the host.
#
# Usage:
#   bash scripts/sync-flake-path-deps.sh           # vidya + freeq
#   bash scripts/sync-flake-path-deps.sh vidya     # one dep
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Prefer explicit boxci checkout root so siblings land under workspaces/.
if [[ -n "${BOXCI_REPO_ROOT:-}" ]]; then
  ROOT="$(cd "$BOXCI_REPO_ROOT" && pwd)"
fi
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
  # Tolerate flake evaluation when /etc/nix is not writable (CF Containers).
  if [[ -z "${NIX_CONFIG:-}" ]]; then
    export NIX_CONFIG=$'experimental-features = nix-command flakes\naccept-flake-config = true'
  fi
}

flake_input_path() {
  local input="$1"
  (cd "$ROOT" && nix eval --impure --raw --expr "(builtins.getFlake (toString ./.)).inputs.${input}.outPath")
}

# freeq-sdk's workspace path deps — incomplete siblings break cargo check.
freeq_required_tomls() {
  local dest="$1"
  printf '%s\n' \
    "$dest/freeq-sdk/Cargo.toml" \
    "$dest/freeq-oauth/Cargo.toml" \
    "$dest/freeq-ssrf/Cargo.toml"
}

expected_marker() {
  local name="$1" dest="$2"
  local f
  case "$name" in
    vidya) [[ -f "$dest/Cargo.toml" ]] ;;
    freeq)
      while IFS= read -r f; do
        [[ -f "$f" ]] || return 1
      done < <(freeq_required_tomls "$dest")
      return 0
      ;;
    *) [[ -e "$dest" ]] ;;
  esac
}

describe_missing_markers() {
  local name="$1" dest="$2"
  local f
  case "$name" in
    vidya)
      [[ -f "$dest/Cargo.toml" ]] || echo "$dest/Cargo.toml"
      ;;
    freeq)
      while IFS= read -r f; do
        [[ -f "$f" ]] || echo "$f"
      done < <(freeq_required_tomls "$dest")
      ;;
  esac
}

# Install a staged tree at $dest. Shared CI siblings (e.g. /home/boxd/freeq)
# may contain root-owned files that block in-place cp; renaming the directory
# only needs write on the parent, so stage → mv aside → mv in.
install_sibling() {
  local name="$1" dest="$2" staging="$3"
  local parent tmp old missing

  parent="$(dirname "$dest")"
  mkdir -p "$parent"

  if [[ -L "$dest" ]]; then
    rm -f "$dest"
  fi

  if [[ ! -e "$dest" ]]; then
    cp -a "$staging" "$dest"
    chmod -R u+w "$dest" 2>/dev/null || true
    if ! expected_marker "$name" "$dest"; then
      missing="$(describe_missing_markers "$name" "$dest" | tr '\n' ' ')"
      log "fresh install of ${name} incomplete (missing: ${missing})"
      return 1
    fi
    return 0
  fi

  # Fast path: fully writable tree — wipe and copy in place.
  if [[ -d "$dest" && -w "$dest" ]] && touch "$dest/.sync-write-test" 2>/dev/null; then
    rm -f "$dest/.sync-write-test"
    if find "$dest" -mindepth 1 -maxdepth 1 -exec rm -rf {} + 2>/dev/null \
      && cp -a "$staging/." "$dest/" \
      && chmod -R u+w "$dest" 2>/dev/null; then
      if expected_marker "$name" "$dest"; then
        return 0
      fi
      missing="$(describe_missing_markers "$name" "$dest" | tr '\n' ' ')"
      log "in-place sync of ${name} left incomplete tree (missing: ${missing}); trying atomic replace"
    else
      log "in-place sync of ${name} failed; trying atomic replace"
    fi
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
        if expected_marker "$name" "$dest"; then
          return 0
        fi
        missing="$(describe_missing_markers "$name" "$dest" | tr '\n' ' ')"
        log "atomic replace of ${name} incomplete (missing: ${missing})"
        return 1
      fi
      mv "$old" "$dest" 2>/dev/null || true
      rm -rf "$tmp"
    else
      rm -rf "$tmp"
    fi
  fi

  # Only reuse a sibling that already has the full required graph. A stale
  # freeq with freeq-sdk but no freeq-oauth used to pass here and broke CI.
  if expected_marker "$name" "$dest"; then
    log "warning: ${dest} not fully writable; reusing existing complete checkout"
    return 0
  fi

  missing="$(describe_missing_markers "$name" "$dest" | tr '\n' ' ')"
  log "cannot sync ${name} → ${dest} (not writable; incomplete checkout missing: ${missing})"
  return 1
}

sync_input() {
  local name="$1" input="$2"
  local dest="$WORKSPACES/$name"
  local src rev staging missing

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

  if ! expected_marker "$name" "$staging"; then
    missing="$(describe_missing_markers "$name" "$staging" | tr '\n' ' ')"
    log "flake input ${input} @ ${rev:0:7} incomplete before install (missing: ${missing})"
    return 1
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

freeq_ok() {
  expected_marker freeq "$WORKSPACES/freeq"
}

vidya_ok() {
  expected_marker vidya "$WORKSPACES/vidya"
}

main() {
  if ! command -v nix >/dev/null 2>&1; then
    log "nix not on PATH"
    exit 1
  fi
  load_nix
  if ! command -v jq >/dev/null 2>&1; then
    log "jq not on PATH (boxci: wrap with nix shell nixpkgs#jq nixpkgs#gnupatch)"
    exit 1
  fi
  if ! command -v patch >/dev/null 2>&1; then
    log "patch not on PATH (boxci: wrap with nix shell nixpkgs#gnupatch)"
    exit 1
  fi

  if want vidya "$@"; then
    sync_input vidya vidya
  fi
  if want freeq "$@"; then
    sync_input freeq freeq
  fi

  if vidya_ok && freeq_ok; then
    log "ok (vidya + freeq under $WORKSPACES)"
  elif vidya_ok; then
    log "ok (vidya under $WORKSPACES)"
  elif freeq_ok; then
    log "ok (freeq under $WORKSPACES)"
  else
    log "sync finished but expected Cargo.toml files are missing"
    describe_missing_markers vidya "$WORKSPACES/vidya" >&2 || true
    describe_missing_markers freeq "$WORKSPACES/freeq" >&2 || true
    exit 1
  fi
}

main "$@"
