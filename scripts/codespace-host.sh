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
VIDYA_URL="${SLEEK_VIDYA_URL:-https://tangled.org/nandi.uk/vidya}"

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
    log "noVNC: https://${CODESPACE_NAME}-6080.app.github.dev  (password: vscode)"
  fi
  exit 0
fi

run_host
