#!/usr/bin/env bash
# Ensure flakes + nix-command are always on so bare `nix develop` / `nix build`
# work without --extra-experimental-features flags.
#
# Safe to source or exec repeatedly. Prefer:
#   1) /etc/nix/nix.conf (system)
#   2) ~/.config/nix/nix.conf (user)
#   3) NIX_CONFIG env (process)
#
# Usage:
#   bash scripts/ensure-nix-flakes.sh   # write confs
#   . scripts/ensure-nix-flakes.sh && ensure_nix_flakes

_sleek_have_sudo() {
  command -v sudo >/dev/null 2>&1 || return 1
  sudo -n true 2>/dev/null
}

_sleek_run_root() {
  if [[ "$(id -u)" -eq 0 ]]; then
    "$@"
  elif _sleek_have_sudo; then
    sudo "$@"
  else
    return 1
  fi
}

# Upsert a key = value line in a nix.conf (preserves other keys).
_sleek_upsert_nix_conf() {
  local file="$1"
  local key="$2"
  local value="$3"
  local line="${key} = ${value}"
  local dir
  dir="$(dirname "$file")"

  if [[ "$file" == /etc/* ]]; then
    _sleek_run_root mkdir -p "$dir" || return 1
    if [[ -f "$file" ]] && grep -qE "^[[:space:]]*${key}[[:space:]]*=" "$file" 2>/dev/null; then
      _sleek_run_root sed -i -E "s|^[[:space:]]*${key}[[:space:]]*=.*|${line}|" "$file" || return 1
    else
      printf '%s\n' "$line" | _sleek_run_root tee -a "$file" >/dev/null || return 1
    fi
  else
    mkdir -p "$dir" || return 1
    touch "$file" || return 1
    if grep -qE "^[[:space:]]*${key}[[:space:]]*=" "$file" 2>/dev/null; then
      local tmp
      tmp="$(mktemp)" || return 1
      sed -E "s|^[[:space:]]*${key}[[:space:]]*=.*|${line}|" "$file" >"$tmp" || return 1
      mv "$tmp" "$file" || return 1
    else
      printf '%s\n' "$line" >>"$file" || return 1
    fi
  fi
}

ensure_nix_flakes() {
  # Process env — works even if conf files are unwritable this session.
  # Stock Nix reads experimental-features; Determinate also honors
  # extra-experimental-features. Set both so neither needs CLI flags.
  local _want=$'experimental-features = nix-command flakes\nextra-experimental-features = nix-command flakes\naccept-flake-config = true'
  if [[ -z "${NIX_CONFIG:-}" ]]; then
    export NIX_CONFIG="$_want"
  else
    case "${NIX_CONFIG}" in
      *experimental-features*) ;;
      *) export NIX_CONFIG="${NIX_CONFIG}"$'\n'"${_want}" ;;
    esac
  fi

  # User conf
  _sleek_upsert_nix_conf "${HOME}/.config/nix/nix.conf" "experimental-features" "nix-command flakes" || true
  _sleek_upsert_nix_conf "${HOME}/.config/nix/nix.conf" "extra-experimental-features" "nix-command flakes" || true
  _sleek_upsert_nix_conf "${HOME}/.config/nix/nix.conf" "accept-flake-config" "true" || true

  # System conf (Codespaces with passwordless sudo)
  if [[ -w /etc/nix/nix.conf ]] || _sleek_have_sudo || [[ "$(id -u)" -eq 0 ]]; then
    _sleek_upsert_nix_conf /etc/nix/nix.conf "experimental-features" "nix-command flakes" || true
    _sleek_upsert_nix_conf /etc/nix/nix.conf "extra-experimental-features" "nix-command flakes" || true
    _sleek_upsert_nix_conf /etc/nix/nix.conf "accept-flake-config" "true" || true
  fi
}

# If executed (not sourced), run ensure under strict mode.
if [[ "${BASH_SOURCE[0]:-}" == "${0}" ]]; then
  set -euo pipefail
  ensure_nix_flakes
fi
