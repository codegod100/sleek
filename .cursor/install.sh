#!/usr/bin/env bash
# Cursor Cloud install — idempotent; safe to re-run on cached Builds.
#
# environment.json:
#   install → bash .cursor/install.sh
#   start   → bash .cursor/install.sh ensure-nix
#
# Full install bakes Nix, flake deps, sibling path crates, and a host cargo
# build into the snapshot. `ensure-nix` is the per-pod boot path (daemon + rad).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

SOCKET="/nix/var/nix/daemon-socket/socket"

log() {
  echo "sleek-install: $*" >&2
}

run_root() {
  if [[ "$(id -u)" -eq 0 ]]; then
    "$@"
  else
    sudo "$@"
  fi
}

load_nix_env() {
  # shellcheck disable=SC1091
  if [[ -f /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh ]]; then
    . /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh
  elif [[ -f "$HOME/.nix-profile/etc/profile.d/nix.sh" ]]; then
    . "$HOME/.nix-profile/etc/profile.d/nix.sh"
  fi
  export PATH="/nix/var/nix/profiles/default/bin:${HOME}/.nix-profile/bin:${PATH}"
  if nix_daemon_ping; then
    export NIX_REMOTE=daemon
  else
    unset NIX_REMOTE || true
  fi
}

nix_daemon_ping() {
  command -v nix >/dev/null 2>&1 || return 1
  [[ -S "$SOCKET" ]] || return 1
  NIX_REMOTE=daemon nix store ping --store daemon >/dev/null 2>&1
}

nix_store_ok() {
  command -v nix >/dev/null 2>&1 || return 1
  if nix_daemon_ping; then
    export NIX_REMOTE=daemon
    return 0
  fi
  unset NIX_REMOTE || true
  nix store ping --store local >/dev/null 2>&1 && [[ -w /nix/var/nix ]]
}

remove_stale_nix_socket() {
  # --init none leaves a socket path after reboot/snapshot with no daemon.
  if [[ -e "$SOCKET" ]] && ! nix_daemon_ping; then
    log "removing stale nix daemon socket at $SOCKET"
    run_root rm -f "$SOCKET"
    unset NIX_REMOTE || true
  fi
}

start_nix_daemon() {
  if nix_daemon_ping; then
    export NIX_REMOTE=daemon
    return 0
  fi

  remove_stale_nix_socket

  local bin="/nix/var/nix/profiles/default/bin/nix"
  if [[ ! -x "$bin" ]]; then
    bin="$(command -v nix || true)"
  fi
  [[ -n "$bin" ]] || return 1

  run_root mkdir -p /nix/var/nix/daemon-socket
  run_root chmod 755 /nix/var/nix/daemon-socket
  log "starting nix daemon (no systemd)…"
  run_root bash -c "setsid '$bin' daemon >>/tmp/nix-daemon.log 2>&1 < /dev/null &" || true

  local i
  for i in $(seq 1 60); do
    if nix_daemon_ping; then
      export NIX_REMOTE=daemon
      log "nix daemon is up"
      return 0
    fi
    sleep 0.25
  done

  log "nix daemon did not become reachable at $SOCKET"
  return 1
}

ensure_nix_daemon_ready() {
  load_nix_env
  if nix_store_ok; then
    return 0
  fi
  start_nix_daemon || true
  load_nix_env
  nix_store_ok
}

ensure_rad_remote() {
  if [[ -x "$ROOT/scripts/ensure-rad-remote.sh" ]]; then
    bash "$ROOT/scripts/ensure-rad-remote.sh"
  fi
}

ensure_path_deps() {
  local parent dest
  parent="$(cd "$ROOT/.." && pwd)"
  if [[ -f "$parent/vidya/Cargo.toml" && -f "$parent/freeq/freeq-sdk/Cargo.toml" ]]; then
    log "sibling path deps present ($parent/vidya, $parent/freeq)"
    return 0
  fi
  if ! command -v nix >/dev/null 2>&1 || ! nix_store_ok; then
    log "cannot sync path deps — nix store unavailable"
    return 1
  fi
  log "materializing flake.lock path deps…"
  bash "$ROOT/scripts/sync-flake-path-deps.sh"
}

warm_host_build() {
  if ! nix_store_ok; then
    log "skipping host cargo build (nix store unavailable)"
    return 0
  fi
  log "warming host cargo build (baked into snapshot)…"
  nix develop "$ROOT" --command cargo build --manifest-path host/Cargo.toml
}

# ── start (per-pod) ──────────────────────────────────────────────────
if [[ "${1:-}" == "ensure-nix" ]]; then
  if ! ensure_nix_daemon_ready; then
    log "nix store is not reachable"
    exit 1
  fi
  log "nix daemon ready ($(nix --version 2>/dev/null | head -1))"
  ensure_rad_remote
  # Snapshot usually already has siblings; re-sync only if missing.
  ensure_path_deps || true
  exit 0
fi

# ── install (snapshot bake) ──────────────────────────────────────────
# Codespace/Cursor helpers (chromium) when an X display is present.
export SLEEK_CODESPACE="${SLEEK_CODESPACE:-1}"

log "running codespace-bootstrap (nix, cachix, flake warm)…"
bash "$ROOT/scripts/codespace-bootstrap.sh"

load_nix_env
if ! nix_store_ok; then
  start_nix_daemon || true
  load_nix_env
fi
if ! nix_store_ok; then
  log "nix store still unreachable after bootstrap"
  exit 1
fi

ensure_path_deps
ensure_rad_remote
warm_host_build

log "done"
