#!/usr/bin/env bash
# In-tree cargo-apk (phone / aarch64) + adb install on a connected device.
#
# Prefer the flake app (brings Rust + NDK + adb into PATH):
#   nix run .#deploy-android
#   nix run .#deploy-android -- --launch
#   nix run .#deploy-android -- --release --launch
#
# Or from an existing flake shell:
#   ./scripts/deploy-android.sh
#   just deploy-android
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/android"
PKG="uk.nandi.sleek"
# SleekActivity subclasses NativeActivity (classes.dex injected post cargo-apk).
ACTIVITY="$PKG/uk.nandi.sleek.SleekActivity"
TARGET="${SLEEK_ANDROID_TARGET:-aarch64-linux-android}"
API_LEVEL="${SLEEK_ANDROID_API:-28}"

RELEASE=0
LAUNCH=0
BUILD_ONLY=0
INSTALL_ONLY=0
LAUNCH_ONLY=0
CLEAN=0

usage() {
  cat <<EOF >&2
usage: $0 [options] [run|build|install|launch]

  run      build APK + adb install -r (default)
  build    cargo-apk only (print APK path)
  install  build + adb install -r
  launch   adb am start only (no rebuild)

options:
  --release       cargo apk build --release (default: debug)
  --launch        also start the activity after install
  --build-only    same as "build"
  --install-only  install existing APK under android/target (no rebuild)
  --clean         cargo clean -p sleek for the target before build
  -h, --help      this help

env:
  ANDROID_HOME / ANDROID_NDK_HOME   SDK + NDK (flake app sets these)
  ANDROID_SERIAL                    pin adb device when several are attached
  SLEEK_ANDROID_TARGET              default aarch64-linux-android
EOF
}

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
    --launch)
      LAUNCH=1
      shift
      ;;
    --build-only)
      BUILD_ONLY=1
      shift
      ;;
    --install-only)
      INSTALL_ONLY=1
      shift
      ;;
    --clean)
      CLEAN=1
      shift
      ;;
    run)
      shift
      ;;
    build)
      BUILD_ONLY=1
      shift
      ;;
    install)
      BUILD_ONLY=0
      INSTALL_ONLY=0
      LAUNCH_ONLY=0
      shift
      ;;
    launch)
      LAUNCH_ONLY=1
      shift
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage
      exit 1
      ;;
  esac
done

need() { command -v "$1" >/dev/null || { echo "missing: $1 (use: nix run .#deploy-android)" >&2; exit 1; }; }

need cargo
need adb
need cargo-apk
need rustc

export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-$HOME/.local/share/android-ndk-r29}}"
export ANDROID_NDK_ROOT="$ANDROID_NDK_HOME"
export ANDROID_HOME="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/.local/share/android-sdk}}"
export ANDROID_SDK_ROOT="${ANDROID_SDK_ROOT:-$ANDROID_HOME}"

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
  echo "  rustup target add $TARGET   # or use: nix run .#deploy-android" >&2
  exit 1
fi

[[ -f "$APP/Cargo.toml" ]] || {
  echo "error: android crate not found at $APP" >&2
  echo "  run from the sleek repo (or via nix run .#deploy-android from the repo root)" >&2
  exit 1
}

# Devices currently in adb "device" state (serials only; strips \r from Windows adb).
adb_device_serials() {
  adb devices 2>/dev/null | tr -d '\r' | awk 'NR > 1 && $2 == "device" { print $1 }'
}

# Prefer a physical USB phone for aarch64 deploys; Waydroid shows up as host:port.
pick_adb_serial() {
  local -a serials=()
  local s
  while IFS= read -r s; do
    [[ -n "$s" ]] && serials+=("$s")
  done < <(adb_device_serials)

  if [[ ${#serials[@]} -eq 0 ]]; then
    return 1
  fi

  if [[ -n "${ANDROID_SERIAL:-}" ]]; then
    for s in "${serials[@]}"; do
      if [[ "$s" == "$ANDROID_SERIAL" ]]; then
        echo "$ANDROID_SERIAL"
        return 0
      fi
    done
    echo "ANDROID_SERIAL=$ANDROID_SERIAL is not in 'device' state." >&2
    return 1
  fi

  if [[ ${#serials[@]} -eq 1 ]]; then
    echo "${serials[0]}"
    return 0
  fi

  # Multiple devices: for phone (aarch64) prefer USB serials over tcpip/Waydroid.
  local -a preferred=()
  local -a others=()
  for s in "${serials[@]}"; do
    if [[ "$s" == *:* ]]; then
      others+=("$s")
    else
      preferred+=("$s")
    fi
  done

  if [[ ${#preferred[@]} -eq 1 ]]; then
    echo "multiple adb devices; using USB ${preferred[0]} (set ANDROID_SERIAL to override)" >&2
    echo "${preferred[0]}"
    return 0
  fi

  if [[ ${#preferred[@]} -gt 1 ]]; then
    echo "multiple USB adb devices — set ANDROID_SERIAL to one of:" >&2
    printf '  %s\n' "${preferred[@]}" >&2
    adb devices -l >&2 || true
    return 1
  fi

  # Only tcpip devices (e.g. several emulators / Waydroid).
  echo "multiple adb devices (no USB phone) — set ANDROID_SERIAL to one of:" >&2
  printf '  %s\n' "${serials[@]}" >&2
  adb devices -l >&2 || true
  return 1
}

ensure_adb() {
  echo "waiting for adb device…" >&2
  local serial
  if ! serial="$(pick_adb_serial)"; then
    if [[ -z "$(adb_device_serials | head -1)" ]]; then
      echo "No adb device in 'device' state." >&2
      echo "Enable USB debugging and authorize this computer, then re-run." >&2
    fi
    adb devices -l >&2 || true
    exit 1
  fi
  export ANDROID_SERIAL="$serial"
  echo "adb device: $ANDROID_SERIAL ($(adb get-state 2>/dev/null || echo '?'))" >&2
  adb devices -l >&2 || true
}

find_apk() {
  local profile="$1"
  local apk=""
  local dir="$APP/target/${profile}/apk"
  # Prefer the final signed package; never install *-unaligned leftovers.
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
  # Drop tiny/broken intermediate so find_apk cannot prefer it over a real package.
  rm -f "$APP/target/${profile}/apk/"*-unaligned.apk 2>/dev/null || true
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
    if [[ "$CLEAN" -eq 1 ]]; then
      cargo clean -p sleek --target "$TARGET" >&2 2>/dev/null || true
    fi
    cargo apk "${apk_args[@]}" >&2
  )

  local apk
  apk="$(find_apk "$profile")" || {
    echo "APK not found under $APP/target" >&2
    exit 1
  }

  # Guard against installing a stub (e.g. unaligned leftovers without .so).
  # Use python3 (always in the flake app) — unzip is not.
  if ! python3 - "$apk" <<'PY'
import sys, zipfile
with zipfile.ZipFile(sys.argv[1]) as z:
    ok = any(n.startswith("lib/") and n.endswith(".so") for n in z.namelist())
sys.exit(0 if ok else 1)
PY
  then
    echo "error: $apk has no native libs — cargo-apk packaging failed?" >&2
    echo "  try: chmod -R u+w $APP/target && rm -rf $APP/target/${profile}/apk && re-run" >&2
    exit 1
  fi

  # Inject uk.nandi.sleek.SleekActivity as classes.dex (freeq:// OAuth handler).
  bash "$APP/scripts/inject-activity-dex.sh" "$apk" "$APP/src/assets/sleek_activity.dex"
  echo "$apk"
}

ensure_release_signing() {
  mkdir -p "$HOME/.android"
  local keystore="$HOME/.android/sleek-release.keystore"
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
  # Inject ephemerally if missing (do not commit).
  if ! grep -q 'signing.release' "$APP/Cargo.toml"; then
    cat >>"$APP/Cargo.toml" <<EOF

[package.metadata.android.signing.release]
path = "$keystore"
keystore_password = "android"
key_alias = "androiddebugkey"
key_password = "android"
EOF
    echo "note: appended signing.release to android/Cargo.toml (local only)" >&2
  fi
}

install_apk() {
  local apk="$1"
  ensure_adb
  echo "install -r $apk" >&2
  adb install -r "$apk" >&2
  echo "ok: installed $PKG" >&2
}

launch_app() {
  ensure_adb
  echo "launch $ACTIVITY" >&2
  adb shell am start -n "$ACTIVITY" >&2
}

if [[ "$LAUNCH_ONLY" -eq 1 ]]; then
  launch_app
  exit 0
fi

if [[ "$INSTALL_ONLY" -eq 1 ]]; then
  profile=debug
  [[ "$RELEASE" -eq 1 ]] && profile=release
  apk="$(find_apk "$profile")" || {
    echo "no APK under $APP/target — build first" >&2
    exit 1
  }
  install_apk "$apk"
  if [[ "$LAUNCH" -eq 1 ]]; then
    launch_app
  fi
  exit 0
fi

if [[ "$BUILD_ONLY" -eq 1 ]]; then
  apk="$(build_apk)"
  echo "$apk"
  exit 0
fi

# Default / install / run: build + install, optional launch.
apk="$(build_apk)"
install_apk "$apk"
if [[ "$LAUNCH" -eq 1 ]]; then
  launch_app
fi
echo "$apk"
