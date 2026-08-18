#!/usr/bin/env bash
# In-tree cargo-apk (x86_64) + Waydroid adb install/launch.
#
# Prefer the flake app (brings Rust + NDK + adb + display config):
#   nix run .#waydroid                 # debug (default)
#   nix run .#waydroid-release         # release (optimized + signed)
#   nix run .#waydroid -- build
#   nix run .#waydroid -- install
#   nix run .#waydroid -- launch
#   nix run .#waydroid -- --release run
#
# Display (set by flake app; override with env):
#   SLEEK_WAYDROID_WIDTH=1080
#   SLEEK_WAYDROID_HEIGHT=2400
#   SLEEK_WAYDROID_LCD_DENSITY=420
#   SLEEK_WAYDROID_SHOW_UI=1          # open show-full-ui window
#   SLEEK_WAYDROID_START_SESSION=1    # start session if stopped
#   SLEEK_WAYDROID_RELEASE=1          # cargo apk --release (or use --release)
#
# Or from an existing flake shell:
#   ./scripts/waydroid.sh
#   just waydroid
#   just waydroid-release
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/android"
PKG="uk.nandi.sleek"
# SleekActivity subclasses NativeActivity (classes.dex injected post cargo-apk).
ACTIVITY="$PKG/uk.nandi.sleek.SleekActivity"
TARGET="${SLEEK_ANDROID_TARGET:-x86_64-linux-android}"
API_LEVEL="${SLEEK_ANDROID_API:-28}"

# Portrait phone defaults (flake app re-exports these; keep in sync with flake.nix).
SLEEK_WAYDROID_WIDTH="${SLEEK_WAYDROID_WIDTH:-1080}"
SLEEK_WAYDROID_HEIGHT="${SLEEK_WAYDROID_HEIGHT:-2400}"
SLEEK_WAYDROID_LCD_DENSITY="${SLEEK_WAYDROID_LCD_DENSITY:-420}"
SLEEK_WAYDROID_SHOW_UI="${SLEEK_WAYDROID_SHOW_UI:-1}"
SLEEK_WAYDROID_START_SESSION="${SLEEK_WAYDROID_START_SESSION:-1}"
# 1 / true / yes → cargo apk --release (also: --release flag, or nix run .#waydroid-release).
RELEASE=0
case "${SLEEK_WAYDROID_RELEASE:-0}" in
  1 | true | yes | TRUE | YES) RELEASE=1 ;;
esac

export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-$HOME/.local/share/android-ndk-r29}}"
export ANDROID_NDK_ROOT="$ANDROID_NDK_HOME"
export ANDROID_HOME="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/.local/share/android-sdk}}"
export ANDROID_SDK_ROOT="${ANDROID_SDK_ROOT:-$ANDROID_HOME}"

need() { command -v "$1" >/dev/null || { echo "missing: $1 (use: nix run .#waydroid or .#waydroid-release)" >&2; exit 1; }; }

need waydroid
need cargo
need adb
need cargo-apk
need rustc

# Prefer NDK llvm + platform-tools when present (host layout or flake app PATH).
if [[ -d "$ANDROID_NDK_HOME/toolchains/llvm/prebuilt" ]]; then
  prebuilt=""
  for host in linux-x86_64 linux-aarch64; do
    if [[ -d "$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$host/bin" ]]; then
      prebuilt="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$host/bin"
      break
    fi
  done
  if [[ -n "$prebuilt" ]]; then
    export PATH="$prebuilt:$PATH"
  fi
fi
if [[ -d "$ANDROID_HOME/platform-tools" ]]; then
  export PATH="$ANDROID_HOME/platform-tools:$PATH"
fi

triple_upper="$(echo "$TARGET" | tr '[:lower:]-' '[:upper:]_')"
clang_bin="${TARGET}${API_LEVEL}-clang"
export "CC_${TARGET//-/_}=$clang_bin"
export "CARGO_TARGET_${triple_upper}_LINKER=$clang_bin"
export "AR_${TARGET//-/_}=llvm-ar"
export "CARGO_TARGET_${triple_upper}_AR=llvm-ar"

if ! rustc --print sysroot --target "$TARGET" >/dev/null 2>&1; then
  echo "error: rustc missing target $TARGET" >&2
  echo "  rustup target add $TARGET   # or use: nix run .#waydroid" >&2
  exit 1
fi

[[ -f "$APP/Cargo.toml" ]] || {
  echo "error: android crate not found at $APP" >&2
  echo "  run from the sleek repo (or via nix run .#waydroid from the repo root)" >&2
  exit 1
}

waydroid_ip() {
  waydroid status 2>/dev/null | awk -F': *' '/IP address/{gsub(/[[:space:]]/,"",$2); print $2; exit}'
}

session_running() {
  waydroid status 2>/dev/null | grep -q 'Session:.*RUNNING'
}

# Apply portrait phone window size / density via waydroid props (idempotent).
apply_waydroid_config() {
  local w h d cur
  w="$SLEEK_WAYDROID_WIDTH"
  h="$SLEEK_WAYDROID_HEIGHT"
  d="$SLEEK_WAYDROID_LCD_DENSITY"

  echo "waydroid display: ${w}x${h} density=${d}" >&2

  # prop set only works with a running container/session.
  if ! session_running && [[ "${SLEEK_WAYDROID_START_SESSION}" != "1" ]]; then
    echo "note: session not RUNNING — skip prop set (start session first)" >&2
    return 0
  fi

  set_prop_if_needed() {
    local key="$1" want="$2"
    cur="$(waydroid prop get "$key" 2>/dev/null | tr -d '[:space:]' || true)"
    if [[ "$cur" == "$want" ]]; then
      return 0
    fi
    echo "  prop set $key=$want (was: ${cur:-unset})" >&2
    waydroid prop set "$key" "$want" >/dev/null 2>&1 || {
      echo "  warning: failed to set $key (needs running container)" >&2
    }
  }

  set_prop_if_needed persist.waydroid.width "$w"
  set_prop_if_needed persist.waydroid.height "$h"
  set_prop_if_needed persist.waydroid.lcd_density "$d"
  set_prop_if_needed ro.sf.lcd_density "$d"
}

ensure_session() {
  if session_running; then
    return 0
  fi
  if [[ "${SLEEK_WAYDROID_START_SESSION}" != "1" ]]; then
    echo "Waydroid session not RUNNING" >&2
    echo "  start with: waydroid session start" >&2
    echo "  or: SLEEK_WAYDROID_START_SESSION=1 nix run .#waydroid" >&2
    exit 1
  fi
  echo "starting Waydroid session…" >&2
  # Needs a Wayland/X display for the user session.
  if [[ -z "${WAYLAND_DISPLAY:-}" && -z "${DISPLAY:-}" ]]; then
    if [[ -n "${XDG_RUNTIME_DIR:-}" && -S "${XDG_RUNTIME_DIR}/wayland-0" ]]; then
      export WAYLAND_DISPLAY=wayland-0
    elif [[ -S /tmp/.X11-unix/X0 ]]; then
      export DISPLAY=:0
    fi
  fi
  waydroid session start >/dev/null 2>&1 &
  local i
  for i in $(seq 1 60); do
    if session_running; then
      echo "Waydroid session RUNNING" >&2
      return 0
    fi
    sleep 0.5
  done
  echo "error: Waydroid session failed to start" >&2
  waydroid status >&2 || true
  exit 1
}

ensure_adb() {
  ensure_session
  mkdir -p "$HOME/.android" "$HOME/.local/share/waydroid/data/misc/adb"
  [[ -f "$HOME/.android/adbkey" ]] || adb keygen "$HOME/.android/adbkey"
  cp "$HOME/.android/adbkey.pub" "$HOME/.local/share/waydroid/data/misc/adb/adb_keys" 2>/dev/null || true
  local ip
  ip="$(waydroid_ip)"
  ip="${ip:-192.168.240.112}"
  waydroid adb connect >/dev/null 2>&1 || adb connect "${ip}:5555" >/dev/null 2>&1 || true
  local _
  for _ in $(seq 1 40); do
    if adb devices 2>/dev/null | tr -d '\r' | grep -qE "${ip}:5555[[:space:]]+device"; then
      export ANDROID_SERIAL="${ip}:5555"
      export WAYDROID_IP="$ip"
      echo "ADB ready: $ANDROID_SERIAL" >&2
      return 0
    fi
    sleep 0.5
  done
  echo "ADB not ready for ${ip}:5555" >&2
  adb devices -l >&2 || true
  exit 1
}

# Freeform `waydroid app launch` often flashes then closes on wlroots/Niri.
# Keep the full Android UI window open, then start the activity inside it.
ensure_full_ui() {
  if [[ "${SLEEK_WAYDROID_SHOW_UI}" != "1" ]]; then
    return 0
  fi
  if pgrep -a waydroid 2>/dev/null | grep -qE 'show-full-ui|first-launch'; then
    return 0
  fi
  echo "Opening Waydroid full UI (${SLEEK_WAYDROID_WIDTH}x${SLEEK_WAYDROID_HEIGHT})…" >&2
  # Detach so nix run / just does not block on the window.
  nohup waydroid show-full-ui >/dev/null 2>&1 &
  local _
  for _ in $(seq 1 30); do
    if pgrep -a waydroid 2>/dev/null | grep -qE 'show-full-ui|first-launch'; then
      sleep 0.5
      return 0
    fi
    # Fallback: window may already be composing without matching the pgrep string.
    if adb shell dumpsys window 2>/dev/null | grep -q 'mCurrentFocus'; then
      sleep 0.5
      return 0
    fi
    sleep 0.3
  done
  echo "warning: show-full-ui may not have appeared — check compositor / WAYLAND_DISPLAY" >&2
}

# cargo-apk stages NDK shared libs as mode 0555; a later rebuild then fails with
# "Permission denied (os error 13)" when overwriting libc++_shared.so etc.
writable_apk_stage() {
  local profile="$1"
  local stage
  for stage in \
    "$APP/target/${profile}/apk" \
    "$APP/target/apk" \
    "$APP/target"; do
    if [[ -d "$stage" ]]; then
      chmod -R u+w "$stage" 2>/dev/null || true
    fi
  done
  rm -f "$APP/target/${profile}/apk/"*-unaligned.apk 2>/dev/null || true
}

find_apk() {
  local profile="$1"
  local apk=""
  local dir="$APP/target/${profile}/apk"
  for cand in \
    "$dir/sleek.apk" \
    "$APP/target/sleek.apk" \
    "$dir/sleek-${profile}.apk"; do
    if [[ -f "$cand" ]]; then
      echo "$cand"
      return 0
    fi
  done
  apk="$(find "$APP/target" -type f -path "*/${profile}/apk/*.apk" ! -name '*-unaligned.apk' 2>/dev/null | head -1 || true)"
  if [[ -z "${apk:-}" ]]; then
    apk="$(find "$APP/target" -type f \( -name 'sleek.apk' -o -name "*-${profile}.apk" \) ! -name '*-unaligned.apk' 2>/dev/null | head -1 || true)"
  fi
  if [[ -z "${apk:-}" ]]; then
    apk="$(find "$APP/target" -type f -name '*.apk' ! -name '*-unaligned.apk' 2>/dev/null | head -1 || true)"
  fi
  [[ -n "${apk:-}" && -f "$apk" ]] || return 1
  echo "$apk"
}

ensure_release_signing() {
  mkdir -p "$HOME/.android"
  local keystore="$APP/ci.keystore"
  if [[ ! -f "$keystore" ]]; then
    keystore="$HOME/.android/sleek-release.keystore"
  fi
  if [[ ! -f "$keystore" ]]; then
    need keytool
    echo "generating release keystore at $keystore" >&2
    keytool -genkeypair -v \
      -keystore "$keystore" \
      -storepass android \
      -alias androiddebugkey \
      -keypass android \
      -keyalg RSA -keysize 2048 -validity 10000 \
      -dname "CN=Sleek Local,O=Sleek,C=US" >&2
  fi
  export SLEEK_KEYSTORE="$keystore"
  export SLEEK_KEYSTORE_PASSWORD=android
  export SLEEK_KEY_ALIAS=androiddebugkey
  export SLEEK_KEY_PASSWORD=android

  # cargo-apk reads [package.metadata.android.signing.release] from Cargo.toml.
  # Inject before [patch] if missing (do not commit).
  if ! grep -q 'signing.release' "$APP/Cargo.toml"; then
    python3 - "$APP/Cargo.toml" "$keystore" <<'PY'
import pathlib, sys
path = pathlib.Path(sys.argv[1])
keystore = sys.argv[2]
text = path.read_text()
block = f"""
[package.metadata.android.signing.release]
path = "{keystore}"
keystore_password = "android"
key_alias = "androiddebugkey"
key_password = "android"
"""
marker = "[patch.crates-io]"
if marker in text:
    text = text.replace(marker, block + "\n" + marker, 1)
else:
    text = text + block
path.write_text(text)
PY
    echo "note: injected signing.release into android/Cargo.toml (local only)" >&2
  fi
}

build_apk() {
  [[ -d "$ANDROID_NDK_HOME" ]] || {
    echo "Set ANDROID_NDK_HOME (got: $ANDROID_NDK_HOME)" >&2
    echo "  flake app sets this from androidenv; or install an NDK under ~/.local/share" >&2
    exit 1
  }
  [[ -d "$ANDROID_HOME" ]] || {
    echo "Set ANDROID_HOME (got: $ANDROID_HOME)" >&2
    exit 1
  }

  local profile=debug
  local -a apk_args=(build --target "$TARGET" -p sleek --lib)
  if [[ "$RELEASE" -eq 1 ]]; then
    profile=release
    apk_args+=(--release)
    # cargo-apk --release needs signing metadata.
    ensure_release_signing
  fi

  echo "cargo apk ${apk_args[*]}  (in-tree → $APP)" >&2
  echo "  ANDROID_HOME=$ANDROID_HOME" >&2
  echo "  ANDROID_NDK_HOME=$ANDROID_NDK_HOME" >&2
  echo "  target=$TARGET profile=$profile" >&2

  writable_apk_stage "$profile"

  (
    cd "$APP"
    # Optional full clean: SLEEK_WAYDROID_CLEAN=1 nix run .#waydroid
    if [[ "${SLEEK_WAYDROID_CLEAN:-0}" == "1" ]]; then
      cargo clean -p sleek --target "$TARGET" >&2 2>/dev/null || true
    fi
    cargo apk "${apk_args[@]}" >&2 || {
      echo "error: cargo apk failed" >&2
      exit 1
    }
  ) || {
    echo "error: APK build failed — not installing a stale package" >&2
    exit 1
  }

  local apk
  apk="$(find_apk "$profile")" || {
    echo "APK not found under $APP/target" >&2
    exit 1
  }

  # Guard against installing a stub without native libs.
  if command -v python3 >/dev/null 2>&1; then
    if ! python3 - "$apk" <<'PY'
import sys, zipfile
with zipfile.ZipFile(sys.argv[1]) as z:
    ok = any(n.startswith("lib/") and n.endswith(".so") for n in z.namelist())
sys.exit(0 if ok else 1)
PY
    then
      echo "error: $apk has no native libs — cargo-apk packaging failed?" >&2
      echo "  try: SLEEK_WAYDROID_CLEAN=1 nix run .#waydroid" >&2
      exit 1
    fi
  fi

  # Inject uk.nandi.sleek.SleekActivity as classes.dex (freeq:// OAuth handler).
  bash "$APP/scripts/inject-activity-dex.sh" "$apk" "$APP/src/assets/sleek_activity.dex"
  echo "$apk"
}

install_apk() {
  local apk="$1"
  ensure_adb
  apply_waydroid_config
  echo "install $apk" >&2
  # Prefer waydroid app install (avoids adb incremental install failures).
  if waydroid app install "$apk" >&2; then
    echo "ok: installed $PKG (waydroid app install)" >&2
    return 0
  fi
  echo "waydroid app install failed — falling back to adb install -r" >&2
  adb install -r "$apk" >&2
  echo "ok: installed $PKG" >&2
}

launch_app() {
  ensure_adb
  apply_waydroid_config
  ensure_full_ui
  echo "launch $ACTIVITY" >&2
  adb shell am force-stop "$PKG" >/dev/null 2>&1 || true
  adb shell am start -n "$ACTIVITY" >&2
  sleep 0.8
  local pid
  pid="$(adb shell pidof "$PKG" 2>/dev/null | tr -d '\r' || true)"
  if [[ -z "$pid" ]]; then
    echo "warning: process not running — check: adb logcat | grep -i sleek" >&2
  else
    echo "Launched $PKG (pid $pid) on ${WAYDROID_IP:-waydroid}" >&2
  fi
  if [[ "${SLEEK_WAYDROID_SHOW_UI}" == "1" ]]; then
    echo "Focus the Waydroid window if you do not see Sleek." >&2
  fi
}

usage() {
  cat <<EOF >&2
usage: $0 [options] [build|install|launch|run]

  run      build APK + install + launch + show-full-ui (default)
  build    cargo-apk only (print APK path)
  install  build + install
  launch   start activity + show-full-ui (no rebuild)

options:
  --release       cargo apk build --release (default: debug)
  -h, --help      this help

env (flake app sets defaults):
  ANDROID_HOME / ANDROID_NDK_HOME   SDK + NDK
  SLEEK_ANDROID_TARGET              default x86_64-linux-android
  SLEEK_WAYDROID_WIDTH              default 1080
  SLEEK_WAYDROID_HEIGHT             default 2400
  SLEEK_WAYDROID_LCD_DENSITY        default 420
  SLEEK_WAYDROID_SHOW_UI=0|1        open show-full-ui (default 1)
  SLEEK_WAYDROID_START_SESSION=0|1  start session if stopped (default 1)
  SLEEK_WAYDROID_RELEASE=0|1        same as --release (waydroid-release sets 1)
  SLEEK_WAYDROID_CLEAN=1            cargo clean before build

flake apps:
  nix run .#waydroid              # debug iterate
  nix run .#waydroid-release      # release iterate
EOF
}

cmd=run
while [[ $# -gt 0 ]]; do
  case "$1" in
    -h | --help)
      usage
      exit 0
      ;;
    --release)
      RELEASE=1
      shift
      ;;
    build | install | launch | run)
      cmd="$1"
      shift
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage
      exit 1
      ;;
  esac
done

case "$cmd" in
  build)
    apk="$(build_apk)"
    echo "$apk"
    ;;
  install)
    apk="$(build_apk)"
    install_apk "$apk"
    echo "$apk"
    ;;
  launch)
    launch_app
    ;;
  run)
    apk="$(build_apk)"
    install_apk "$apk"
    launch_app
    echo "$apk"
    ;;
  *)
    usage
    exit 1
    ;;
esac
