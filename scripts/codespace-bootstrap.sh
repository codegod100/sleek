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

# Cloud / Codespace clones only get `origin`. Patch publish needs `rad`.
if [[ -x "$ROOT/scripts/ensure-rad-remote.sh" ]]; then
  bash "$ROOT/scripts/ensure-rad-remote.sh" || log "ensure-rad-remote failed (non-fatal)"
fi

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
  if [[ -f /etc/nix/nix.conf ]] || [[ -d /etc/nix ]]; then
    run_root mkdir -p /etc/nix || true
    if [[ -f /etc/nix/nix.conf ]] && grep -qE '^build-users-group' /etc/nix/nix.conf 2>/dev/null; then
      run_root sed -i 's/^build-users-group.*/build-users-group =/' /etc/nix/nix.conf || true
    else
      echo 'build-users-group =' | run_root tee -a /etc/nix/nix.conf >/dev/null || true
    fi
  fi
  # shellcheck disable=SC1091
  . "$ROOT/scripts/ensure-nix-flakes.sh"
  ensure_nix_flakes

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
    cat <<EOF
# sleek-nix-env
if [ -e /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh ]; then
  . /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh
fi
export PATH="/nix/var/nix/profiles/default/bin:\${HOME}/.nix-profile/bin:\${PATH}"
if [ -S /nix/var/nix/daemon-socket/socket ]; then
  export NIX_REMOTE=daemon
else
  unset NIX_REMOTE
fi
# flakes / nix-command without CLI flags
export NIX_CONFIG="experimental-features = nix-command flakes
extra-experimental-features = nix-command flakes"
if [ -f "$ROOT/scripts/ensure-nix-flakes.sh" ]; then
  . "$ROOT/scripts/ensure-nix-flakes.sh"
  ensure_nix_flakes 2>/dev/null || true
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

# Flakes + nix-command always on (no --extra-experimental-features flags needed).
# shellcheck disable=SC1091
. "$ROOT/scripts/ensure-nix-flakes.sh"
ensure_nix_flakes
log "nix flakes enabled (user + system conf + NIX_CONFIG)"

# ── Cachix: codegod100 (pull always; push when CACHIX_AUTH_TOKEN is set) ─
# Public cache: https://codegod100.cachix.org
# Push auth: set GitHub Codespaces secret CACHIX_AUTH_TOKEN (personal/org/repo).
CACHIX_CACHE="${CACHIX_CACHE:-codegod100}"
CACHIX_SUBSTITUTER="https://${CACHIX_CACHE}.cachix.org"
CACHIX_PUBKEY="codegod100.cachix.org-1:LZFL5VrR644WUjleS3bLbVeOdzlXqzKznQWvD5MVthA="

ensure_nix_conf_kv() {
  # ensure_nix_conf_kv FILE KEY VALUE
  # Appends or merges space-separated values for KEY without duplicating VALUE.
  # FILE may be root-owned (uses run_root when not writable).
  local file="$1" key="$2" value="$3"
  local line cur tmp dir as_root=0
  dir="$(dirname "$file")"

  if [[ ! -w "$dir" ]] || { [[ -e "$file" ]] && [[ ! -w "$file" ]]; }; then
    as_root=1
    run_root mkdir -p "$dir" || return 1
    run_root touch "$file" || return 1
  else
    mkdir -p "$dir" || return 1
    touch "$file" || return 1
  fi

  # Work on a user-writable copy, then install back.
  tmp="$(mktemp)" || return 1
  if [[ "$as_root" -eq 1 ]]; then
    run_root cat "$file" >"$tmp" || { rm -f "$tmp"; return 1; }
  else
    cat "$file" >"$tmp" || { rm -f "$tmp"; return 1; }
  fi

  if grep -qE "^${key}[[:space:]]*=" "$tmp" 2>/dev/null; then
    line="$(grep -E "^${key}[[:space:]]*=" "$tmp" | tail -1)"
    cur="${line#*=}"
    cur="${cur#"${cur%%[![:space:]]*}"}"
    case " $cur " in
      *" $value "*)
        rm -f "$tmp"
        return 0
        ;;
      *)
        grep -vE "^${key}[[:space:]]*=" "$tmp" >"${tmp}.new" || true
        echo "${key} = ${cur} ${value}" >>"${tmp}.new"
        mv "${tmp}.new" "$tmp"
        ;;
    esac
  else
    echo "${key} = ${value}" >>"$tmp"
  fi

  if [[ "$as_root" -eq 1 ]]; then
    run_root cp "$tmp" "$file" || { rm -f "$tmp"; return 1; }
    run_root chmod 644 "$file" || true
  else
    mv "$tmp" "$file" || { rm -f "$tmp"; return 1; }
  fi
  rm -f "$tmp"
}

# Restart nix-daemon so trusted-substituters / public-keys take effect.
reload_nix_daemon_if_needed() {
  if ! have_sudo && [[ "$(id -u)" -ne 0 ]]; then
    return 0
  fi
  log "reloading nix-daemon to pick up substituter trust…"
  if command -v systemctl >/dev/null 2>&1; then
    if run_root systemctl restart nix-daemon.service 2>/dev/null \
      || run_root systemctl restart nix-daemon 2>/dev/null; then
      sleep 1
      load_nix_env
      return 0
    fi
  fi
  # No systemd (Cursor cloud / Codespaces with --init none): kill + relaunch.
  run_root pkill -x nix-daemon 2>/dev/null || true
  sleep 0.5
  run_root rm -f "$SOCKET" 2>/dev/null || true
  start_daemon_manual || true
  load_nix_env
}

configure_cachix_pull() {
  # User-level conf works for single-user installs and as a client hint.
  NIX_USER_CONF="${NIX_USER_CONF:-$HOME/.config/nix/nix.conf}"
  mkdir -p "$(dirname "$NIX_USER_CONF")"
  ensure_nix_conf_kv "$NIX_USER_CONF" "extra-substituters" "$CACHIX_SUBSTITUTER"

  # Multi-user / Determinate: non-trusted users (trusted-users = root only) may
  # only use substituters listed in trusted-substituters. Flake nixConfig
  # extra-substituters alone is ignored with:
  #   warning: ignoring untrusted substituter '…cachix.org'…
  # Determinate overwrites /etc/nix/nix.conf — put durable settings in
  # nix.custom.conf (included via !include).
  #
  # Do NOT put extra-trusted-public-keys in the user conf on multi-user Nix:
  # that setting is restricted and yields noisy warnings for non-trusted users.
  # Also leave accept-flake-config off once the daemon trusts the cache — accepting
  # flake nixConfig would re-apply restricted trusted-public-keys and warn.
  local multi_user=0
  if [[ -S "$SOCKET" ]] || grep -qE '^trusted-users' /etc/nix/nix.conf 2>/dev/null; then
    multi_user=1
  fi

  if have_sudo || [[ "$(id -u)" -eq 0 ]]; then
    run_root mkdir -p /etc/nix 2>/dev/null || true
    local sys_conf="/etc/nix/nix.conf"
    # Prefer Determinate's custom conf when the main conf includes it.
    if [[ -f /etc/nix/nix.custom.conf ]] \
      || grep -qF 'nix.custom.conf' /etc/nix/nix.conf 2>/dev/null; then
      sys_conf="/etc/nix/nix.custom.conf"
      run_root touch "$sys_conf" 2>/dev/null || true
    else
      run_root touch "$sys_conf" 2>/dev/null || true
    fi

    ensure_nix_conf_kv "$sys_conf" "extra-substituters" "$CACHIX_SUBSTITUTER" || true
    # Critical: allow non-trusted users to pull from this cache.
    ensure_nix_conf_kv "$sys_conf" "extra-trusted-substituters" "$CACHIX_SUBSTITUTER" || true
    ensure_nix_conf_kv "$sys_conf" "extra-trusted-public-keys" "$CACHIX_PUBKEY" || true
    log "added $CACHIX_CACHE as trusted substituter in $sys_conf"
    # Drop legacy user-conf public key / flake-accept (causes restricted-setting warnings).
    if [[ -f "$NIX_USER_CONF" ]]; then
      local tmp
      tmp="$(mktemp)"
      grep -vE '^(extra-trusted-public-keys|accept-flake-config)[[:space:]]*=' "$NIX_USER_CONF" >"$tmp" || true
      mv "$tmp" "$NIX_USER_CONF"
    fi
    reload_nix_daemon_if_needed || true
  elif [[ "$multi_user" -eq 0 ]]; then
    # Single-user: client conf can hold the public key + accept flake nixConfig.
    ensure_nix_conf_kv "$NIX_USER_CONF" "extra-trusted-public-keys" "$CACHIX_PUBKEY"
    if ! grep -qE '^accept-flake-config' "$NIX_USER_CONF" 2>/dev/null; then
      echo "accept-flake-config = true" >>"$NIX_USER_CONF"
    fi
  fi
  log "nix pull configured: $CACHIX_SUBSTITUTER"
}

install_cachix_cli() {
  if command -v cachix >/dev/null 2>&1; then
    return 0
  fi
  if ! nix_store_ok; then
    log "skipping cachix install (nix store not ready)"
    return 1
  fi
  log "installing cachix via nix profile…"
  if nix profile install nixpkgs#cachix 2>/dev/null \
    || nix-env -iA nixpkgs.cachix 2>/dev/null; then
    load_nix_env
    log "cachix installed"
    return 0
  fi
  log "could not install cachix (optional for push)"
  return 1
}

configure_cachix_push() {
  # Prefer env (Codespaces secret). Never echo the token.
  if [[ -z "${CACHIX_AUTH_TOKEN:-}" ]]; then
    log "CACHIX_AUTH_TOKEN unset — pull-only (no push to $CACHIX_CACHE)"
    log "  Add Codespace secret CACHIX_AUTH_TOKEN to enable: cachix push $CACHIX_CACHE"
    return 0
  fi
  if ! command -v cachix >/dev/null 2>&1; then
    log "cachix CLI missing; cannot store authtoken"
    return 1
  fi
  # Writes ~/.config/cachix/cachix.dhall — not printed.
  if printf '%s' "$CACHIX_AUTH_TOKEN" | cachix authtoken --stdin >/dev/null 2>&1; then
    log "cachix authtoken configured — auto-push on: just android"
    log "  (manual: cachix push $CACHIX_CACHE <paths>; opt out: SLEEK_CACHIX_PUSH=0)"
  else
    # Older/newer CLI flag variants
    if printf '%s' "$CACHIX_AUTH_TOKEN" | cachix authtoken >/dev/null 2>&1; then
      log "cachix authtoken configured — auto-push on: just android"
      log "  (manual: cachix push $CACHIX_CACHE <paths>; opt out: SLEEK_CACHIX_PUSH=0)"
    else
      log "cachix authtoken failed (token invalid or CLI mismatch)"
      return 1
    fi
  fi
}

configure_cachix_pull
if [[ "${SLEEK_SKIP_CACHIX:-}" != "1" ]]; then
  install_cachix_cli || true
  configure_cachix_push || true
fi

# ── GitHub CLI: prefer PAT from OpenBao over Cursor integration token ─
# Store with: OPENBAO_TOKEN=… ./scripts/openbao-put-key.sh GH_TOKEN --from-gh
if [[ -n "${OPENBAO_TOKEN:-}" && "${SLEEK_SKIP_GH_OPENBAO:-}" != "1" ]]; then
  if [[ -x "$ROOT/scripts/configure-gh-from-openbao.sh" ]]; then
    log "configuring gh from OpenBao GH_TOKEN…"
    bash "$ROOT/scripts/configure-gh-from-openbao.sh" || log "gh OpenBao configure skipped/failed (optional)"
  fi
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

# ── starship prompt ──────────────────────────────────────────────────
if ! command -v starship >/dev/null 2>&1; then
  if nix_store_ok; then
    log "installing starship via nix profile…"
    if nix profile add nixpkgs#starship 2>/dev/null \
      || nix profile install nixpkgs#starship 2>/dev/null \
      || nix-env -iA nixpkgs.starship 2>/dev/null; then
      load_nix_env
      log "starship installed"
    else
      log "could not install starship (optional)"
    fi
  else
    log "skipping starship install (nix store not ready)"
  fi
fi

install_bashrc_starship() {
  local marker="# sleek-starship"
  touch "$HOME/.bashrc"
  if grep -qF "$marker" "$HOME/.bashrc" 2>/dev/null; then
    return 0
  fi
  # Drop the codespace default PS1 so it does not fight starship.
  if grep -qE '^export PS1=' "$HOME/.bashrc" 2>/dev/null; then
    local tmp
    tmp="$(mktemp)"
    grep -vE '^export PS1=' "$HOME/.bashrc" >"$tmp" || true
    mv "$tmp" "$HOME/.bashrc"
  fi
  {
    echo ""
    echo "# nix profile (starship, direnv, …)"
    echo 'export PATH="${HOME}/.nix-profile/bin:/nix/var/nix/profiles/default/bin:${PATH}"'
    echo ""
    echo "$marker"
    echo 'if command -v starship >/dev/null 2>&1; then'
    echo '  eval "$(starship init bash)"'
    echo 'fi'
  } >>"$HOME/.bashrc"
  log "wrote starship init to ~/.bashrc"
}
install_bashrc_starship

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

# ── VNC browser for OAuth (Bluesky) ──────────────────────────────────
# desktop-lite has no browser; apt only ships chromium/firefox snaps (broken
# here). Install Chromium via nix and point $BROWSER at our no-sandbox wrapper.
ensure_vnc_browser() {
  if [[ -z "${SLEEK_CODESPACE:-}${CODESPACE_NAME:-}" && ! -S /tmp/.X11-unix/X1 ]]; then
    return 0
  fi
  local wrapper="$ROOT/scripts/vnc-browser.sh"
  chmod +x "$wrapper" 2>/dev/null || true
  export PATH="${HOME}/.nix-profile/bin:/nix/var/nix/profiles/default/bin:${PATH}"
  if ! command -v chromium >/dev/null 2>&1; then
    if command -v nix >/dev/null 2>&1 && nix_store_ok; then
      log "installing chromium (nix profile) for VNC OAuth…"
      # `nix profile add` is the current command; `install` still works as alias.
      nix profile add nixpkgs#chromium 2>/dev/null \
        || nix profile install nixpkgs#chromium 2>/dev/null \
        || log "chromium install failed — run: nix profile add nixpkgs#chromium"
    fi
  fi
  if [[ -x "$wrapper" ]]; then
    # Persist for login shells (codespace-env also sets this).
    local envline="export BROWSER=\"$wrapper\""
    if ! grep -qF 'scripts/vnc-browser.sh' "$HOME/.bashrc" 2>/dev/null; then
      {
        echo ""
        echo "# sleek VNC browser (OAuth)"
        echo "$envline"
        echo "export PATH=\"\${HOME}/.nix-profile/bin:\${PATH}\""
      } >>"$HOME/.bashrc"
      log "BROWSER → $wrapper"
    fi
    export BROWSER="$wrapper"
  fi
}
ensure_vnc_browser

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
