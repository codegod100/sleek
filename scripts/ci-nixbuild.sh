#!/usr/bin/env bash
# Buildkite (and local CI) helper mirroring nixbuild/nixbuild-action setup.
#
# Subcommands:
#   resolve-creds   Soft-load NIXBUILD_TOKEN (Buildkite secret / OpenBao).
#                   Writes an env file path on stdout as NIXBUILD_CREDS_ENV=...
#                   and SKIP=0|1. Never prints the token.
#   setup           Install/configure Nix + SSH to eu.nixbuild.net; requires token.
#   remote-build ATTR [OUT_LINK] [ARTIFACT_REL …]
#                   nix build ATTR on ssh-ng://nixbuild (→ eu.nixbuild.net).
#                   If ARTIFACT_REL args are given, stream those files out of
#                   the remote store path with `nix store cat` (no full closure
#                   copy — flatpak outs reference multi‑GiB runtimes that OOM
#                   hosted 2x4 agents). Otherwise `nix copy` the out path and
#                   symlink OUT_LINK (default: result).
#
# Env:
#   NIXBUILD_TOKEN / NIXBUILDNET_TOKEN — auth token (never logged)
#   OPENBAO_TOKEN — optional; used to fetch NIXBUILD_TOKEN via fetch-openbao-env.sh
#   NIXBUILD_SSH_HOST — default eu.nixbuild.net
#
# Docs: https://docs.nixbuild.net/remote-builds/
# GHA equivalent: nixbuild/nix-quick-install-action + nixbuild/nixbuild-action
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NIXBUILD_SSH_HOST="${NIXBUILD_SSH_HOST:-eu.nixbuild.net}"
# Host key from nixbuild/nixbuild-action (default ssh_public_host_key).
NIXBUILD_HOST_KEY='ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPIQCZc54poJ8vqawd8TraNryQeJnvH1eLpIDgbiqymM'

usage() {
  sed -n '2,22p' "$0"
}

resolve_creds() {
  echo "--- :lock: Resolve nixbuild credentials" >&2

  if command -v buildkite-agent >/dev/null 2>&1; then
    if [[ -z "${NIXBUILD_TOKEN:-}" ]]; then
      if nt="$(buildkite-agent secret get NIXBUILD_TOKEN 2>/dev/null || true)"; then
        if [[ -n "$nt" ]]; then
          export NIXBUILD_TOKEN="$nt"
          echo "Loaded NIXBUILD_TOKEN from Buildkite secret" >&2
        fi
      fi
    else
      echo "NIXBUILD_TOKEN already present in job env" >&2
    fi
    if [[ -z "${OPENBAO_TOKEN:-}" ]]; then
      if ot="$(buildkite-agent secret get OPENBAO_TOKEN 2>/dev/null || true)"; then
        if [[ -n "$ot" ]]; then
          export OPENBAO_TOKEN="$ot"
          echo "Loaded OPENBAO_TOKEN from Buildkite secret" >&2
        fi
      fi
    fi
  fi

  if [[ -z "${NIXBUILD_TOKEN:-}" && -n "${OPENBAO_TOKEN:-}" ]]; then
    echo "Fetching NIXBUILD_TOKEN from OpenBao via OPENBAO_TOKEN" >&2
    chmod +x "$ROOT/scripts/fetch-openbao-env.sh"
    # shellcheck disable=SC1091
    eval "$("$ROOT/scripts/fetch-openbao-env.sh" --export --keys NIXBUILD_TOKEN)"
  fi

  if [[ -z "${NIXBUILD_TOKEN:-}" ]]; then
    echo "NIXBUILD_TOKEN and OPENBAO_TOKEN are both unset in this job." >&2
    echo "Create a Buildkite cluster secret (do not invent tokens — copy from OpenBao/nixbuild):" >&2
    echo "  UI: https://buildkite.com/organizations/nandi/clusters → Default cluster → Secrets" >&2
    echo "  Keys: NIXBUILD_TOKEN  (preferred)  or  OPENBAO_TOKEN  (fetches NIXBUILD_TOKEN from OpenBao)" >&2
    echo "  Scope policy example: pipeline_slug sleek" >&2
    echo "SKIP=1"
    return 0
  fi

  export NIXBUILDNET_TOKEN="${NIXBUILD_TOKEN}"
  creds_env="$(mktemp)"
  umask 077
  {
    printf 'export NIXBUILD_TOKEN=%q\n' "$NIXBUILD_TOKEN"
    printf 'export NIXBUILDNET_TOKEN=%q\n' "$NIXBUILDNET_TOKEN"
  } >"$creds_env"
  chmod 600 "$creds_env"
  echo "NIXBUILD_TOKEN is set — nixbuild.net auth available" >&2
  echo "NIXBUILD_CREDS_ENV=$creds_env"
  echo "SKIP=0"
}

ensure_nix() {
  if ! command -v nix >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -sSf -L \
      https://install.determinate.systems/nix \
      | sh -s -- install linux --no-confirm --init none
    # shellcheck disable=SC1091
    . /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh || true
    export PATH="/nix/var/nix/profiles/default/bin:$PATH"
  fi
  command -v nix >/dev/null 2>&1 || {
    echo "nix not on PATH after install" >&2
    exit 1
  }
  if ! command -v jq >/dev/null 2>&1; then
    if command -v apt-get >/dev/null 2>&1; then
      sudo DEBIAN_FRONTEND=noninteractive apt-get update
      sudo DEBIAN_FRONTEND=noninteractive apt-get install -y jq
    fi
  fi
  command -v jq >/dev/null 2>&1 || {
    echo "jq required to read nixbuild signing key" >&2
    exit 1
  }
}

setup_nixbuild() {
  : "${NIXBUILDNET_TOKEN:?NIXBUILDNET_TOKEN required (run resolve-creds first)}"
  ensure_nix

  echo "--- :nixbuild: configure SSH + Nix for $NIXBUILD_SSH_HOST"

  ssh_known_hosts="$(mktemp)"
  ssh_config="$(mktemp)"
  builders_file="$(mktemp)"

  {
    echo "eu.nixbuild.net $NIXBUILD_HOST_KEY"
    echo "nixbuild $NIXBUILD_HOST_KEY"
  } >"$ssh_known_hosts"

  setenv_line="NIXBUILDNET_TOKEN=${NIXBUILDNET_TOKEN} token=${NIXBUILDNET_TOKEN}"
  if [[ -n "${BUILDKITE_BUILD_NUMBER:-}" ]]; then
    setenv_line+=" NIXBUILDNET_TAG_BUILDKITE_BUILD_NUMBER=${BUILDKITE_BUILD_NUMBER}"
    setenv_line+=" NIXBUILDNET_TAG_BUILDKITE_PIPELINE_SLUG=${BUILDKITE_PIPELINE_SLUG:-}"
    setenv_line+=" NIXBUILDNET_TAG_BUILDKITE_COMMIT=${BUILDKITE_COMMIT:-}"
  fi

  # Mirror nixbuild/nixbuild-action SSH config (Host alias + SetEnv token).
  {
    echo "Host nixbuild eu.nixbuild.net"
    echo "  HostName $NIXBUILD_SSH_HOST"
    echo "  HostKeyAlias nixbuild"
    echo "  LogLevel ERROR"
    echo "  StrictHostKeyChecking yes"
    echo "  UserKnownHostsFile $ssh_known_hosts"
    echo "  ControlPath none"
    echo "  ServerAliveInterval 60"
    echo "  IPQoS throughput"
    echo "  PreferredAuthentications none"
    echo "  User authtoken"
    echo "  SendEnv NIXBUILDNET_TOKEN"
    # Legacy token= + documented NIXBUILDNET_TOKEN (docs.nixbuild.net/access-control).
    echo "  SetEnv $setenv_line"
  } >"$ssh_config"
  chmod 600 "$ssh_config" "$ssh_known_hosts"
  export NIX_SSHOPTS="-F$ssh_config"

  # Root must share SSH auth so nix-daemon remote ops authenticate.
  sudo mkdir -p /root/.ssh
  sudo cp "$ssh_config" /root/.ssh/config
  sudo cp "$ssh_known_hosts" /root/.ssh/known_hosts
  sudo chmod 700 /root/.ssh
  sudo chmod 600 /root/.ssh/config /root/.ssh/known_hosts

  echo "--- :nixbuild: fetch signing key (auth liveness)"
  nixbuild_pubkey="$(
    ssh -F "$ssh_config" nixbuild api settings signing-key-for-builds --show \
      | jq -r '"\(.keyName):\(.publicKey)"'
  )"
  [[ -n "$nixbuild_pubkey" && "$nixbuild_pubkey" != ":" ]] || {
    echo "failed to read nixbuild signing key (SSH/auth problem?)" >&2
    exit 1
  }
  echo "nixbuild substituter pubkey: ${nixbuild_pubkey%%:*}"

  # Same idea as nixbuild-action (+ i686 for Android SDK ncurses-abi5-compat).
  # Keep jobs-per-connection modest on hosted agents (avoid SSH EOF storms).
  {
    echo 'nixbuild x86_64-linux - 32 1 big-parallel,benchmark,kvm,nixos-test,ca-derivations'
    echo 'nixbuild aarch64-linux - 32 1 big-parallel,benchmark,kvm,nixos-test,ca-derivations'
    echo 'nixbuild i686-linux - 32 1 big-parallel,benchmark,kvm,nixos-test,ca-derivations'
  } >"$builders_file"

  sudo mkdir -p /etc/nix
  sudo cp "$builders_file" /etc/nix/sleek-nixbuild-builders
  sudo chmod 644 /etc/nix/sleek-nixbuild-builders

  {
    echo 'experimental-features = nix-command flakes'
    echo 'accept-flake-config = true'
    echo 'builders-use-substitutes = true'
    echo 'require-sigs = true'
    echo 'max-jobs = 0'
    echo 'builders = @/etc/nix/sleek-nixbuild-builders'
    # Prefer Host alias (SSH config) so SetEnv/token applies; priority matches nixbuild-action.
    echo 'extra-substituters = ssh://nixbuild?priority=100'
    echo "extra-trusted-public-keys = $nixbuild_pubkey"
  } | sudo tee /etc/nix/sleek-nixbuild.conf >/dev/null
  sudo chmod 644 /etc/nix/sleek-nixbuild.conf

  if ! sudo grep -q 'sleek-nixbuild.conf' /etc/nix/nix.conf 2>/dev/null; then
    echo 'include /etc/nix/sleek-nixbuild.conf' | sudo tee -a /etc/nix/nix.conf >/dev/null
  fi
  if ! sudo grep -qF "$nixbuild_pubkey" /etc/nix/nix.conf 2>/dev/null; then
    echo "extra-trusted-public-keys = $nixbuild_pubkey" | sudo tee -a /etc/nix/nix.conf >/dev/null
  fi
  if ! sudo grep -q 'ssh://nixbuild' /etc/nix/nix.conf 2>/dev/null; then
    echo 'extra-substituters = ssh://nixbuild?priority=100' | sudo tee -a /etc/nix/nix.conf >/dev/null
  fi

  mkdir -p "$HOME/.config/nix"
  sudo cp /etc/nix/sleek-nixbuild.conf "$HOME/.config/nix/nix.conf"
  export NIX_CONFIG="include /etc/nix/sleek-nixbuild.conf"

  # Persist SSH opts for later remote-build in the same job (via env file).
  setup_env="$(mktemp)"
  umask 077
  {
    printf 'export NIX_SSHOPTS=%q\n' "$NIX_SSHOPTS"
    printf 'export NIX_CONFIG=%q\n' "$NIX_CONFIG"
    printf 'export PATH=%q\n' "$PATH"
  } >"$setup_env"
  chmod 600 "$setup_env"
  echo "NIXBUILD_SETUP_ENV=$setup_env"

  if [[ ! -S /nix/var/nix/daemon-socket/socket ]]; then
    echo "Starting nix-daemon with nixbuild trusted keys"
    sudo /nix/var/nix/profiles/default/bin/nix daemon >/tmp/nix-daemon.log 2>&1 &
    for _ in $(seq 1 30); do
      [[ -S /nix/var/nix/daemon-socket/socket ]] && break
      sleep 1
    done
    [[ -S /nix/var/nix/daemon-socket/socket ]] || {
      echo "nix-daemon socket missing after start; log:" >&2
      cat /tmp/nix-daemon.log >&2 || true
      exit 1
    }
  else
    echo "nix-daemon already running (socket present)"
  fi

  echo "--- :nix: show-config (key names / builders / substituters)"
  nix show-config 2>/dev/null | awk -F' = ' '/^trusted-public-keys =/{print; exit}' \
    | tr ' ' '\n' | awk -F: 'NF{print "trusted-key:", $1}' || true
  nix show-config 2>/dev/null | grep -E '^(substituters|extra-substituters|builders|builders-use-substitutes) =' || true
  echo "remote store: ssh-ng://nixbuild → HostName $NIXBUILD_SSH_HOST"
}

remote_build() {
  local attr="${1:?flake attr required, e.g. .#android}"
  local out_link="${2:-result}"
  shift 2 || true
  local -a artifacts=("$@")
  : "${NIX_SSHOPTS:?run setup first (NIX_SSHOPTS unset)}"

  # Recommended remote-store path (docs.nixbuild.net/remote-builds):
  #   nix build --builders '' --eval-store auto --store ssh-ng://eu.nixbuild.net ...
  # Use Host alias "nixbuild" so SSH SetEnv token applies.
  local store_uri="ssh-ng://nixbuild"
  echo "--- :nix: nix build $attr ($store_uri → $NIXBUILD_SSH_HOST)"
  local out_json
  out_json="$(mktemp)"
  nix build \
    --builders '' \
    --max-jobs 1 \
    --eval-store auto \
    --store "$store_uri" \
    "$attr" -L --print-build-logs --json | tee "$out_json"
  local out_path
  out_path="$(jq -r '.[0].outputs.out // empty' "$out_json")"
  [[ -n "$out_path" ]] || {
    echo "nix build produced no out path; json:" >&2
    cat "$out_json" >&2 || true
    exit 1
  }

  if [[ ${#artifacts[@]} -gt 0 ]]; then
    # Stream named files from the remote out path — avoids `nix copy` of the
    # full runtime closure (OOM on Buildkite LINUX_AMD64_2X4).
    mkdir -p "$out_link"
    local rel dest
    for rel in "${artifacts[@]}"; do
      dest="$out_link/$rel"
      mkdir -p "$(dirname "$dest")"
      echo "Streaming $out_path/$rel → $dest (nix store cat, no closure copy)"
      nix store cat --store "$store_uri" "$out_path/$rel" >"$dest"
      [[ -s "$dest" ]] || {
        echo "empty/missing artifact after store cat: $dest" >&2
        exit 1
      }
      ls -lh "$dest"
    done
    echo "out_link=$out_link (streamed from $out_path)"
    return 0
  fi

  echo "Copying $out_path from nixbuild → local store"
  nix copy --from "$store_uri" "$out_path"
  ln -sfn "$out_path" "$out_link"
  echo "out_link=$out_link -> $out_path"
}

cmd="${1:-}"
case "$cmd" in
  resolve-creds) resolve_creds ;;
  setup)
    setup_nixbuild
    ;;
  remote-build)
    shift
    remote_build "$@"
    ;;
  -h | --help | help)
    usage
    ;;
  "")
    usage
    exit 2
    ;;
  *)
    echo "ci-nixbuild: unknown command: $cmd" >&2
    usage >&2
    exit 2
    ;;
esac
