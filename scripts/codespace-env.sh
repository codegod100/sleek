# shellcheck shell=bash
# Login/env shim for Codespaces + local SSH.
#
# Source from bashrc (bootstrap installs the hook). When you land in the
# sleek workspace after `gh codespace ssh`, this re-execs into `nix develop`
# once so cargo/just/rustc match the flake.
#
# Safe to source repeatedly: skips if already in the shell or if nix is missing.

# VNC OAuth browser (safe for non-interactive too).
_sleek_env_src_early="${BASH_SOURCE[0]:-}"
if [[ -n "$_sleek_env_src_early" ]]; then
  _sleek_root_early="$(cd "$(dirname "$_sleek_env_src_early")/.." && pwd 2>/dev/null || true)"
  if [[ -n "${_sleek_root_early:-}" && -x "${_sleek_root_early}/scripts/vnc-browser.sh" ]]; then
    export BROWSER="${BROWSER:-${_sleek_root_early}/scripts/vnc-browser.sh}"
    export PATH="${HOME}/.nix-profile/bin:${PATH:-}"
  fi
  unset _sleek_root_early
fi
unset _sleek_env_src_early

# Only for interactive shells (not scp/sftp/CI scripts).
case $- in
  *i*) ;;
  *) return 0 2>/dev/null || exit 0 ;;
esac

# Already loaded.
if [[ -n "${SLEEK_NIX_SHELL:-}" || -n "${IN_NIX_SHELL:-}" ]]; then
  return 0 2>/dev/null || true
fi

# Resolve repo root: this file lives at scripts/codespace-env.sh
_sleek_env_src="${BASH_SOURCE[0]:-}"
if [[ -z "$_sleek_env_src" ]]; then
  return 0 2>/dev/null || true
fi
_sleek_root="$(cd "$(dirname "$_sleek_env_src")/.." && pwd 2>/dev/null || true)"
unset _sleek_env_src

if [[ -z "${_sleek_root:-}" || ! -f "${_sleek_root}/flake.nix" ]]; then
  unset _sleek_root
  return 0 2>/dev/null || true
fi

# Only auto-enter when cwd is inside the sleek repo (or Codespaces workspace root).
_sleek_cwd="$(pwd -P 2>/dev/null || pwd)"
case "${_sleek_cwd}/" in
  "${_sleek_root}/"* | /workspaces/*/)
    # Codespaces often starts in /workspaces/<name> which is the repo root.
    if [[ "${_sleek_cwd}" != "${_sleek_root}" && "${_sleek_cwd}"/ != "${_sleek_root}"/* ]]; then
      # Allow /workspaces/<repo> when it is this checkout.
      if [[ ! -f "${_sleek_cwd}/flake.nix" ]]; then
        unset _sleek_root _sleek_cwd
        return 0 2>/dev/null || true
      fi
      _sleek_root="${_sleek_cwd}"
    fi
    ;;
  *)
    unset _sleek_root _sleek_cwd
    return 0 2>/dev/null || true
    ;;
esac

# shellcheck disable=SC1091
if [[ -f /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh ]]; then
  # shellcheck disable=SC1091
  . /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh
elif [[ -f "${HOME}/.nix-profile/etc/profile.d/nix.sh" ]]; then
  # shellcheck disable=SC1091
  . "${HOME}/.nix-profile/etc/profile.d/nix.sh"
fi
export PATH="/nix/var/nix/profiles/default/bin:${HOME}/.nix-profile/bin:${PATH}"

# Flakes without flags (user conf + NIX_CONFIG; no sudo required at login).
if [[ -f "${_sleek_root}/scripts/ensure-nix-flakes.sh" ]]; then
  # shellcheck disable=SC1091
  . "${_sleek_root}/scripts/ensure-nix-flakes.sh"
  ensure_nix_flakes
fi

# Prefer direnv when available (non-exec path; keeps nested shells happy).
if command -v direnv >/dev/null 2>&1 && [[ -f "${_sleek_root}/.envrc" ]]; then
  # direnv hook should already be in bashrc; force a reload if needed.
  if [[ -z "${DIRENV_DIR:-}" ]]; then
    eval "$(cd "${_sleek_root}" && direnv export bash 2>/dev/null)" || true
  fi
  if [[ -n "${IN_NIX_SHELL:-}" || -n "${DIRENV_DIR:-}" ]]; then
    unset _sleek_root _sleek_cwd
    return 0 2>/dev/null || true
  fi
fi

if ! command -v nix >/dev/null 2>&1; then
  # Soft hint once per shell; do not block login if bootstrap has not run.
  if [[ -z "${SLEEK_NIX_HINT:-}" ]]; then
    export SLEEK_NIX_HINT=1
    echo "sleek: nix not on PATH — run: bash ${_sleek_root}/scripts/codespace-bootstrap.sh" >&2
  fi
  unset _sleek_root _sleek_cwd
  return 0 2>/dev/null || true
fi

# Opt out: SLEEK_NO_AUTO_NIX=1 gh codespace ssh ...
if [[ -n "${SLEEK_NO_AUTO_NIX:-}" ]]; then
  unset _sleek_root _sleek_cwd
  return 0 2>/dev/null || true
fi

# Multi-user install without daemon: try a quick start, else hint (do not hang login).
if [[ -d /nix/var/nix/daemon-socket ]] && [[ ! -S /nix/var/nix/daemon-socket/socket ]]; then
  if command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then
    if [[ -x /nix/var/nix/profiles/default/bin/nix-daemon ]]; then
      sudo bash -c 'setsid /nix/var/nix/profiles/default/bin/nix-daemon --daemon >>/tmp/nix-daemon.log 2>&1 < /dev/null &' 2>/dev/null || true
    else
      sudo bash -c 'setsid nix daemon >>/tmp/nix-daemon.log 2>&1 < /dev/null &' 2>/dev/null || true
    fi
    _i=0
    while [[ $_i -lt 20 && ! -S /nix/var/nix/daemon-socket/socket ]]; do
      sleep 0.25
      _i=$((_i + 1))
    done
    unset _i
  fi
fi

if [[ -d /nix/var/nix/daemon-socket ]] && [[ ! -S /nix/var/nix/daemon-socket/socket ]]; then
  if [[ -z "${SLEEK_NIX_HINT:-}" ]]; then
    export SLEEK_NIX_HINT=1
    echo "sleek: nix-daemon not running — run: bash ${_sleek_root}/scripts/codespace-bootstrap.sh" >&2
    echo "  or:  sudo nix daemon &" >&2
  fi
  unset _sleek_root _sleek_cwd
  return 0 2>/dev/null || true
fi

# Daemon if socket is up; else single-user (must own /nix). Never leave NIX_REMOTE
# pointing at a dead multi-user layout — that yields gc.lock Permission denied.
if [[ -S /nix/var/nix/daemon-socket/socket ]]; then
  export NIX_REMOTE=daemon
else
  unset NIX_REMOTE || true
  if [[ ! -w /nix/var/nix ]]; then
    if [[ -z "${SLEEK_NIX_HINT:-}" ]]; then
      export SLEEK_NIX_HINT=1
      echo "sleek: nix store not usable — run: bash ${_sleek_root}/scripts/codespace-bootstrap.sh" >&2
    fi
    unset _sleek_root _sleek_cwd
    return 0 2>/dev/null || true
  fi
fi

export SLEEK_NIX_SHELL=1
cd "${_sleek_root}" || true
unset _sleek_cwd
# Replace this interactive shell with the flake shell (args: none → user shell).
exec nix develop "${_sleek_root}" --command "${SHELL:-bash}"
