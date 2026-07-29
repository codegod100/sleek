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
BASHRC_NIX_MARKER="# sleek-nix-env"

have_sudo() {
  command -v sudo >/dev/null 2>&1 || return 1
  if sudo -n true 2>/dev/null; then
    return 0
  fi
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
  # Force daemon mode when the socket is live; otherwise local single-user.
  if [[ -S "$SOCKET" ]]; then
    export NIX_REMOTE=daemon
  else
    unset NIX_REMOTE || true
  fi
}

# True only when the store is usable for real work (fetch + lock).
nix_store_ok() {
  command -v nix >/dev/null 2>&1 || return 1
  if [[ -S "$SOCKET" ]]; then
    export NIX_REMOTE=daemon
    nix store ping --store daemon >/dev/null 2>&1 || return 1
    # Prove the client is not falling back to a root-owned local store
    # (that path fails later on gc.lock while fetching flakes).
    if ! nix path-info --store daemon --json /nix/store 2>/dev/null | head -c1 >/dev/null; then
      # path-info of the store root is optional; ping is enough if remote is daemon.
      :
    fi
    return 0
  fi
  # Single-user: must be able to open the local DB (implies write to /nix/var/nix).
  unset NIX_REMOTE || true
  if ! NIX_REMOTE= nix store ping --store local >/dev/null 2>&1; then
    return 1
  fi
  # Can we create/open locks in /nix/var/nix as this user?
  if [[ ! -w /nix/var/nix ]]; then
    return 1
  fi
  return 0
}

find_nix_daemon() {
  local c
  for c in \
    /nix/var/nix/profiles/default/bin/nix-daemon \
    nix-daemon \
    "$(command -v nix-daemon 2>/dev/null || true)"
  do
    if [[ -n "$c" && -x "$c" ]]; then
      echo "$c"
      return 0
    fi
    if [[ -n "$c" ]] && command -v "$c" >/dev/null 2>&1; then
      command -v "$c"
      return 0
    fi
  done
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
  local bin
  bin="$(find_nix_daemon)" || {
    log "no nix-daemon / nix binary found to start"
    return 1
  }

  run_root mkdir -p /nix/var/nix/daemon-socket || true
  run_root chmod 755 /nix/var/nix/daemon-socket || true

  if [[ -e "$SOCKET" && ! -S "$SOCKET" ]]; then
    run_root rm -f "$SOCKET" || true
  fi

  if [[ -S "$SOCKET" ]]; then
    export NIX_REMOTE=daemon
    return 0
  fi

  : >"$DAEMON_LOG"
  run_root chmod 666 "$DAEMON_LOG" 2>/dev/null || true

  if [[ "$(basename "$bin")" == "nix" ]]; then
    log "starting: sudo $bin daemon  (log: $DAEMON_LOG)"
    run_root bash -c "setsid '$bin' daemon >>'$DAEMON_LOG' 2>&1 < /dev/null &" || true
  else
    log "starting: sudo $bin --daemon  (log: $DAEMON_LOG)"
    run_root bash -c "setsid '$bin' --daemon >>'$DAEMON_LOG' 2>&1 < /dev/null &" || true
  fi

  local i
  for i in $(seq 1 60); do
    if [[ -S "$SOCKET" ]]; then
      log "nix-daemon socket is up"
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

# Codespace-friendly single-user: user owns all of /nix so bare `nix develop` works
# without NIX_REMOTE/daemon. Disposable VMs only.
convert_to_single_user() {
  if ! have_sudo && [[ "$(id -u)" -ne 0 ]]; then
    return 1
  fi

  log "converting /nix to single-user ownership for $(id -un) (Codespace)…"

  # Stop any daemon so it does not fight ownership.
  if command -v systemctl >/dev/null 2>&1; then
    run_root systemctl stop nix-daemon.socket 2>/dev/null || true
    run_root systemctl stop nix-daemon.service 2>/dev/null || true
    run_root systemctl stop nix-daemon 2>/dev/null || true
  fi
  run_root pkill -x nix-daemon 2>/dev/null || true
  run_root pkill -f '[n]ix daemon' 2>/dev/null || true
  sleep 0.5
  run_root rm -f "$SOCKET" 2>/dev/null || true

  # Empty build-users-group → builds as calling user (single-user style).
  if [[ -f /etc/nix/nix.conf ]]; then
    if grep -qE '^build-users-group' /etc/nix/nix.conf 2>/dev/null; then
      run_root sed -i 's/^build-users-group.*/build-users-group =/' /etc/nix/nix.conf || true
    else
      echo 'build-users-group =' | run_root tee -a /etc/nix/nix.conf >/dev/null || true
    fi
    if ! grep -q 'experimental-features' /etc/nix/nix.conf 2>/dev/null; then
      echo 'experimental-features = nix-command flakes' | run_root tee -a /etc/nix/nix.conf >/dev/null || true
    fi
  fi

  # Full ownership — partial chown left gc.lock unwritable.
  run_root chown -R "$(id -u):$(id -g)" /nix

  unset NIX_REMOTE || true
  export NIX_REMOTE=""

  if NIX_REMOTE= nix store ping --store local >/dev/null 2>&1 && [[ -w /nix/var/nix ]]; then
    log "single-user store OK (you own /nix; NIX_REMOTE unset)"
    return 0
  fi
  log "single-user conversion failed"
  return 1
}

ensure_nix_daemon_or_single_user() {
  if nix_store_ok; then
    return 0
  fi

  log "nix store not usable; repairing…"
  log "  socket: $([[ -S "$SOCKET" ]] && echo up || echo down)"
  log "  /nix/var/nix writable: $([[ -w /nix/var/nix ]] && echo yes || echo no)"
  log "  sudo: $(have_sudo && echo yes || echo no)"
  log "  uid: $(id -u) ($(id -un))"
  log "  NIX_REMOTE=${NIX_REMOTE:-<unset>}"

  if ! have_sudo && [[ "$(id -u)" -ne 0 ]]; then
    log "need passwordless sudo to repair nix"
    return 1
  fi

  # 1) Try multi-user daemon
  if command -v systemctl >/dev/null 2>&1; then
    run_root systemctl daemon-reload 2>/dev/null || true
    run_root systemctl enable --now nix-daemon.socket 2>/dev/null || true
    run_root systemctl start nix-daemon.socket 2>/dev/null || true
    run_root systemctl start nix-daemon.service 2>/dev/null || true
    run_root systemctl start nix-daemon 2>/dev/null || true
    sleep 1
  fi

  if [[ ! -S "$SOCKET" ]]; then
    start_daemon_manual || true
  fi

  load_nix_env
  if [[ -S "$SOCKET" ]]; then
    export NIX_REMOTE=daemon
    if nix store ping --store daemon >/dev/null 2>&1; then
      # Daemon is up. Still verify we won't hit local locks by forcing remote.
      if NIX_REMOTE=daemon nix store ping >/dev/null 2>&1; then
        log "multi-user daemon OK"
        return 0
      fi
    fi
  fi

  # 2) Codespaces: multi-user is flaky without real systemd — go single-user.
  log "daemon path unreliable; using single-user ownership fallback"
  convert_to_single_user || return 1
  load_nix_env
  unset NIX_REMOTE || true
  nix_store_ok
}

# Persist env so plain `nix develop` (not only ./scripts/enter) works.
install_bashrc_nix_env() {
  local block
  block=$(
    cat <<'EOF'
# sleek-nix-env
if [ -e /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh ]; then
  . /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh
fi
export PATH="/nix/var/nix/profiles/default/bin:${HOME}/.nix-profile/bin:${PATH}"
if [ -S /nix/var/nix/daemon-socket/socket ]; then
  export NIX_REMOTE=daemon
else
  unset NIX_REMOTE
fi
EOF
  )

  touch "$HOME/.bashrc"
  if grep -qF "$BASHRC_NIX_MARKER" "$HOME/.bashrc" 2>/dev/null; then
    # Refresh block (remove old marker section roughly).
    local tmp
    tmp="$(mktemp)"
    # Drop previous sleek-nix-env / sleek-nix-profile / sleek-nix-single-user lines
    grep -vF 'sleek-nix-env' "$HOME/.bashrc" \
      | grep -vF 'sleek-nix-profile' \
      | grep -vF 'sleek-nix-single-user' \
      | grep -vF 'NIX_REMOTE=daemon' \
      | grep -vF 'unset NIX_REMOTE' \
      | grep -vF 'nix-daemon.sh' \
      | grep -vF '/nix/var/nix/profiles/default/bin' \
      >"$tmp" || true
    mv "$tmp" "$HOME/.bashrc"
  fi
  {
    echo ""
    echo "$block"
  } >>"$HOME/.bashrc"
  log "wrote nix env block to ~/.bashrc"
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
  exit 1
fi

if ! nix_store_ok; then
  ensure_nix_daemon_or_single_user || true
  load_nix_env
fi

# On Codespaces, prefer single-user if daemon still leaves local lock issues.
# Detect: socket up but /nix/var/nix not writable and NIX_REMOTE not honored.
if [[ -S "$SOCKET" ]] && [[ ! -w /nix/var/nix ]]; then
  export NIX_REMOTE=daemon
  if ! nix store ping --store daemon >/dev/null 2>&1; then
    convert_to_single_user || true
  fi
fi

install_bashrc_nix_env

# Flakes + new CLI in user conf
mkdir -p "$HOME/.config/nix"
if [[ ! -f "$HOME/.config/nix/nix.conf" ]] || ! grep -q 'experimental-features' "$HOME/.config/nix/nix.conf" 2>/dev/null; then
  echo "experimental-features = nix-command flakes" >>"$HOME/.config/nix/nix.conf"
fi

# Apply mode for this process
if [[ -S "$SOCKET" ]] && nix store ping --store daemon >/dev/null 2>&1; then
  export NIX_REMOTE=daemon
  MODE="daemon"
else
  unset NIX_REMOTE || true
  MODE="single-user"
fi

if ! nix_store_ok; then
  echo "sleek-bootstrap: nix is installed but cannot talk to the store." >&2
  echo "  Socket: $SOCKET  (socket=$([[ -S $SOCKET ]] && echo yes || echo no))" >&2
  echo "  /nix/var/nix writable: $([[ -w /nix/var/nix ]] && echo yes || echo no)" >&2
  echo "  NIX_REMOTE=${NIX_REMOTE:-<unset>}" >&2
  echo "  Try:  sudo chown -R \"\$(id -u):\$(id -g)\" /nix && unset NIX_REMOTE" >&2
  echo "  Or:   sudo nix daemon &  && export NIX_REMOTE=daemon" >&2
  if [[ -s "$DAEMON_LOG" ]]; then
    tail -n 20 "$DAEMON_LOG" >&2 || true
  fi
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

# ── warm the flake ───────────────────────────────────────────────────
if [[ "${SLEEK_SKIP_FLAKE_WARM:-}" != "1" ]]; then
  if nix_store_ok; then
    log "warming flake devShell (nix develop -c true) mode=$MODE …"
    if nix develop "$ROOT" -c true; then
      log "flake ready"
    else
      log "flake warm failed (network?). You can still run: ./scripts/enter"
    fi
  else
    log "skipping flake warm — nix store unreachable"
  fi
fi

log "done. mode=$MODE  NIX_REMOTE=${NIX_REMOTE:-<unset>}"
log "SSH: gh codespace ssh  →  auto nix shell (or ./scripts/enter)"
log "opt out: SLEEK_NO_AUTO_NIX=1"

if ! nix_store_ok; then
  log "FAILED: store still broken."
  exit 1
fi

log "nix store OK ($(nix --version 2>/dev/null | head -1))"
