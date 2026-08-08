#!/usr/bin/env bash
# Shared helpers for Radicle issue → cursor-agent → patch Buildkite steps (sleek).
set -euo pipefail

bk_die() { echo "radicle-issue-agent: $*" >&2; exit 1; }

bk_require_cmd() {
  command -v "$1" >/dev/null 2>&1 || bk_die "$1 not found on PATH"
}

# Cursor installer puts `agent` / `cursor-agent` under ~/.local/bin (and sometimes
# ~/.cursor/bin). Bootstrap runs as a subprocess, so every issue-agent entrypoint
# must re-export these dirs — otherwise delegate sees exit 127 after a successful
# install.
bk_export_cursor_path() {
  export PATH="${HOME}/.cursor/bin:${HOME}/.local/bin:/usr/local/bin:${PATH:-}"
}

# Resolve cursor-agent CLI name after PATH is set. Prints binary name or returns 1.
bk_cursor_agent_cmd() {
  if command -v cursor-agent >/dev/null 2>&1; then
    echo "cursor-agent"
    return 0
  fi
  if command -v agent >/dev/null 2>&1; then
    echo "agent"
    return 0
  fi
  return 1
}

bk_repo_root() {
  git rev-parse --show-toplevel 2>/dev/null || bk_die "not inside a git repository"
}

bk_require_rad_repo() {
  local root
  root="$(bk_repo_root)"
  git -C "$root" remote get-url rad >/dev/null 2>&1 \
    || bk_die "remote 'rad' missing — clone via rad clone or rad remote add rad <rid>"
}

bk_rad_rid() {
  git remote get-url rad
}

# New issue COBs use the creation commit as the issue id (40-char hex).
bk_commit_is_new_issue() {
  local commit=$1 rid
  rid="$(bk_rad_rid)"
  rad cob list --repo "$rid" --type xyz.radicle.issue 2>/dev/null \
    | grep -qxF "$commit"
}

# Parse rad issue show --header output into title + description.
bk_issue_details() {
  local issue_id=$1
  python3 - "$issue_id" <<'PY'
import re
import subprocess
import sys

issue_id = sys.argv[1]
proc = subprocess.run(
    ["rad", "issue", "show", issue_id, "--header"],
    capture_output=True,
    text=True,
)
if proc.returncode != 0:
    sys.stderr.write(proc.stderr or proc.stdout)
    sys.exit(proc.returncode)

text = proc.stdout
title = ""
body = ""

title_match = re.search(r"Title\s+(.+?)\s+Issue", text, re.DOTALL)
if title_match:
    title = " ".join(title_match.group(1).split())

lines = [ln.strip() for ln in text.splitlines()]
for ln in lines:
    if not ln or ln.startswith("╭") or ln.startswith("╰") or ln.startswith("│"):
        continue
    if re.match(r"^(Title|Issue|Author|Status)\b", ln):
        continue
    if re.match(r"^[●○]", ln):
        continue
    body = ln
    break

if not body:
    for ln in reversed(lines):
        cleaned = ln.strip("│ ").strip()
        if cleaned and not re.match(r"^(Title|Issue|Author|Status)\b", cleaned):
            body = cleaned
            break

print(title)
print(body)
PY
}

bk_short_id() {
  local id=$1
  echo "${id:0:7}"
}

bk_annotate() {
  local style=$1
  local message=$2
  if command -v buildkite-agent >/dev/null 2>&1; then
    buildkite-agent annotate "$message" --style "$style" --context "radicle-issue-agent" || true
  fi
}
