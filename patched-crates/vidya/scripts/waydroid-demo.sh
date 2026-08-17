#!/usr/bin/env bash
# In-tree APK build + Waydroid install/launch.
#   just waydroid
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEMO="$ROOT/android-demo"
PKG="uk.nandi.vidya.demo"
ACTIVITY="$PKG/android.app.NativeActivity"
OUT="$ROOT/docs/screenshots/mobile"

export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$HOME/.local/share/android-ndk-r29}"
export ANDROID_NDK_ROOT="$ANDROID_NDK_HOME"
export ANDROID_HOME="${ANDROID_HOME:-$HOME/.local/share/android-sdk}"
unset ANDROID_SDK_ROOT 2>/dev/null || true

need() { command -v "$1" >/dev/null || { echo "missing: $1" >&2; exit 1; }; }

export PATH="${ANDROID_NDK_HOME}/toolchains/llvm/prebuilt/linux-x86_64/bin:${ANDROID_HOME}/platform-tools:${HOME}/.cargo/bin:${PATH}"

need waydroid
need cargo
need adb
need cargo-apk
need rustc

export CC_x86_64_linux_android="${CC_x86_64_linux_android:-x86_64-linux-android28-clang}"
export CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER="${CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER:-$CC_x86_64_linux_android}"
export AR_x86_64_linux_android="${AR_x86_64_linux_android:-llvm-ar}"

if ! rustc --print sysroot --target x86_64-linux-android >/dev/null 2>&1; then
  echo "error: rustc missing x86_64-linux-android" >&2
  echo "  rustup target add x86_64-linux-android" >&2
  exit 1
fi

waydroid_ip() {
  waydroid status 2>/dev/null | awk -F': *' '/IP address/{gsub(/[[:space:]]/,"",$2); print $2; exit}'
}

ensure_adb() {
  waydroid status | grep -q 'Session:.*RUNNING' || {
    echo "Waydroid session not RUNNING" >&2
    exit 1
  }
  mkdir -p "$HOME/.android" "$HOME/.local/share/waydroid/data/misc/adb"
  [[ -f "$HOME/.android/adbkey" ]] || adb keygen "$HOME/.android/adbkey"
  cp "$HOME/.android/adbkey.pub" "$HOME/.local/share/waydroid/data/misc/adb/adb_keys" 2>/dev/null || true
  local ip
  ip="$(waydroid_ip)"
  ip="${ip:-192.168.240.112}"
  waydroid adb connect >/dev/null 2>&1 || adb connect "${ip}:5555" >/dev/null 2>&1 || true
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
  exit 1
}

# stdout = APK path only
build_apk() {
  [[ -d "$ANDROID_NDK_HOME" ]] || {
    echo "Set ANDROID_NDK_HOME=$ANDROID_NDK_HOME" >&2
    exit 1
  }

  echo "cargo apk (in-tree) → $DEMO" >&2
  echo "rustc $(rustc --version) | $(command -v rustc)" >&2

  (
    cd "$DEMO"
    # Force a real rebuild so UI fixes are not skipped (0.18s "Finished" = stale).
    cargo clean -p vidya-demo --target x86_64-linux-android >&2 2>/dev/null || true
    cargo apk build --target x86_64-linux-android >&2
  )

  local apk="$DEMO/target/debug/apk/vidya-demo.apk"
  if [[ ! -f "$apk" ]]; then
    apk="$(find "$DEMO/target" -name 'vidya-demo.apk' 2>/dev/null | head -1 || true)"
  fi
  if [[ ! -f "$apk" ]]; then
    echo "APK not found under $DEMO/target" >&2
    exit 1
  fi
  printf '%s\n' "$apk"
}

# Prefer `waydroid app install` (no adb incremental spam). Fall back to adb.
install_apk() {
  local apk=$1
  echo "Installing $apk ($(du -h "$apk" | awk '{print $1}'))" >&2
  if waydroid app install "$apk" >&2; then
    return 0
  fi
  # Fallback: full reinstall via adb (Waydroid rejects incremental)
  adb uninstall "$PKG" >/dev/null 2>&1 || true
  adb install "$apk" >/dev/null
  echo "Installed via adb" >&2
}

write_section_file() {
  local sec="${VIDYA_DEMO_SECTION:-}"
  if [[ -n "$sec" ]]; then
    adb shell "echo -n '${sec}' >/data/local/tmp/vidya_section" || true
  fi
}

# Freeform `waydroid app launch` often flashes then closes on wlroots/Niri.
# Keep the full Android UI window open, then start the activity inside it.
ensure_full_ui() {
  # Already have a waydroid surface / full-ui?
  if pgrep -a waydroid 2>/dev/null | grep -q 'show-full-ui\|first-launch'; then
    return 0
  fi
  # Cage / weston child sometimes named differently — also check for surfaceflinger clients
  if adb shell pidof surfaceflinger >/dev/null 2>&1; then
    echo "Opening Waydroid full UI window…" >&2
    # Detach so just waydroid does not block on the window
    nohup waydroid show-full-ui >/dev/null 2>&1 &
    # Wait until the display is actually composing
    for _ in $(seq 1 30); do
      if adb shell dumpsys window 2>/dev/null | grep -q 'mCurrentFocus'; then
        sleep 0.5
        return 0
      fi
      sleep 0.3
    done
  fi
}

launch_app() {
  ensure_full_ui
  write_section_file
  adb shell am force-stop "$PKG" >/dev/null 2>&1 || true
  # Start inside the full UI (more stable than freeform app windows)
  adb shell am start -n "$ACTIVITY" >/dev/null
  sleep 0.8
  local pid
  pid="$(adb shell pidof "$PKG" 2>/dev/null | tr -d '\r' || true)"
  if [[ -z "$pid" ]]; then
    echo "warning: process exited immediately — check: adb logcat | grep vidya" >&2
    exit 1
  fi
  echo "Launched $PKG (pid $pid) on ${WAYDROID_IP:-waydroid}" >&2
  echo "If you only saw a flash: focus the Waydroid window (full Android UI)." >&2
}

cmd_install() {
  ensure_adb
  local apk
  apk="$(build_apk)"
  install_apk "$apk"
  launch_app
}

cmd_launch() {
  ensure_adb
  launch_app
}

cmd_shots() {
  ensure_adb
  mkdir -p "$OUT"
  local pairs=(
    "overview:home"
    "typography:type"
    "actions:actions"
    "surfaces:surfaces"
    "palette:colors"
    "forms:forms"
  )
  for pair in "${pairs[@]}"; do
    local section="${pair%%:*}" name="${pair##*:}"
    echo "→ $section" >&2
    VIDYA_DEMO_SECTION="$section" launch_app
    sleep 2.5
    adb exec-out screencap -p >"$OUT/vidya-${name}.png"
    ls -la "$OUT/vidya-${name}.png" >&2
  done
}

case "${1:-}" in
  run|install) cmd_install ;;
  launch) cmd_launch ;;
  shots) cmd_shots ;;
  *)
    echo "Usage: $0 {run|install|launch|shots}" >&2
    exit 1
    ;;
esac
