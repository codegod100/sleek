#!/usr/bin/env bash
# In-tree APK build + Waydroid install/launch (mirrors vidya/scripts/waydroid-demo.sh).
#   just waydroid
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/android"
PKG="uk.nandi.sleek"
# SleekActivity subclasses NativeActivity (classes.dex injected post cargo-apk).
ACTIVITY="$PKG/uk.nandi.sleek.SleekActivity"

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

build_apk() {
  [[ -d "$ANDROID_NDK_HOME" ]] || {
    echo "Set ANDROID_NDK_HOME=$ANDROID_NDK_HOME" >&2
    exit 1
  }

  echo "cargo apk (in-tree) → $APP" >&2
  # cargo-apk stages NDK libs as mode 0555; rebuilds fail with Permission denied otherwise.
  if [[ -d "$APP/target/debug/apk" ]]; then
    chmod -R u+w "$APP/target/debug/apk" 2>/dev/null || true
  fi
  rm -f "$APP/target/debug/apk/"*-unaligned.apk 2>/dev/null || true
  (
    cd "$APP"
    cargo clean -p sleek --target x86_64-linux-android >&2 2>/dev/null || true
    cargo apk build --target x86_64-linux-android >&2
  )

  local apk
  # Prefer final sleek.apk; never install *-unaligned stubs.
  if [[ -f "$APP/target/debug/apk/sleek.apk" ]]; then
    apk="$APP/target/debug/apk/sleek.apk"
  else
    apk="$(find "$APP/target" -type f \( -name 'sleek.apk' -o -name '*-debug.apk' \) ! -name '*-unaligned.apk' 2>/dev/null | head -1)"
  fi
  if [[ -z "${apk:-}" ]]; then
    apk="$(find "$APP/target" -type f -name '*.apk' ! -name '*-unaligned.apk' 2>/dev/null | head -1)"
  fi
  [[ -n "${apk:-}" && -f "$apk" ]] || {
    echo "APK not found under $APP/target" >&2
    exit 1
  }
  # Inject uk.nandi.sleek.SleekActivity as classes.dex (freeq:// OAuth handler).
  bash "$APP/scripts/inject-activity-dex.sh" "$apk" "$APP/src/assets/sleek_activity.dex"
  echo "$apk"
}

install_apk() {
  local apk="$1"
  ensure_adb
  echo "install $apk" >&2
  adb install -r "$apk" >&2
}

launch_app() {
  ensure_adb
  echo "launch $ACTIVITY" >&2
  adb shell am start -n "$ACTIVITY" >&2
}

cmd="${1:-run}"
case "$cmd" in
  build)
    build_apk
    ;;
  install)
    apk="$(build_apk)"
    install_apk "$apk"
    ;;
  launch)
    launch_app
    ;;
  run)
    apk="$(build_apk)"
    install_apk "$apk"
    launch_app
    ;;
  *)
    echo "usage: $0 {build|install|launch|run}" >&2
    exit 1
    ;;
esac
