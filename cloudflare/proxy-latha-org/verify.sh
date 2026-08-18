#!/usr/bin/env bash
# End-to-end check: push a trivial commit to tangled/main, then poll
# proxy.latha.org until the rebuilt artifact shows up (or timeout).
#
# Usage: ./verify.sh   (run from the sleek repo root, `tangled` remote set)
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

marker="ci: proxy.latha.org pipeline smoke test $(date -u +%FT%TZ)"
git commit --allow-empty -m "$marker" >/dev/null
sha="$(git rev-parse HEAD)"
echo "pushing $sha to tangled/main ..." >&2
git push tangled main

url="https://proxy.latha.org/artifacts/$sha/sleek.apk"
echo "polling $url (up to 20min) ..." >&2
for i in $(seq 1 120); do
  code="$(curl -s -o /dev/null -w '%{http_code}' "$url" || true)"
  if [[ "$code" == "200" ]]; then
    echo "ok: artifact live at $url" >&2
    exit 0
  fi
  sleep 10
done

echo "timeout: no artifact at $url after 20min — check BuildBuddy invocation list" >&2
exit 1
