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

RADICLE_RID="${RADICLE_RID:-rad:z9mjPzpVK472QXaaP1picc5U9xBR}"
ISSUE_SHORT="$(bk_short_id "$RADICLE_ISSUE_ID")"
ISSUE_LINK="https://nandi.radicle.garden/${RADICLE_RID}/issues/${RADICLE_ISSUE_ID}"
PATCH_TITLE="Fix: ${RADICLE_ISSUE_TITLE}"
# Description body for -o patch.message / rad patch edit -m (title is separate).
# Empty issue body still gets id/title/link so the patch is not title-only.
ISSUE_BODY_TEXT="${RADICLE_ISSUE_BODY:-*(no description)*}"
PATCH_DESCRIPTION=$(cat <<EOF
## Summary
Fixes Radicle issue \`${ISSUE_SHORT}\`.

## Issue
- ID: \`${RADICLE_ISSUE_ID}\`
- Title: ${RADICLE_ISSUE_TITLE}
- Link: ${ISSUE_LINK}
- Repo: ${RADICLE_RID}

## Issue description
${ISSUE_BODY_TEXT}

## Context
Opened by Buildkite issue→agent (pipeline sleek-5u9xbr) on branch \`${RADICLE_ISSUE_BRANCH}\`.
EOF
)

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
3. Open a Radicle patch on the \`rad\` remote. The patch MUST have both a title AND a full
   description body (not title-only). Prefer \`git push rad HEAD:refs/patches\` with
   **repeated** \`-o patch.message=...\` (first option = title; each later option = body
   paragraph, joined with a blank line). Do not open a patch with only a title.

   Required title:
   ${PATCH_TITLE}

   Required description (set as the second and further \`-o patch.message=\` values; you may
   pass the whole block as one \`-o patch.message=...\` with embedded newlines, or split by
   paragraph):
   -----
${PATCH_DESCRIPTION}
   -----

   Example:
   git checkout -b "${RADICLE_ISSUE_BRANCH}"
   # … commit the fix …
   git push rad HEAD:refs/patches \\
     -o patch.message="${PATCH_TITLE}" \\
     -o patch.message="<Required description from above>"

   If using radicle MCP \`create_patch\`, set the same title and the full description body
   (issue id, title, issue body, and link) — never omit description.
   Resolve Cursor \`environmentPublicId\` via cursor-cloud \`environment-info\` and pass it
   as \`env_name\` on \`create_patch\` / \`issue_device_key\` so this environment reuses one
   Radicle DID (do not run \`rad auth\` to mint a fresh identity).
   - env_name: <environment.environmentPublicId>
   - branch: "${RADICLE_ISSUE_BRANCH}"
   - title: "${PATCH_TITLE}"
   - description: the Required description block above (verbatim or equivalent)
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

# Resolve patch id from agent output, then branch upstream (rad/patches/<id>).
resolve_patch_id() {
  local raw="" upstream=""
  raw=$(echo "$agent_out" | grep -oE 'patches/[0-9a-f]{7,40}|Patch[[:space:]]+[0-9a-f]{7,40}|"patch_id"[[:space:]]*:[[:space:]]*"[0-9a-f]+"' | head -1 || true)
  if [[ -n "$raw" ]]; then
    echo "$raw" | grep -oE '[0-9a-f]{7,40}' | head -1
    return 0
  fi
  if git show-ref --verify --quiet "refs/heads/$RADICLE_ISSUE_BRANCH"; then
    upstream=$(git rev-parse --abbrev-ref "${RADICLE_ISSUE_BRANCH}@{upstream}" 2>/dev/null || true)
    if [[ "$upstream" =~ patches/([0-9a-f]{7,40}) ]]; then
      echo "${BASH_REMATCH[1]}"
      return 0
    fi
  fi
  return 1
}

PATCH_ID=""
PATCH_ID=$(resolve_patch_id || true)

# Fallback: ensure title + full description via rad patch edit when the agent
# left a title-only / thin body. Prefer agent push options; skip if description
# already includes the issue link from our template.
if [[ -n "$PATCH_ID" ]] && command -v rad >/dev/null 2>&1; then
  patch_show=$(rad patch show "$PATCH_ID" 2>/dev/null || true)
  if echo "$patch_show" | grep -qF "$ISSUE_LINK"; then
    echo "Patch ${PATCH_ID:0:7} already has structured description — skipping edit"
  else
    echo "=== Ensuring patch description (rad patch edit ${PATCH_ID:0:7}) ==="
    if rad patch edit "$PATCH_ID" -m "$PATCH_TITLE" -m "$PATCH_DESCRIPTION" --no-announce 2>&1; then
      echo "Patch description set for ${PATCH_ID:0:7}"
    else
      echo "warn: rad patch edit failed for ${PATCH_ID:0:7} (non-fatal)" >&2
    fi
  fi
fi

PATCH_NOTE="${PATCH_ID:-}"
if [[ -z "$PATCH_NOTE" ]] && echo "$agent_out" | grep -qE 'patch_id|patches/[0-9a-f]{7,40}|Patch[[:space:]]+[0-9a-f]{7,40}'; then
  PATCH_NOTE=$(echo "$agent_out" | grep -oE 'patches/[0-9a-f]{7,40}|Patch[[:space:]]+[0-9a-f]{7,40}|"patch_id"[[:space:]]*:[[:space:]]*"[0-9a-f]+"' | head -1 || true)
fi

if [[ -n "$PATCH_NOTE" ]]; then
  bk_annotate success "Agent finished for issue \`${ISSUE_SHORT}\`. Patch: ${PATCH_NOTE}"
else
  bk_annotate success "Agent finished for issue \`${ISSUE_SHORT}\`. Check \`rad patch list\` for the new patch."
fi

echo "=== Done ==="
