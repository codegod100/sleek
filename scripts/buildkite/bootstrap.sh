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
  local name=$1 value="" err="" errf rc=0
  if [[ -n "${!name:-}" ]]; then
    return 0
  fi
  if command -v buildkite-agent >/dev/null 2>&1; then
    errf="$(mktemp)"
    set +e
    # Trailing newline is stripped by $() — restore one for PEM materialization.
    value="$(buildkite-agent secret get "$name" 2>"$errf")"
    rc=$?
    set -e
    err="$(tr -d '\r' <"$errf" | tr '\n' ' ' | sed 's/[[:space:]]*$//')"
    rm -f "$errf"
    if [[ $rc -eq 0 && -n "$value" ]]; then
      # printf -v keeps multiline PEMs intact (unlike export name=$value).
      printf -v "$name" '%s\n' "$value"
      export "$name"
      return 0
    fi
    # Undeclared optional secrets always 404 under cluster secrets — stay quiet.
    # Required secrets (listed in step secrets:) must warn so policy misses show up.
    if [[ "${2:-}" == "--required" ]]; then
      if [[ -n "$err" ]]; then
        echo "[bootstrap] warn: soft-load $name failed: $err" >&2
        echo "[bootstrap] hint: Default cluster → Secrets → $name → allow pipeline ${BUILDKITE_PIPELINE_SLUG:-sleek-5u9xbr} (or all pipelines)" >&2
      else
        echo "[bootstrap] warn: soft-load $name returned empty (secret missing or policy excludes this pipeline)" >&2
      fi
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
  bk_soft_secret CURSOR_API_KEY --required
  if [[ -z "${CURSOR_API_KEY:-}" ]]; then
    bk_die "CURSOR_API_KEY is not set — add it as a Buildkite cluster secret (policy must allow sleek-5u9xbr)"
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
  # Only RADICLE_SECRET_KEY is listed under step secrets: (required). Optional
  # OPENBAO_TOKEN / RADICLE_PUBLIC_KEY / RAD_PASSPHRASE soft-get quietly (404 if
  # undeclared) and may still come from OpenBao when OPENBAO_TOKEN is present.
  bk_soft_secret OPENBAO_TOKEN
  bk_soft_secret RADICLE_SECRET_KEY --required
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

# RADICLE_SEED is nid@host:port; rad seed --from / rad clone --seed want NID only.
radicle_seed_nid() {
  local seed="${1:-$RADICLE_SEED}"
  echo "${seed%%@*}"
}

start_rad_node() {
  # IMPORTANT: `rad node status` exits 0 even when the node is stopped, so do
  # not gate on its exit code. `rad node start` is idempotent when already up.
  echo "[bootstrap] ensuring rad node is running..."
  rad node start >/dev/null
  # Connect to Garden so seed/fetch and patch announce can reach the seed.
  local seed_nid
  seed_nid="$(radicle_seed_nid)"
  if rad node connect "$RADICLE_SEED" >/dev/null 2>&1; then
    echo "[bootstrap] connected to Garden seed ${seed_nid}"
  else
    echo "[bootstrap] warn: rad node connect ${RADICLE_SEED} failed (will retry on seed)" >&2
  fi
}

# `rad init --existing` only links a working tree; it does not fetch the RID.
# Garden HTTPS clones have no $RAD_HOME/storage/<rid>, so seed/fetch first.
ensure_rid_in_storage() {
  local rid_naked seed_nid timeout
  rid_naked="${RADICLE_RID#rad:}"
  rid_naked="${rid_naked#rad://}"
  seed_nid="$(radicle_seed_nid)"
  timeout="${RADICLE_SEED_TIMEOUT:-120s}"

  if [[ -d "$RAD_HOME/storage/$rid_naked" ]]; then
    echo "[bootstrap] Radicle storage already has ${rid_naked}"
    return 0
  fi

  echo "[bootstrap] fetching ${RADICLE_RID} into \$RAD_HOME/storage (required before rad init --existing)"
  # Prefer Garden seed NID. Note: `rad seed` can exit 0 after only updating the
  # local seeding policy when no peers are reachable — always verify storage.
  rad seed "$RADICLE_RID" --scope followed --from "$seed_nid" --timeout "$timeout" || true
  if [[ ! -d "$RAD_HOME/storage/$rid_naked" ]]; then
    echo "[bootstrap] warn: storage empty after seed --from; reconnecting and retrying via routing table" >&2
    rad node connect "$RADICLE_SEED" >/dev/null 2>&1 || true
    rad seed "$RADICLE_RID" --scope followed --timeout "$timeout" \
      || bk_die "failed to fetch ${RADICLE_RID} into local storage — is the node connected to Garden (${RADICLE_SEED})?"
  fi
  [[ -d "$RAD_HOME/storage/$rid_naked" ]] \
    || bk_die "storage path missing after rad seed: $RAD_HOME/storage/$rid_naked"
}

# Garden HTTPS checkouts only have `origin` → https://…garden….git.
# Seed RID into local storage, then link the working tree + add `rad` remote
# so `git push rad` works.
ensure_rad_remote() {
  local root rid_naked
  root="$(bk_repo_root)"
  if git -C "$root" remote get-url rad >/dev/null 2>&1; then
    echo "[bootstrap] git remote 'rad' already configured"
    # Still ensure storage exists (needed for git-remote-rad push/fetch).
    ensure_rid_in_storage
    return 0
  fi

  ensure_rid_in_storage

  echo "[bootstrap] linking Garden checkout via rad init --existing ${RADICLE_RID}"
  (
    cd "$root"
    rad init --existing "$RADICLE_RID" \
      --name sleek \
      --default-branch main \
      --no-confirm \
      --public
  )
  if ! git -C "$root" remote get-url rad >/dev/null 2>&1; then
    rid_naked="${RADICLE_RID#rad:}"
    rid_naked="${rid_naked#rad://}"
    echo "[bootstrap] warn: rad init --existing did not add remote; adding rad:// manually" >&2
    git -C "$root" remote add rad "rad://${rid_naked}"
  fi
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
