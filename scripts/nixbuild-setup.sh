#!/usr/bin/env bash
# Configure SSH auth to nixbuild.net. On its own this only makes the host
# reachable/authenticated for `ssh eu.nixbuild.net …` — it does NOT tell Nix
# to actually use it as a builder (no `builders =` / --store is set here), so
# a plain `nix build` afterwards still builds locally. For CI that needs the
# build to actually run on nixbuild.net, use scripts/ci-nixbuild.sh instead
# (its `setup` step configures builders + NIX_CONFIG, and `remote-build`
# streams just the named output files, avoiding a full closure copy — see
# .tangled/workflows/packages.yml). This script is kept for callers that only
# want nixbuild.net as a pull-only substituter.
#
# Uses an auth token (NIXBUILD_TOKEN). On NixOS / multi-user Nix, also writes
# /root/.ssh so nix-daemon can reach the remote builder.
#
# Usage:
#   export NIXBUILD_TOKEN=…
#   ./scripts/nixbuild-setup.sh
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
