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

# ── nix ──────────────────────────────────────────────────────────────
if ! command -v nix >/dev/null 2>&1; then
  log "installing nix (Determinate installer, multi-user)…"
  if [[ "$(id -u)" -eq 0 ]]; then
    curl --proto '=https' --tlsv1.2 -sSf -L https://install.determinate.systems/nix \
      | sh -s -- install --no-confirm
  else
    # Codespaces remoteUser is usually non-root with passwordless sudo.
    curl --proto '=https' --tlsv1.2 -sSf -L https://install.determinate.systems/nix \
      | sh -s -- install --no-confirm
  fi
  # Load nix into this script's environment.
  # shellcheck disable=SC1091
  if [[ -f /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh ]]; then
    . /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh
  elif [[ -f "$HOME/.nix-profile/etc/profile.d/nix.sh" ]]; then
    . "$HOME/.nix-profile/etc/profile.d/nix.sh"
  fi
fi

if ! command -v nix >/dev/null 2>&1; then
  echo "sleek-bootstrap: nix still not on PATH after install" >&2
  exit 1
fi

# Flakes + new CLI (no-op if already set by Determinate).
mkdir -p "$HOME/.config/nix"
if [[ ! -f "$HOME/.config/nix/nix.conf" ]] || ! grep -q 'experimental-features' "$HOME/.config/nix/nix.conf" 2>/dev/null; then
  {
    echo "experimental-features = nix-command flakes"
  } >>"$HOME/.config/nix/nix.conf"
fi

# ── direnv ───────────────────────────────────────────────────────────
if ! command -v direnv >/dev/null 2>&1; then
  log "installing direnv via nix profile…"
  nix profile install nixpkgs#direnv 2>/dev/null \
    || nix-env -iA nixpkgs.direnv 2>/dev/null \
    || log "could not install direnv (optional); login shim will use nix develop"
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
      # Replace old hook block
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
  # Trust this repo's .envrc (non-interactive allow).
  (cd "$ROOT" && direnv allow .) 2>/dev/null || true
fi

# ── warm the flake (best-effort; speeds first `enter`) ───────────────
if [[ "${SLEEK_SKIP_FLAKE_WARM:-}" != "1" ]]; then
  log "warming flake devShell (nix develop -c true)…"
  if nix develop "$ROOT" -c true; then
    log "flake ready"
  else
    log "flake warm failed (network?). You can still run: ./scripts/enter"
  fi
fi

log "done. SSH: gh codespace ssh  →  auto nix shell (or ./scripts/enter)"
log "opt out: SLEEK_NO_AUTO_NIX=1"
