#!/usr/bin/env bash
# Install cursor-agent (if missing), materialize Radicle CI identity, and
# (optionally) build Radicle MCP for issue→agent runs.
#
# Cluster secrets (Buildkite → Default cluster → Secrets; soft-loaded):
#   CURSOR_API_KEY          — required for agent
#   RADICLE_SECRET_KEY      — OpenSSH private key PEM for a *dedicated* CI identity
#   RADICLE_PUBLIC_KEY      — optional; derived via ssh-keygen -y if missing
#   RAD_PASSPHRASE          — optional; empty OK for empty-passphrase CI keys
#   OPENBAO_TOKEN           — optional; can fetch the above from secret/data/radicle
#
# OpenBao field names under secret/data/radicle match the env names above.
# Never commit key material. Prefer a dedicated CI DID (not a personal identity).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

REPO_ROOT="$(bk_repo_root)"
export PATH="${HOME}/.local/bin:/usr/local/bin:${PATH:-}"

# Sleek RID (Garden HTTPS checkouts lack a rad:// remote until we link them).
RADICLE_RID="${RADICLE_RID:-rad:z9mjPzpVK472QXaaP1picc5U9xBR}"
RAD_HOME="${RAD_HOME:-${HOME}/.radicle}"
export RAD_HOME
export PATH="${RAD_HOME}/bin:${PATH}"

# Garden seed (from local preferredSeeds / AGENTS Garden host).
RADICLE_SEED="${RADICLE_SEED:-z6MknYm3iSpuY5hLCH93K5Ls5KG7cBK4fQwybqcHzxDsT2jU@nandi.radicle.garden:58019}"

bk_soft_secret() {
  local name=$1 value=""
  if [[ -n "${!name:-}" ]]; then
    return 0
  fi
  if command -v buildkite-agent >/dev/null 2>&1; then
    value="$(buildkite-agent secret get "$name" 2>/dev/null || true)"
    if [[ -n "$value" ]]; then
      # printf -v keeps multiline PEMs intact (unlike export name=$value).
      printf -v "$name" '%s' "$value"
      export "$name"
    fi
  fi
}

install_cursor_agent() {
  if command -v cursor-agent >/dev/null 2>&1; then
    return 0
  fi
  if command -v agent >/dev/null 2>&1; then
    return 0
  fi

  echo "[bootstrap] installing Cursor CLI..."
  curl -fsSL https://cursor.com/install | bash
  export PATH="${HOME}/.cursor/bin:${PATH:-}"
}

cursor_agent_cmd() {
  if command -v cursor-agent >/dev/null 2>&1; then
    echo "cursor-agent"
    return 0
  fi
  if command -v agent >/dev/null 2>&1; then
    echo "agent"
    return 0
  fi
  bk_die "cursor-agent / agent not found after install"
}

build_radicle_mcp() {
  local mcp_dir="$REPO_ROOT/mcp/radicle"
  if [[ ! -f "$mcp_dir/package.json" ]]; then
    echo "[bootstrap] no mcp/radicle in repo — skipping MCP build"
    return 0
  fi

  if [[ -f "$mcp_dir/dist/index.js" ]]; then
    echo "[bootstrap] radicle MCP already built"
    return 0
  fi

  bk_require_cmd node
  bk_require_cmd npm
  echo "[bootstrap] building radicle MCP..."
  npm ci --prefix "$mcp_dir"
  npm run build --prefix "$mcp_dir"
}

verify_auth() {
  local cmd
  cmd="$(cursor_agent_cmd)"
  bk_soft_secret CURSOR_API_KEY
  if [[ -z "${CURSOR_API_KEY:-}" ]]; then
    bk_die "CURSOR_API_KEY is not set — add it as a Buildkite cluster secret"
  fi
  if ! "$cmd" status >/dev/null 2>&1; then
    bk_die "Cursor CLI auth failed — check CURSOR_API_KEY"
  fi
}

install_radicle_cli() {
  if command -v rad >/dev/null 2>&1 && command -v git-remote-rad >/dev/null 2>&1; then
    echo "[bootstrap] rad + git-remote-rad already on PATH"
    return 0
  fi
  echo "[bootstrap] installing Radicle CLI into ${RAD_HOME}..."
  curl -fsSL https://radicle.xyz/install | sh -s -- --no-modify-path --prefix="$RAD_HOME"
  export PATH="${RAD_HOME}/bin:${PATH}"
  command -v rad >/dev/null 2>&1 || bk_die "rad missing after install"
  command -v git-remote-rad >/dev/null 2>&1 || bk_die "git-remote-rad missing after install"
}

load_radicle_secrets() {
  bk_soft_secret OPENBAO_TOKEN
  bk_soft_secret RADICLE_SECRET_KEY
  bk_soft_secret RADICLE_PUBLIC_KEY
  bk_soft_secret RAD_PASSPHRASE

  # Optional OpenBao fallback (field names = env names under secret/data/radicle).
  # Only require RADICLE_SECRET_KEY; public key / passphrase are optional.
  if [[ -z "${RADICLE_SECRET_KEY:-}" && -n "${OPENBAO_TOKEN:-}" ]]; then
    local root fetch exports
    root="$(cd "$SCRIPT_DIR/../.." && pwd)"
    fetch="$root/scripts/fetch-openbao-env.sh"
    if [[ -x "$fetch" ]]; then
      echo "[bootstrap] loading RADICLE_SECRET_KEY from OpenBao secret/data/radicle"
      if exports="$(
        OPENBAO_SECRET_PATH=secret/data/radicle \
        OPENBAO_EXTRA_PATHS= \
        "$fetch" --export --keys RADICLE_SECRET_KEY 2>/dev/null
      )"; then
        eval "$exports"
      else
        echo "[bootstrap] warn: OpenBao secret/data/radicle missing RADICLE_SECRET_KEY" >&2
      fi
      # Best-effort optional fields (ignore failure if absent).
      if exports="$(
        OPENBAO_SECRET_PATH=secret/data/radicle \
        OPENBAO_EXTRA_PATHS= \
        "$fetch" --export --keys RADICLE_PUBLIC_KEY 2>/dev/null
      )"; then
        eval "$exports"
      fi
      if exports="$(
        OPENBAO_SECRET_PATH=secret/data/radicle \
        OPENBAO_EXTRA_PATHS= \
        "$fetch" --export --keys RAD_PASSPHRASE 2>/dev/null
      )"; then
        eval "$exports"
      fi
    fi
  fi
}

write_radicle_config() {
  mkdir -p "$RAD_HOME"
  if [[ -f "$RAD_HOME/config.json" ]]; then
    return 0
  fi
  cat >"$RAD_HOME/config.json" <<EOF
{
  "publicExplorer": "https://nandi.radicle.garden/nodes/\$host/\$rid\$path",
  "preferredSeeds": ["${RADICLE_SEED}"],
  "node": {
    "alias": "sleek-ci",
    "listen": [],
    "peers": { "type": "dynamic" },
    "connect": ["${RADICLE_SEED}"],
    "externalAddresses": [],
    "network": "main",
    "log": "INFO",
    "relay": "auto",
    "seedingPolicy": { "default": "block" }
  }
}
EOF
}

materialize_radicle_keys() {
  local key_path pub_path
  key_path="$RAD_HOME/keys/radicle"
  pub_path="$RAD_HOME/keys/radicle.pub"
  mkdir -p "$RAD_HOME/keys"

  if [[ -f "$key_path" && -f "$pub_path" ]]; then
    echo "[bootstrap] Radicle keys already present under $RAD_HOME/keys"
    return 0
  fi

  if [[ -z "${RADICLE_SECRET_KEY:-}" ]]; then
    bk_die "RADICLE_SECRET_KEY unset — add Buildkite secret (or OpenBao secret/data/radicle) for a dedicated CI identity"
  fi

  # Accept raw PEM or base64-encoded PEM.
  if [[ "$RADICLE_SECRET_KEY" == -----BEGIN* ]]; then
    printf '%s\n' "$RADICLE_SECRET_KEY" >"$key_path"
  else
    if ! printf '%s' "$RADICLE_SECRET_KEY" | base64 -d >"$key_path" 2>/dev/null; then
      bk_die "RADICLE_SECRET_KEY is neither OpenSSH PEM nor base64"
    fi
    if ! head -1 "$key_path" | grep -q 'BEGIN.*PRIVATE KEY'; then
      bk_die "RADICLE_SECRET_KEY base64 did not decode to an OpenSSH private key"
    fi
  fi
  chmod 600 "$key_path"

  if [[ -n "${RADICLE_PUBLIC_KEY:-}" ]]; then
    printf '%s\n' "$RADICLE_PUBLIC_KEY" >"$pub_path"
  else
    # Empty passphrase is fine for dedicated CI keys (seeder-style).
    if ! ssh-keygen -y -P "${RAD_PASSPHRASE:-}" -f "$key_path" >"$pub_path" 2>/dev/null; then
      bk_die "failed to derive public key — set RADICLE_PUBLIC_KEY or fix RAD_PASSPHRASE"
    fi
  fi
  chmod 644 "$pub_path"
  echo "[bootstrap] materialized Radicle identity keys (alias config: sleek-ci)"
}

# Export empty RAD_PASSPHRASE when unset so git-remote-rad uses MemorySigner
# instead of requiring ssh-agent (ephemeral hosted agents rarely have one ready).
ensure_rad_passphrase_env() {
  if [[ -z "${RAD_PASSPHRASE+x}" ]]; then
    export RAD_PASSPHRASE=""
  else
    export RAD_PASSPHRASE
  fi
}

start_rad_node() {
  if rad node status >/dev/null 2>&1; then
    echo "[bootstrap] rad node already running"
  else
    echo "[bootstrap] starting rad node..."
    rad node start >/dev/null
  fi
  # Best-effort connect to Garden so patch announce can reach the seed.
  rad node connect "$RADICLE_SEED" >/dev/null 2>&1 || true
}

# Garden HTTPS checkouts only have `origin` → https://…garden….git.
# Link them into local storage + add `rad` remote so `git push rad` works.
ensure_rad_remote() {
  local root
  root="$(bk_repo_root)"
  if git -C "$root" remote get-url rad >/dev/null 2>&1; then
    echo "[bootstrap] git remote 'rad' already configured"
    return 0
  fi
  echo "[bootstrap] linking Garden checkout via rad init --existing ${RADICLE_RID}"
  (
    cd "$root"
    rad init --existing "$RADICLE_RID" \
      --name sleek \
      --default-branch main \
      --no-confirm \
      --public
  )
  git -C "$root" remote get-url rad >/dev/null 2>&1 \
    || bk_die "rad remote still missing after rad init --existing"
}

setup_radicle_identity() {
  # Soft-skip when this bootstrap is reused outside the issue-agent path and
  # no identity secrets are configured yet.
  load_radicle_secrets
  if [[ -z "${RADICLE_SECRET_KEY:-}" && ! -f "$RAD_HOME/keys/radicle" ]]; then
    if [[ "${RADICLE_REQUIRE_IDENTITY:-1}" == "1" ]]; then
      bk_die "no Radicle CI identity — set RADICLE_SECRET_KEY (Buildkite) or keys under \$RAD_HOME"
    fi
    echo "[bootstrap] warn: skipping Radicle identity (RADICLE_REQUIRE_IDENTITY=0)"
    return 0
  fi

  install_radicle_cli
  write_radicle_config
  materialize_radicle_keys
  ensure_rad_passphrase_env
  start_rad_node
  ensure_rad_remote

  echo "[bootstrap] Radicle identity ready: $(rad self --alias 2>/dev/null || echo '?') $(rad self --did 2>/dev/null || true)"
}

install_cursor_agent
build_radicle_mcp
verify_auth
setup_radicle_identity

echo "[bootstrap] ready (repo=$REPO_ROOT rad_home=$RAD_HOME)"
