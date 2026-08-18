#!/usr/bin/env bash
# One-shot: ensure sibling deps + run the desktop host (Codespace VNC / local).
#
# Path deps resolve as:
#   /workspaces/sleek/android → ../../freeq/freeq-sdk, ../../vidya
# so freeq + vidya must sit next to the sleek checkout.
#
# Usage (inside the codespace or any machine with the same layout):
#   bash scripts/codespace-host.sh           # foreground (just host)
#   bash scripts/codespace-host.sh --bg      # background + log
#   bash scripts/codespace-host.sh --deps    # only clone freeq/vidya
#
# From your laptop:
#   gh codespace ssh -c <name> -- bash /workspaces/sleek/scripts/codespace-host.sh --bg
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORKSPACES="$(cd "$ROOT/.." && pwd)"
LOG_DIR="${SLEEK_HOST_LOG_DIR:-/tmp/sleek-logs}"
LOG="$LOG_DIR/host.log"
FREEQ_URL="${SLEEK_FREEQ_URL:-https://github.com/codegod100/freeq.git}"
VIDYA_URL="${SLEEK_VIDYA_URL:-https://nandi.radicle.garden/z2UqGTRH21s3pHnJgSuMwRaPPNNcW.git}"

MODE=fg
for arg in "$@"; do
  case "$arg" in
    --bg | -d | --daemon) MODE=bg ;;
    --deps) MODE=deps ;;
    -h | --help)
      sed -n '2,16p' "$0" | sed 's/^# \?//'
      exit 0
      ;;
    *)
      echo "unknown arg: $arg (try --bg | --deps)" >&2
      exit 2
      ;;
  esac
done

log() { echo "sleek-host: $*" >&2; }

# desktop-lite defaults to 1440x768 — often wider than a browser tab, so the
# remote desktop is bigger than the noVNC viewport (scroll/pan). Shrink the X
# screen when possible. Override: SLEEK_VIEWPORT=1280x720
# noVNC also scales if opened with ?resize=scale (see README / bootstrap).
fit_vnc_desktop() {
  export DISPLAY="${DISPLAY:-:1}"
  if [[ ! -S /tmp/.X11-unix/X1 && ! -S /tmp/.X11-unix/X0 ]]; then
    return 0
  fi
  if ! command -v xrandr >/dev/null 2>&1; then
    return 0
  fi
  local geom="${SLEEK_VIEWPORT:-1280x720}"
  geom="${geom%%x[0-9]}" # strip optional depth from 1280x720x16 if present
  # Accept WxH or WxHxD
  local wh
  wh="$(echo "$geom" | sed -E 's/^([0-9]+x[0-9]+).*/\1/')"
  [[ "$wh" =~ ^[0-9]+x[0-9]+$ ]] || wh="1280x720"
  local out
  out="$(xrandr 2>/dev/null | awk '/ connected/{print $1; exit}')"
  if [[ -z "$out" ]]; then
    return 0
  fi
  if xrandr --query 2>/dev/null | grep -qE "^${out} connected.*${wh}"; then
    log "VNC desktop already ${wh}"
    return 0
  fi
  if xrandr --query 2>/dev/null | grep -qE "^\s+${wh}\s"; then
    log "resizing VNC desktop → ${wh} (was bigger than typical browser viewport)"
    xrandr --output "$out" --mode "$wh" 2>/dev/null || true
  else
    log "mode ${wh} not listed; leave geometry as-is (use noVNC ?resize=scale)"
  fi
}

# Prefer noVNC "Local scaling" so a large remote desktop still fits the browser.
prefer_novnc_scale() {
  local ui
  for ui in /usr/local/novnc/noVNC*/app/ui.js; do
    [[ -f "$ui" ]] || continue
    if grep -q "initSetting('resize', 'off')" "$ui" 2>/dev/null; then
      if sed -i "s/initSetting('resize', 'off')/initSetting('resize', 'scale')/" "$ui" 2>/dev/null; then
        log "noVNC default resize → scale ($ui)"
      elif command -v sudo >/dev/null 2>&1; then
        sudo sed -i "s/initSetting('resize', 'off')/initSetting('resize', 'scale')/" "$ui" 2>/dev/null \
          && log "noVNC default resize → scale ($ui)" || true
      fi
    fi
  done
}

ensure_clone() {
  local dest="$1" url="$2" name="$3"
  if [[ -f "$dest/Cargo.toml" ]] || [[ -f "$dest/freeq-sdk/Cargo.toml" ]]; then
    return 0
  fi
  if [[ -d "$dest/.git" ]]; then
    log "$name present but incomplete at $dest"
    return 1
  fi
  log "cloning $name → $dest"
  git clone --depth 1 "$url" "$dest"
}

ensure_deps() {
  mkdir -p "$WORKSPACES"
  # Prefer flake.lock inputs (Radicle vidya + GitHub freeq) for path deps.
  if [[ -x "$ROOT/scripts/sync-flake-path-deps.sh" ]] && command -v nix >/dev/null 2>&1; then
    if bash "$ROOT/scripts/sync-flake-path-deps.sh"; then
      return 0
    fi
    log "flake sync failed; falling back to git clone"
  fi
  ensure_clone "$WORKSPACES/freeq" "$FREEQ_URL" freeq
  ensure_clone "$WORKSPACES/vidya" "$VIDYA_URL" vidya
  if [[ ! -f "$WORKSPACES/freeq/freeq-sdk/Cargo.toml" ]]; then
    log "missing freeq-sdk at $WORKSPACES/freeq/freeq-sdk"
    exit 1
  fi
  if [[ ! -f "$WORKSPACES/vidya/Cargo.toml" ]]; then
    log "missing vidya at $WORKSPACES/vidya"
    exit 1
  fi
  log "deps ok (freeq + vidya under $WORKSPACES)"
}

run_host() {
  cd "$ROOT"
  export PATH="/nix/var/nix/profiles/default/bin:${HOME}/.nix-profile/bin:${PATH:-}"
  export NIX_CONFIG="${NIX_CONFIG:-experimental-features = nix-command flakes}"
  export DISPLAY="${DISPLAY:-:1}"
  export LIBGL_ALWAYS_SOFTWARE="${LIBGL_ALWAYS_SOFTWARE:-1}"
  # just host uses this for Codespace display + software GL defaults
  export SLEEK_CODESPACE="${SLEEK_CODESPACE:-1}"
  # Bluesky OAuth needs a real browser inside VNC (not host xdg-open).
  if [[ -x "$ROOT/scripts/vnc-browser.sh" ]]; then
    export BROWSER="${BROWSER:-$ROOT/scripts/vnc-browser.sh}"
  fi

  if [[ -S /tmp/.X11-unix/X1 ]]; then
    export DISPLAY=:1
  elif [[ -S /tmp/.X11-unix/X0 && -z "${DISPLAY:-}" ]]; then
    export DISPLAY=:0
  fi

  if ! command -v nix >/dev/null 2>&1; then
    log "nix not on PATH — run: bash scripts/codespace-bootstrap.sh"
    exit 1
  fi

  # Prefer enter/just so LD_LIBRARY_PATH + LIBCLANG_PATH come from the flake.
  if [[ -x "$ROOT/scripts/enter" ]]; then
    exec "$ROOT/scripts/enter" just host
  fi
  exec nix develop -c just host
}

ensure_deps
if [[ "$MODE" == deps ]]; then
  exit 0
fi

# Before launch: shrink oversized X desktop + prefer noVNC scale-to-fit.
if [[ -n "${SLEEK_CODESPACE:-}" || -n "${CODESPACE_NAME:-}" || -S /tmp/.X11-unix/X1 ]]; then
  prefer_novnc_scale || true
  fit_vnc_desktop || true
fi

if [[ "$MODE" == bg ]]; then
  mkdir -p "$LOG_DIR"
  # Avoid stacking hosts
  if pgrep -f 'host/target/debug/sleek|host/target/release/sleek' >/dev/null 2>&1; then
    log "sleek already running:"
    pgrep -af 'host/target/.*/sleek' || true
    log "log: $LOG"
    exit 0
  fi
  log "starting host in background → $LOG"
  # shellcheck disable=SC2086
  nohup bash "$0" >>"$LOG" 2>&1 &
  echo $! >"$LOG_DIR/host.pid"
  log "pid $(cat "$LOG_DIR/host.pid")  tail -f $LOG"
  if [[ -n "${CODESPACE_NAME:-}" ]]; then
    log "noVNC: https://${CODESPACE_NAME}-6080.app.github.dev/vnc.html?resize=scale&autoconnect=true&password=vscode"
  fi
  exit 0
fi

run_host
