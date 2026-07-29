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

have_sudo() {
  command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null
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
  # Common install locations when PATH was not updated yet.
  export PATH="/nix/var/nix/profiles/default/bin:${HOME}/.nix-profile/bin:${PATH}"
}

nix_works() {
  command -v nix >/dev/null 2>&1 || return 1
  # Prefer a real store ping; fall back to --version if offline.
  if nix store ping --store daemon 2>/dev/null; then
    return 0
  fi
  if nix store ping 2>/dev/null; then
    return 0
  fi
  # Last resort: version only (does not prove store access).
  nix --version >/dev/null 2>&1
}

# Multi-user installs need the daemon; Codespaces often finish install without
# starting it (missing socket → later "big-lock: Permission denied").
ensure_nix_daemon() {
  local socket="/nix/var/nix/daemon-socket/socket"
  local daemon_bin=""

  if [[ -S "$socket" ]] && nix_works; then
    return 0
  fi

  # Single-user / init=none layouts have no daemon socket by design.
  if [[ ! -d /nix/var/nix/daemon-socket ]] && [[ -d /nix/store ]] && nix_works; then
    return 0
  fi

  if command -v nix-daemon >/dev/null 2>&1; then
    daemon_bin="$(command -v nix-daemon)"
  elif [[ -x /nix/var/nix/profiles/default/bin/nix-daemon ]]; then
    daemon_bin="/nix/var/nix/profiles/default/bin/nix-daemon"
  elif [[ -x /nix/var/nix/profiles/default/bin/nix ]]; then
    # Newer installs: `nix daemon`
    daemon_bin="/nix/var/nix/profiles/default/bin/nix"
  fi

  log "ensuring nix-daemon is running…"

  # systemd (when available and units exist)
  if command -v systemctl >/dev/null 2>&1 && run_root true 2>/dev/null; then
    run_root systemctl daemon-reload 2>/dev/null || true
    if run_root systemctl start nix-daemon.socket 2>/dev/null \
      || run_root systemctl start nix-daemon.service 2>/dev/null; then
      sleep 1
      if [[ -S "$socket" ]]; then
        log "nix-daemon started via systemd"
        return 0
      fi
    fi
  fi

  # Manual daemon (Codespaces / containers without working user systemd)
  if [[ -n "$daemon_bin" ]] && run_root true 2>/dev/null; then
    run_root mkdir -p /nix/var/nix/daemon-socket 2>/dev/null || true
    # Kill a stale non-listening process only if socket is missing.
    if [[ ! -S "$socket" ]]; then
      if [[ "$(basename "$daemon_bin")" == "nix" ]]; then
        run_root bash -c "nohup '$daemon_bin' daemon >>/tmp/nix-daemon.log 2>&1 &" || true
      else
        run_root bash -c "nohup '$daemon_bin' --daemon >>/tmp/nix-daemon.log 2>&1 &" || true
      fi
      local i
      for i in $(seq 1 40); do
        if [[ -S "$socket" ]]; then
          log "nix-daemon started (manual); logs: /tmp/nix-daemon.log"
          return 0
        fi
        sleep 0.25
      done
      log "daemon socket still missing after start; see /tmp/nix-daemon.log"
      if [[ -f /tmp/nix-daemon.log ]]; then
        tail -n 20 /tmp/nix-daemon.log >&2 || true
      fi
    fi
  else
    log "cannot start nix-daemon (need passwordless sudo or root)"
  fi

  return 1
}

# ── install nix if missing ───────────────────────────────────────────
load_nix_env

if ! command -v nix >/dev/null 2>&1; then
  log "installing nix (Determinate installer)…"
  # Prefer multi-user + systemd when we can manage the daemon later.
  # --no-confirm for non-interactive codespace create.
  curl --proto '=https' --tlsv1.2 -sSf -L https://install.determinate.systems/nix \
    | sh -s -- install --no-confirm
  load_nix_env
fi

if ! command -v nix >/dev/null 2>&1; then
  echo "sleek-bootstrap: nix still not on PATH after install" >&2
  echo "  try: . /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh" >&2
  exit 1
fi

# Repair / start daemon (fixes half-installed multi-user on Codespaces).
if ! nix_works; then
  ensure_nix_daemon || true
  load_nix_env
fi

if ! nix_works; then
  # Second chance: start daemon again after profile load.
  ensure_nix_daemon || true
fi

if ! nix store ping 2>/dev/null && ! nix store ping --store daemon 2>/dev/null; then
  echo "sleek-bootstrap: nix is installed but cannot talk to the store." >&2
  echo "  Socket: /nix/var/nix/daemon-socket/socket" >&2
  echo "  Try:    sudo systemctl start nix-daemon.socket" >&2
  echo "  Or:     sudo /nix/var/nix/profiles/default/bin/nix-daemon --daemon &" >&2
  echo "  Logs:   /tmp/nix-daemon.log" >&2
  # Keep going so hooks still get installed; enter will show the same hint.
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
  log "installing direnv via nix profile…"
  if nix profile install nixpkgs#direnv 2>/dev/null \
    || nix-env -iA nixpkgs.direnv 2>/dev/null; then
    load_nix_env
    log "direnv installed"
  else
    log "could not install direnv (optional); login shim will use nix develop"
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
    # Refresh path if repo moved (rebuild codespace).
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

# direnv bash hook (if present)
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
  if nix store ping 2>/dev/null || nix store ping --store daemon 2>/dev/null; then
    log "warming flake devShell (nix develop -c true)…"
    if nix develop "$ROOT" -c true; then
      log "flake ready"
    else
      log "flake warm failed (network?). You can still run: ./scripts/enter"
    fi
  else
    log "skipping flake warm — nix store unreachable (start nix-daemon first)"
  fi
fi

log "done. SSH: gh codespace ssh  →  auto nix shell (or ./scripts/enter)"
log "opt out: SLEEK_NO_AUTO_NIX=1"
if ! nix store ping 2>/dev/null && ! nix store ping --store daemon 2>/dev/null; then
  log "REPAIR: bash scripts/codespace-bootstrap.sh   # re-run to start daemon"
  exit 1
fi
