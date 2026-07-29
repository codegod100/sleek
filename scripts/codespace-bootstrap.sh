#!/usr/bin/env bash
# Bootstrap nix + direnv + login shim for GitHub Codespaces (and plain VMs).
#
# Run automatically from .devcontainer postCreate/postStart, or once by hand:
#   bash scripts/codespace-bootstrap.sh
#
# After this, `gh codespace ssh` into the workspace should land in the flake
# shell via scripts/codespace-env.sh (bashrc hook) or direnv.
set -euo pipefail

QUIET=0
for arg in "$@"; do
  case "$arg" in
    -q|--quiet) QUIET=1 ;;
  esac
done

log() {
  if [[ "$QUIET" -eq 0 ]]; then
    echo "sleek-bootstrap: $*" >&2
  fi
}

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

SOCKET="/nix/var/nix/daemon-socket/socket"
DAEMON_LOG="/tmp/nix-daemon.log"

have_sudo() {
  command -v sudo >/dev/null 2>&1 || return 1
  # Codespaces is passwordless; -n avoids hanging if not.
  if sudo -n true 2>/dev/null; then
    return 0
  fi
  # Fallback (may prompt); still useful in interactive recovery.
  sudo true 2>/dev/null
}

run_root() {
  if [[ "$(id -u)" -eq 0 ]]; then
    "$@"
  elif have_sudo; then
    sudo "$@"
  else
    return 1
  fi
}

# ── load nix into this shell ─────────────────────────────────────────
load_nix_env() {
  # shellcheck disable=SC1091
  if [[ -f /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh ]]; then
    . /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh
  elif [[ -f "$HOME/.nix-profile/etc/profile.d/nix.sh" ]]; then
    . "$HOME/.nix-profile/etc/profile.d/nix.sh"
  fi
  export PATH="/nix/var/nix/profiles/default/bin:${HOME}/.nix-profile/bin:${PATH}"
}

# True only when the store is usable (never trust `nix --version` alone).
nix_store_ok() {
  command -v nix >/dev/null 2>&1 || return 1
  if [[ -S "$SOCKET" ]]; then
    export NIX_REMOTE=daemon
    nix store ping --store daemon >/dev/null 2>&1 && return 0
  fi
  # Local store (single-user) — must not be the multi-user "big-lock denied" path.
  if NIX_REMOTE= nix store ping --store local >/dev/null 2>&1; then
    return 0
  fi
  nix store ping >/dev/null 2>&1
}

find_nix_daemon() {
  local c
  for c in \
    nix-daemon \
    /nix/var/nix/profiles/default/bin/nix-daemon \
    /run/current-system/sw/bin/nix-daemon \
    "$(command -v nix-daemon 2>/dev/null || true)"
  do
    if [[ -n "$c" ]] && command -v "$c" >/dev/null 2>&1; then
      echo "$c"
      return 0
    fi
    if [[ -n "$c" && -x "$c" ]]; then
      echo "$c"
      return 0
    fi
  done
  # Modern Determinate: `nix daemon` subcommand
  if [[ -x /nix/var/nix/profiles/default/bin/nix ]]; then
    echo "/nix/var/nix/profiles/default/bin/nix"
    return 0
  fi
  if command -v nix >/dev/null 2>&1; then
    command -v nix
    return 0
  fi
  return 1
}

start_daemon_manual() {
  local bin mode
  bin="$(find_nix_daemon)" || {
    log "no nix-daemon / nix binary found to start"
    return 1
  }

  run_root mkdir -p /nix/var/nix/daemon-socket || true
  run_root chmod 755 /nix/var/nix/daemon-socket || true

  # Wipe a dead socket file (not a live socket).
  if [[ -e "$SOCKET" && ! -S "$SOCKET" ]]; then
    run_root rm -f "$SOCKET" || true
  fi

  if [[ -S "$SOCKET" ]]; then
    return 0
  fi

  : >"$DAEMON_LOG"
  run_root chmod 666 "$DAEMON_LOG" 2>/dev/null || true

  if [[ "$(basename "$bin")" == "nix" ]]; then
    mode="daemon"
    log "starting: sudo $bin daemon  (log: $DAEMON_LOG)"
    # Use setsid so the daemon survives this script.
    run_root bash -c "setsid '$bin' daemon >>'$DAEMON_LOG' 2>&1 < /dev/null & echo \$!" \
      || run_root bash -c "nohup '$bin' daemon >>'$DAEMON_LOG' 2>&1 & echo \$!" \
      || true
  else
    mode="--daemon"
    log "starting: sudo $bin --daemon  (log: $DAEMON_LOG)"
    run_root bash -c "setsid '$bin' --daemon >>'$DAEMON_LOG' 2>&1 < /dev/null & echo \$!" \
      || run_root bash -c "nohup '$bin' --daemon >>'$DAEMON_LOG' 2>&1 & echo \$!" \
      || true
  fi

  local i
  for i in $(seq 1 60); do
    if [[ -S "$SOCKET" ]]; then
      log "nix-daemon socket is up ($mode)"
      export NIX_REMOTE=daemon
      return 0
    fi
    sleep 0.25
  done

  log "daemon did not create $SOCKET after 15s"
  if [[ -s "$DAEMON_LOG" ]]; then
    log "--- $DAEMON_LOG ---"
    tail -n 40 "$DAEMON_LOG" >&2 || true
  fi
  return 1
}

# Multi-user installs need the daemon. Codespaces often install Nix but leave
# the daemon dead → "big-lock: Permission denied".
ensure_nix_daemon() {
  if nix_store_ok; then
    return 0
  fi

  log "nix store not usable; repairing daemon…"
  log "  socket exists: $([[ -S "$SOCKET" ]] && echo yes || echo no)"
  log "  sudo: $(have_sudo && echo yes || echo no)"
  log "  uid: $(id -u) ($(id -un))"

  if ! have_sudo && [[ "$(id -u)" -ne 0 ]]; then
    log "need passwordless sudo to start nix-daemon"
    return 1
  fi

  # systemd first (when units exist and actually work)
  if command -v systemctl >/dev/null 2>&1; then
    run_root systemctl daemon-reload 2>/dev/null || true
    if run_root systemctl enable --now nix-daemon.socket 2>/dev/null \
      || run_root systemctl start nix-daemon.socket 2>/dev/null \
      || run_root systemctl start nix-daemon.service 2>/dev/null \
      || run_root systemctl start nix-daemon 2>/dev/null; then
      sleep 1
      if [[ -S "$SOCKET" ]] && nix_store_ok; then
        log "nix-daemon started via systemd"
        return 0
      fi
      log "systemd start returned ok but store still broken; trying manual daemon"
    else
      log "systemd units unavailable or failed; trying manual daemon"
    fi
  fi

  start_daemon_manual || true

  export NIX_REMOTE=daemon
  load_nix_env

  if nix_store_ok; then
    log "nix store OK after daemon repair"
    return 0
  fi

  # Last resort for disposable Codespaces: make the local store writable so
  # non-root can use single-user mode without a daemon.
  if [[ ! -S "$SOCKET" ]] && have_sudo; then
    log "falling back to single-user store ownership (Codespace repair)…"
    # Drop build-users requirement if present so local builds work.
    if [[ -f /etc/nix/nix.conf ]] && grep -q '^build-users-group' /etc/nix/nix.conf 2>/dev/null; then
      run_root sed -i 's/^build-users-group.*/build-users-group =/' /etc/nix/nix.conf 2>/dev/null || true
    fi
    run_root chown -R "$(id -u):$(id -g)" /nix/var/nix/db /nix/var/nix/gcroots /nix/var/nix/profiles /nix/var/nix/temproots 2>/dev/null || true
    # Store paths stay root-owned; only need write on the DB for many ops.
    # For full single-user, also own the store meta:
    run_root chown -R "$(id -u):$(id -g)" /nix/store 2>/dev/null || true
    unset NIX_REMOTE
    export NIX_REMOTE=""
    if NIX_REMOTE= nix store ping --store local >/dev/null 2>&1; then
      log "single-user local store works (NIX_REMOTE unset)"
      # Persist for interactive shells.
      if ! grep -qF 'sleek-nix-single-user' "$HOME/.bashrc" 2>/dev/null; then
        {
          echo ""
          echo "# sleek-nix-single-user"
          echo "unset NIX_REMOTE"
        } >>"$HOME/.bashrc"
      fi
      return 0
    fi
  fi

  return 1
}

# ── install nix if missing ───────────────────────────────────────────
load_nix_env

if ! command -v nix >/dev/null 2>&1; then
  log "installing nix (Determinate installer)…"
  curl --proto '=https' --tlsv1.2 -sSf -L https://install.determinate.systems/nix \
    | sh -s -- install --no-confirm
  load_nix_env
fi

if ! command -v nix >/dev/null 2>&1; then
  echo "sleek-bootstrap: nix still not on PATH after install" >&2
  echo "  try: . /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh" >&2
  exit 1
fi

# Always attempt repair when the store is broken (do not trust nix --version).
if ! nix_store_ok; then
  ensure_nix_daemon || true
  load_nix_env
fi

if ! nix_store_ok; then
  echo "sleek-bootstrap: nix is installed but cannot talk to the store." >&2
  echo "  Socket: $SOCKET  (exists=$([[ -e $SOCKET ]] && echo yes || echo no), socket=$([[ -S $SOCKET ]] && echo yes || echo no))" >&2
  echo "  Try:    sudo systemctl start nix-daemon.socket" >&2
  echo "  Or:     sudo \$(command -v nix) daemon &" >&2
  echo "  Or:     sudo /nix/var/nix/profiles/default/bin/nix-daemon --daemon &" >&2
  echo "  Logs:   $DAEMON_LOG" >&2
  if [[ -s "$DAEMON_LOG" ]]; then
    echo "  --- last log lines ---" >&2
    tail -n 20 "$DAEMON_LOG" >&2 || true
  fi
fi

# Persist PATH for non-login shells that don't source nix-daemon.sh.
NIX_PATH_LINE='[ -e /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh ] && . /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh'
NIX_PATH_MARKER="# sleek-nix-profile"
if [[ -f "$HOME/.bashrc" ]] && ! grep -qF "$NIX_PATH_MARKER" "$HOME/.bashrc" 2>/dev/null; then
  {
    echo ""
    echo "$NIX_PATH_MARKER"
    echo "$NIX_PATH_LINE"
  } >>"$HOME/.bashrc"
  log "added nix profile source to ~/.bashrc"
fi

# Flakes + new CLI (no-op if already set by Determinate / system nix.conf).
mkdir -p "$HOME/.config/nix"
if [[ ! -f "$HOME/.config/nix/nix.conf" ]] || ! grep -q 'experimental-features' "$HOME/.config/nix/nix.conf" 2>/dev/null; then
  {
    echo "experimental-features = nix-command flakes"
  } >>"$HOME/.config/nix/nix.conf"
fi

# ── direnv ───────────────────────────────────────────────────────────
if ! command -v direnv >/dev/null 2>&1; then
  if nix_store_ok; then
    log "installing direnv via nix profile…"
    if nix profile install nixpkgs#direnv 2>/dev/null \
      || nix-env -iA nixpkgs.direnv 2>/dev/null; then
      load_nix_env
      log "direnv installed"
    else
      log "could not install direnv (optional); login shim will use nix develop"
    fi
  else
    log "skipping direnv install (nix store not ready)"
  fi
fi

# ── bashrc hook (codespace ssh / interactive login) ──────────────────
HOOK_LINE="[ -f \"$ROOT/scripts/codespace-env.sh\" ] && . \"$ROOT/scripts/codespace-env.sh\""
HOOK_MARKER="# sleek-nix-shim"

ensure_hook() {
  local rc="$1"
  mkdir -p "$(dirname "$rc")"
  touch "$rc"
  if grep -qF "$HOOK_MARKER" "$rc" 2>/dev/null; then
    if ! grep -qF "$ROOT/scripts/codespace-env.sh" "$rc" 2>/dev/null; then
      local tmp
      tmp="$(mktemp)"
      grep -vF "$HOOK_MARKER" "$rc" | grep -vF "codespace-env.sh" >"$tmp" || true
      mv "$tmp" "$rc"
    else
      return 0
    fi
  fi
  {
    echo ""
    echo "$HOOK_MARKER"
    echo "$HOOK_LINE"
  } >>"$rc"
  log "hooked $rc"
}

ensure_hook "$HOME/.bashrc"

if command -v direnv >/dev/null 2>&1; then
  DIRENV_HOOK='eval "$(direnv hook bash)"'
  if ! grep -qF 'direnv hook bash' "$HOME/.bashrc" 2>/dev/null; then
    {
      echo ""
      echo "# direnv (sleek)"
      echo "$DIRENV_HOOK"
    } >>"$HOME/.bashrc"
    log "added direnv hook to ~/.bashrc"
  fi
  (cd "$ROOT" && direnv allow .) 2>/dev/null || true
fi

# ── warm the flake (best-effort; speeds first `enter`) ───────────────
if [[ "${SLEEK_SKIP_FLAKE_WARM:-}" != "1" ]]; then
  if nix_store_ok; then
    log "warming flake devShell (nix develop -c true)…"
    if nix develop "$ROOT" -c true; then
      log "flake ready"
    else
      log "flake warm failed (network?). You can still run: ./scripts/enter"
    fi
  else
    log "skipping flake warm — nix store unreachable"
  fi
fi

log "done. SSH: gh codespace ssh  →  auto nix shell (or ./scripts/enter)"
log "opt out: SLEEK_NO_AUTO_NIX=1"
if ! nix_store_ok; then
  log "FAILED: store still broken. Paste output of:"
  log "  ls -la /nix/var/nix/daemon-socket; cat $DAEMON_LOG; sudo systemctl status nix-daemon"
  exit 1
fi

log "nix store OK ($(nix --version 2>/dev/null | head -1))"
