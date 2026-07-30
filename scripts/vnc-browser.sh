#!/usr/bin/env bash
# Browser launcher for Codespace / desktop-lite VNC.
# Used as $BROWSER so OAuth (Bluesky) opens inside the noVNC desktop.
#
# Chromium needs --no-sandbox in this environment (user namespaces blocked).
# After launch we raise/maximize the window so it is not buried under Sleek.
set -euo pipefail

export DISPLAY="${DISPLAY:-:1}"
export LIBGL_ALWAYS_SOFTWARE="${LIBGL_ALWAYS_SOFTWARE:-1}"

URL="${1:-about:blank}"
USER_DATA="${SLEEK_BROWSER_PROFILE:-/tmp/sleek-chromium}"
mkdir -p "$USER_DATA"

pick() {
  command -v "$1" 2>/dev/null || true
}

# Raise any Chromium/Firefox window above Sleek (best-effort).
raise_browser() {
  sleep 1.2
  if command -v wmctrl >/dev/null 2>&1; then
    # Drop always-on-top / fullscreen on Sleek so the browser can be focused.
    wmctrl -x -r sleek.sleek -b remove,fullscreen,above 2>/dev/null || true
    wmctrl -x -r sleek.sleek -b add,below 2>/dev/null || true
    # Activate Chromium / Firefox by class or title.
    wmctrl -x -a chromium-browser.Chromium-browser 2>/dev/null \
      || wmctrl -x -a Chromium-browser.Chromium-browser 2>/dev/null \
      || wmctrl -a "Chromium" 2>/dev/null \
      || wmctrl -a "Sign in" 2>/dev/null \
      || wmctrl -a "Firefox" 2>/dev/null \
      || true
    wmctrl -x -r chromium-browser.Chromium-browser -b add,maximized_vert,maximized_horz 2>/dev/null \
      || wmctrl -a "Chromium" 2>/dev/null \
      || true
  fi
  if command -v xdotool >/dev/null 2>&1; then
    id="$(xdotool search --onlyvisible --class 'Chromium' 2>/dev/null | tail -1 || true)"
    if [[ -z "$id" ]]; then
      id="$(xdotool search --onlyvisible --name 'Chromium' 2>/dev/null | tail -1 || true)"
    fi
    if [[ -n "$id" ]]; then
      xdotool windowactivate --sync "$id" 2>/dev/null || true
      xdotool windowraise "$id" 2>/dev/null || true
    fi
  fi
}

CHROMIUM="$(pick chromium)"
[[ -z "$CHROMIUM" ]] && CHROMIUM="$(pick chromium-browser)"
FIREFOX="$(pick firefox)"

if [[ -n "$CHROMIUM" ]]; then
  # Raise in background after chromium starts (we exec, so fork the raiser first).
  (raise_browser) &
  exec "$CHROMIUM" \
    --no-sandbox \
    --disable-gpu \
    --disable-dev-shm-usage \
    --user-data-dir="$USER_DATA" \
    --new-window \
    --window-position=40,40 \
    --window-size=1100,700 \
    --start-maximized \
    "$URL"
fi

if [[ -n "$FIREFOX" ]]; then
  export MOZ_DISABLE_CONTENT_SANDBOX=1
  export MOZ_ENABLE_WAYLAND=0
  (raise_browser) &
  exec "$FIREFOX" --no-remote --new-window "$URL"
fi

if command -v xdg-open >/dev/null 2>&1; then
  exec xdg-open "$URL"
fi

echo "vnc-browser: no chromium/firefox on PATH — install: nix profile install nixpkgs#chromium" >&2
echo "  url: $URL" >&2
exit 1
