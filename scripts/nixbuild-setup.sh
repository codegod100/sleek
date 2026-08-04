#!/usr/bin/env bash
# Configure SSH so Nix can offload builds to nixbuild.net.
#
# Uses an auth token (NIXBUILD_TOKEN). On NixOS / multi-user Nix, also writes
# /root/.ssh so nix-daemon can reach the remote builder.
#
# Usage:
#   export NIXBUILD_TOKEN=…
#   ./scripts/nixbuild-setup.sh
#
# GitHub Actions should use nixbuild/nixbuild-action instead; this script is
# for Spindle/Tangled and other shell-only environments.
set -euo pipefail

TOKEN="${NIXBUILD_TOKEN:-${nixbuild_token:-}}"
HOST="${NIXBUILD_SSH_HOST:-eu.nixbuild.net}"
HOST_KEY="ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPIQCZc54poJ8vqawd8TraNryQeJnvH1eLpIDgbiqymM"

if [[ -z "$TOKEN" ]]; then
  echo "nixbuild-setup: NIXBUILD_TOKEN is not set" >&2
  exit 1
fi

write_ssh_config() {
  local dir="$1"
  mkdir -p "$dir"
  chmod 700 "$dir"
  if ! grep -qF "Host $HOST" "$dir/config" 2>/dev/null; then
    cat >>"$dir/config" <<EOF
Host $HOST
  User authtoken
  PreferredAuthentications none
  SetEnv NIXBUILDNET_TOKEN=$TOKEN
  ServerAliveInterval 60
  PubkeyAcceptedKeyTypes ssh-ed25519

EOF
    chmod 600 "$dir/config"
  else
    # Refresh token in an existing block (idempotent re-runs).
    sed -i "s|^  SetEnv NIXBUILDNET_TOKEN=.*|  SetEnv NIXBUILDNET_TOKEN=$TOKEN|" "$dir/config"
  fi
  if ! grep -qF "$HOST" "$dir/known_hosts" 2>/dev/null; then
    echo "$HOST $HOST_KEY" >>"$dir/known_hosts"
    chmod 644 "$dir/known_hosts"
  fi
}

write_ssh_config "${HOME}/.ssh"

# nix-daemon (root) must see the same config on multi-user installs.
if [[ "$(id -u)" -eq 0 ]]; then
  :
elif command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then
  sudo mkdir -p /root/.ssh
  sudo chmod 700 /root/.ssh
  sudo tee /root/.ssh/config >/dev/null <<EOF
Host $HOST
  User authtoken
  PreferredAuthentications none
  SetEnv NIXBUILDNET_TOKEN=$TOKEN
  ServerAliveInterval 60
  PubkeyAcceptedKeyTypes ssh-ed25519
EOF
  sudo chmod 600 /root/.ssh/config
  if ! sudo grep -qF "$HOST" /root/.ssh/known_hosts 2>/dev/null; then
    echo "$HOST $HOST_KEY" | sudo tee -a /root/.ssh/known_hosts >/dev/null
    sudo chmod 644 /root/.ssh/known_hosts
  fi
fi

echo "nixbuild-setup: configured remote builder at $HOST" >&2
