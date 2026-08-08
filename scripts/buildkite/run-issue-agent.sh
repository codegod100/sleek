#!/usr/bin/env bash
# Buildkite entrypoint: Garden issue event → cursor-agent → Radicle patch.
#
# Required env (Buildkite cluster secrets / OpenBao secret/data/radicle):
#   CURSOR_API_KEY       — Cursor CLI / service account key
#   RADICLE_SECRET_KEY   — dedicated CI identity OpenSSH private key (PEM)
#
# Optional:
#   RADICLE_PUBLIC_KEY / RAD_PASSPHRASE — derived / empty passphrase OK
#   RADICLE_AGENT_MODEL   — passed to cursor-agent --model
#   RADICLE_AGENT_TIMEOUT — seconds (default 3600)
#   RADICLE_AGENT_DRY_RUN — set to 1 to print prompt without running agent
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"
bk_export_cursor_path

TIMEOUT="${RADICLE_AGENT_TIMEOUT:-3600}"
MODEL="${RADICLE_AGENT_MODEL:-}"
DRY_RUN="${RADICLE_AGENT_DRY_RUN:-0}"

echo "=== Radicle issue agent ==="
echo "commit: ${BUILDKITE_COMMIT:-unknown}"
echo "branch: ${BUILDKITE_BRANCH:-unknown}"
echo "repo:   $REPO_ROOT"

# Detect before bootstrap so normal CI (no CURSOR_API_KEY) can soft-skip.
# Keep stderr (warnings) out of eval — only assignment lines are sourced.
detect_err="$(mktemp)"
set +e
detect_out="$(bash "$SCRIPT_DIR/detect-issue.sh" 2>"$detect_err")"
detect_rc=$?
set -e
if [[ -s "$detect_err" ]]; then
  cat "$detect_err" >&2
fi
rm -f "$detect_err"

if [[ "$detect_rc" -eq 2 ]]; then
  echo "$detect_out"
  bk_annotate info "Skipped: not a new Radicle issue event."
  exit 0
fi
if [[ "$detect_rc" -ne 0 ]]; then
  echo "$detect_out" >&2
  bk_die "issue detection failed (exit $detect_rc)"
fi

eval "$(printf '%s\n' "$detect_out" | grep -E '^RADICLE_ISSUE_')"

echo "issue:  $RADICLE_ISSUE_ID"
echo "title:  $RADICLE_ISSUE_TITLE"
echo "branch: $RADICLE_ISSUE_BRANCH"

# Idempotency: skip if we already started work on this issue.
if git show-ref --verify --quiet "refs/heads/$RADICLE_ISSUE_BRANCH"; then
  msg="Branch \`$RADICLE_ISSUE_BRANCH\` already exists — skipping duplicate agent run."
  echo "$msg"
  bk_annotate warning "$msg"
  exit 0
fi

bash "$SCRIPT_DIR/bootstrap.sh"
# Bootstrap is a subprocess — re-apply Cursor + local bin PATH for delegate.
bk_export_cursor_path
if ! bk_cursor_agent_cmd >/dev/null; then
  bk_die "cursor-agent / agent not found on PATH after bootstrap (PATH=$PATH)"
fi
echo "cursor-agent: $(command -v "$(bk_cursor_agent_cmd)")"

if command -v buildkite-agent >/dev/null 2>&1; then
  buildkite-agent meta-data set "radicle_issue_id" "$RADICLE_ISSUE_ID" || true
  buildkite-agent meta-data set "radicle_issue_branch" "$RADICLE_ISSUE_BRANCH" || true
fi

PROMPT=$(cat <<EOF
A new Radicle issue was opened in this repository. Implement a fix and open a Radicle patch.

Issue ID: ${RADICLE_ISSUE_ID}
Title: ${RADICLE_ISSUE_TITLE}
Description:
${RADICLE_ISSUE_BODY}

Requirements:
1. Read the codebase and implement a fix for this issue.
2. Run relevant verification from AGENTS.md when practical:
   - \`nix develop . --command cargo clippy --manifest-path host/Cargo.toml -- -D warnings\`
   - \`nix develop . --command cargo test --manifest-path android/Cargo.toml --lib\`
3. Open a Radicle patch on the \`rad\` remote (prefer \`git push rad HEAD:refs/patches\` with
   \`-o patch.message=...\`, or the radicle MCP \`create_patch\` if available):
   - branch: "${RADICLE_ISSUE_BRANCH}"
   - title: "Fix: ${RADICLE_ISSUE_TITLE}"
   - body: "Addresses Radicle issue ${RADICLE_ISSUE_ID}\\n\\n${RADICLE_ISSUE_BODY}"
   - commit: a clear commit message describing the fix
4. Do not close the issue. Only open the patch.

If the issue is not actionable (needs clarification, is a duplicate, etc.), explain why in your response and do not open a patch.
EOF
)

if [[ "$DRY_RUN" == "1" ]]; then
  echo "--- DRY RUN: agent prompt ---"
  echo "$PROMPT"
  exit 0
fi

DELEGATE="$SCRIPT_DIR/delegate.sh"
if [[ ! -x "$DELEGATE" ]]; then
  bk_die "delegate.sh not found at $DELEGATE"
fi

echo "=== Running cursor-agent (timeout=${TIMEOUT}s) ==="
set +e
agent_args=(--workspace "$REPO_ROOT" --timeout "$TIMEOUT" --force)
if [[ -n "$MODEL" ]]; then
  agent_args+=(--model "$MODEL")
fi
agent_out="$("$DELEGATE" "$PROMPT" "${agent_args[@]}" 2>&1)"
agent_rc=$?
set -e

echo "$agent_out"

if [[ "$agent_rc" -ne 0 ]]; then
  bk_annotate error "cursor-agent failed (exit $agent_rc). See build logs."
  exit "$agent_rc"
fi

# Best-effort: surface patch id from agent output or recent patches.
PATCH_NOTE=""
if echo "$agent_out" | grep -qE 'patch_id|patches/[0-9a-f]{7,40}|Patch[[:space:]]+[0-9a-f]{7,40}'; then
  PATCH_NOTE=$(echo "$agent_out" | grep -oE 'patches/[0-9a-f]{7,40}|Patch[[:space:]]+[0-9a-f]{7,40}|"patch_id"[[:space:]]*:[[:space:]]*"[0-9a-f]+"' | head -1 || true)
fi

if [[ -n "$PATCH_NOTE" ]]; then
  bk_annotate success "Agent finished for issue \`${RADICLE_ISSUE_ID:0:7}\`. Patch: ${PATCH_NOTE}"
else
  bk_annotate success "Agent finished for issue \`${RADICLE_ISSUE_ID:0:7}\`. Check \`rad patch list\` for the new patch."
fi

echo "=== Done ==="
