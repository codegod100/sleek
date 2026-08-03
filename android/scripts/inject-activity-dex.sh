#!/usr/bin/env bash
# Inject uk.nandi.sleek.SleekActivity as APK-root classes.dex and re-sign.
#
# cargo-apk only ships NativeActivity + native .so; a custom Activity must be
# on the app classloader (classes.dex at the zip root). Call this after
# `cargo apk build` and before `adb install`.
#
# Usage:
#   inject-activity-dex.sh <apk-path> [dex-path]
# Env:
#   ANDROID_HOME / ANDROID_SDK_ROOT — for zipalign + apksigner
#   SLEEK_KEYSTORE / SLEEK_KEYSTORE_PASSWORD / SLEEK_KEY_ALIAS / SLEEK_KEY_PASSWORD
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APK="${1:-}"
DEX="${2:-$ROOT/src/assets/sleek_activity.dex}"
[[ -n "$APK" && -f "$APK" ]] || { echo "usage: $0 <apk> [dex]" >&2; exit 1; }
[[ -f "$DEX" ]] || {
  echo "missing $DEX — run android/scripts/rebuild-java-dex.sh first" >&2
  exit 1
}

SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
[[ -n "$SDK" ]] || { echo "Set ANDROID_HOME or ANDROID_SDK_ROOT" >&2; exit 1; }
ZIPALIGN="$(echo "$SDK"/build-tools/*/zipalign | awk '{print $NF}')"
APKSIGNER="$(echo "$SDK"/build-tools/*/apksigner | awk '{print $NF}')"
[[ -x "$ZIPALIGN" || -f "$ZIPALIGN" ]] || { echo "zipalign not found under $SDK" >&2; exit 1; }
[[ -x "$APKSIGNER" || -f "$APKSIGNER" ]] || { echo "apksigner not found under $SDK" >&2; exit 1; }
command -v python3 >/dev/null || { echo "python3 required to rewrite APK zip" >&2; exit 1; }

# Prefer explicit env, then committed CI keystore (stable across builds), then
# cargo-apk debug defaults, then a local release keystore.
KEYSTORE="${SLEEK_KEYSTORE:-}"
if [[ -z "$KEYSTORE" && -f "$ROOT/ci.keystore" ]]; then
  KEYSTORE="$ROOT/ci.keystore"
fi
if [[ -z "$KEYSTORE" ]]; then
  KEYSTORE="${CARGO_APK_RELEASE_KEYSTORE:-${CARGO_APK_DEV_KEYSTORE:-$HOME/.android/debug.keystore}}"
fi
if [[ ! -f "$KEYSTORE" ]]; then
  KEYSTORE="$HOME/.android/sleek-release.keystore"
fi
[[ -f "$KEYSTORE" ]] || {
  echo "No signing keystore at $KEYSTORE (or ~/.android/debug.keystore)" >&2
  exit 1
}
STOREPASS="${SLEEK_KEYSTORE_PASSWORD:-${CARGO_APK_RELEASE_KEYSTORE_PASSWORD:-android}}"
KEYPASS="${SLEEK_KEY_PASSWORD:-$STOREPASS}"
ALIAS="${SLEEK_KEY_ALIAS:-androiddebugkey}"

WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

# Rewrite the APK: drop signatures + old classes.dex, add our SleekActivity dex.
# Uses Python (Info-ZIP `zip` is not always on PATH / in pure nix builds).
python3 - "$APK" "$DEX" "$WORKDIR/unsigned.apk" <<'PY'
import sys
import zipfile
from pathlib import Path

src, dex, dst = map(Path, sys.argv[1:4])
with zipfile.ZipFile(src, "r") as zin, zipfile.ZipFile(
    dst, "w", compression=zipfile.ZIP_DEFLATED
) as zout:
    for info in zin.infolist():
        name = info.filename
        if name.startswith("META-INF/"):
            continue
        if name == "classes.dex":
            continue
        # Preserve compression method / per-file flags when possible.
        data = zin.read(name)
        out = zipfile.ZipInfo(filename=name, date_time=info.date_time)
        out.compress_type = info.compress_type
        out.external_attr = info.external_attr
        out.create_system = info.create_system
        zout.writestr(out, data)
    # classes.dex at zip root — system classloader for SleekActivity.
    dex_bytes = dex.read_bytes()
    zinfo = zipfile.ZipInfo(filename="classes.dex")
    zinfo.compress_type = zipfile.ZIP_DEFLATED
    zout.writestr(zinfo, dex_bytes)
print(f"rewrote APK with classes.dex ({len(dex_bytes)} bytes)", file=sys.stderr)
PY

"$ZIPALIGN" -f 4 "$WORKDIR/unsigned.apk" "$WORKDIR/aligned.apk"
"$APKSIGNER" sign \
  --v1-signing-enabled true \
  --v2-signing-enabled true \
  --ks "$KEYSTORE" \
  --ks-pass "pass:$STOREPASS" \
  --ks-key-alias "$ALIAS" \
  --key-pass "pass:$KEYPASS" \
  "$WORKDIR/aligned.apk"

# APK may be read-only (e.g. copied from the nix store).
chmod u+w "$APK" 2>/dev/null || true
cp "$WORKDIR/aligned.apk" "$APK"
echo "Injected SleekActivity classes.dex into $APK" >&2
