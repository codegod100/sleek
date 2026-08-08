#!/usr/bin/env bash
# Translate a Radicle CI broker push/patch webhook payload into a Buildkite build
# when the event commit is a new issue COB root.
#
# Intended for the Garden webhooks adapter: pipe broker/webhook JSON on stdin.
# Requires BUILDKITE_ORG, BUILDKITE_PIPELINE, BUILDKITE_API_TOKEN.
#
# Example (on Garden after an issue open):
#   echo "$payload" | BUILDKITE_ORG=nandi BUILDKITE_PIPELINE=sleek \
#     BUILDKITE_API_TOKEN=... ./scripts/buildkite/webhook-to-buildkite.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

bk_require_cmd jq

payload="$(cat)"
commit="$(echo "$payload" | jq -r '.commit // .Commit // .head // empty')"
repo="$(echo "$payload" | jq -r '.repo // .repository // .repo_id // empty')"

if [[ -z "$commit" ]]; then
  echo "webhook: no commit in payload — ignoring" >&2
  exit 0
fi

echo "webhook: repo=${repo:-unknown} commit=${commit:0:7}"

export RAD_HOME="${RAD_HOME:-$HOME/.radicle}"
bk_require_cmd rad

# Resolve repo checkout if RAD_REPO_PATH is set (recommended on Garden).
if [[ -n "${RAD_REPO_PATH:-}" && -d "$RAD_REPO_PATH" ]]; then
  cd "$RAD_REPO_PATH"
fi

if ! bk_commit_is_new_issue "$commit" 2>/dev/null; then
  echo "webhook: commit is not a new issue root — ignoring" >&2
  exit 0
fi

exec "$SCRIPT_DIR/trigger-issue-build.sh" "$commit"
