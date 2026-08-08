#!/usr/bin/env bash
# delegate.sh — run a task via the cursor-agent CLI as a subagent.
#   Usage: delegate.sh "task" [--workspace DIR] [--model ID] [--mode plan|ask]
#                      [--output-format json|text] [--timeout SECONDS] [--force]
set -euo pipefail

task=""
workspace=""
model=""
mode=""
output_format="json"
timeout=1800
force=0
trust=1

while [[ $# -gt 0 ]]; do
  case "$1" in
    --workspace)  workspace="$2"; shift 2 ;;
    --model)      model="$2";    shift 2 ;;
    --mode)       mode="$2";     shift 2 ;;
    --output-format) output_format="$2"; shift 2 ;;
    --timeout)    timeout="$2";  shift 2 ;;
    --force)      force=1;       shift ;;
    --notrust)    trust=0;       shift ;;
    -h|--help)    echo "$usage_help"; exit 0 ;;
    -*)           echo "unknown flag: $1" >&2; exit 2 ;;
    *)            if [[ -z "$task" ]]; then task="$1"; else task="$task $1"; fi; shift ;;
  esac
done

if [[ -z "${task:-}" ]]; then
  echo "error: provide a task/instruction to delegate" >&2
  exit 2
fi

AGENT_BIN=""
if command -v cursor-agent >/dev/null 2>&1; then
  AGENT_BIN=cursor-agent
elif command -v agent >/dev/null 2>&1; then
  AGENT_BIN=agent
fi
[[ -n "$AGENT_BIN" ]] || { echo "error: cursor-agent / agent not found on PATH" >&2; exit 127; }
"$AGENT_BIN" status >/dev/null 2>&1 || { echo "error: Cursor CLI not authenticated (set CURSOR_API_KEY or run: $AGENT_BIN login)" >&2; exit 1; }

args=(--print --output-format "$output_format")
[[ -n "$model" ]] && args+=(--model "$model")
[[ -n "$mode"  ]] && args+=(--mode "$mode")
[[ "$force" == 1  ]] && args+=(--force)
[[ "$trust" == 1  ]] && args+=(--trust)
[[ -n "$workspace" ]] && args+=(--workspace "$workspace")
args+=( "$task" )

out="$(mktemp)"; err="$(mktemp)"; rc=0
timeout "$timeout" "$AGENT_BIN" "${args[@]}" >"$out" 2>"$err" || rc="$?"
if [[ "$rc" != 0 ]]; then
  echo "error: $AGENT_BIN failed (exit $rc):" >&2
  sed -n '1,20p' "$err" >&2
  rm -f "$out" "$err"
  exit "$rc"
fi
rm -f "$err"
cat "$out"
rm -f "$out"
