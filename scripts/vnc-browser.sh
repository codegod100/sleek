#!/usr/bin/env bash
# Browser launcher for Codespace / desktop-lite VNC.
# Used as $BROWSER so OAuth (Bluesky) opens inside the noVNC desktop.
#
# Chromium needs --no-sandbox in this environment (user namespaces blocked).
set -euo pipefail

export DISPLAY="${DISPLAY:-:1}"
export LIBGL_ALWAYS_SOFTWARE="${LIBGL_ALWAYS_SOFTWARE:-1}"

URL="${1:-about:blank}"
USER_DATA="${SLEEK_BROWSER_PROFILE:-/tmp/sleek-chromium}"
mkdir -p "$USER_DATA"

# Prefer PATH (nix profile), then common names.
pick() {
  command -v "$1" 2>/dev/null || true
}

CHROMIUM="$(pick chromium)"
[[ -z "$CHROMIUM" ]] && CHROMIUM="$(pick chromium-browser)"
FIREFOX="$(pick firefox)"

if [[ -n "$CHROMIUM" ]]; then
  exec "$CHROMIUM" \
    --no-sandbox \
    --disable-gpu \
    --disable-dev-shm-usage \
    --user-data-dir="$USER_DATA" \
    --new-window \
    "$URL"
fi

if [[ -n "$FIREFOX" ]]; then
  export MOZ_DISABLE_CONTENT_SANDBOX=1
  export MOZ_ENABLE_WAYLAND=0
  exec "$FIREFOX" --no-remote --new-window "$URL"
fi

# Last resort: xdg-open (often fails with no real browser)
if command -v xdg-open >/dev/null 2>&1; then
  exec xdg-open "$URL"
fi

echo "vnc-browser: no chromium/firefox on PATH — install: nix profile install nixpkgs#chromium" >&2
echo "  url: $URL" >&2
exit 1
