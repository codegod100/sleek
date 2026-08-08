#!/usr/bin/env bash
# Install cursor-agent (if missing) and build Radicle MCP for CI runs.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

REPO_ROOT="$(bk_repo_root)"
export PATH="${HOME}/.cursor/bin:${HOME}/.local/bin:/usr/local/bin:${PATH:-}"

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
  if [[ -z "${CURSOR_API_KEY:-}" ]] && command -v buildkite-agent >/dev/null 2>&1; then
    if key="$(buildkite-agent secret get CURSOR_API_KEY 2>/dev/null || true)"; then
      [[ -n "$key" ]] && export CURSOR_API_KEY="$key"
    fi
  fi
  if [[ -z "${CURSOR_API_KEY:-}" ]]; then
    bk_die "CURSOR_API_KEY is not set — add it as a Buildkite cluster secret"
  fi
  if ! "$cmd" status >/dev/null 2>&1; then
    bk_die "Cursor CLI auth failed — check CURSOR_API_KEY"
  fi
}

install_cursor_agent
build_radicle_mcp
verify_auth

echo "[bootstrap] ready (repo=$REPO_ROOT)"
