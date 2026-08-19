#!/usr/bin/env bash
# Convert a cargo-apk-produced APK to an Android App Bundle (.aab).
#
# cargo-apk has no native AAB support; this script bridges the gap by using
# aapt2's proto-format conversion (converting the APK's binary AXML resources
# to the proto binary XML that AABs require) and bundletool to assemble and
# sign the bundle. Output is suitable for direct upload to Google Play Console.
#
# Usage:
#   apk-to-aab.sh <input.apk> <activity.dex> <output.aab>
#
# Required env:
#   ANDROID_HOME or ANDROID_SDK_ROOT — SDK with build-tools/34.0.0 (for aapt2)
#   SLEEK_KEYSTORE                   — path to signing keystore
#   SLEEK_KEYSTORE_PASSWORD          — store password (default: android)
#   SLEEK_KEY_ALIAS                  — key alias   (default: androiddebugkey)
#   SLEEK_KEY_PASSWORD               — key password (default: store password)
# Optional env:
#   BUNDLETOOL_JAR — path to bundletool-all-*.jar; downloaded to
#                    $XDG_CACHE_HOME/bundletool/ if absent (requires curl)
#   JAVA           — java binary (default: java from PATH)

set -euo pipefail

APK="${1:-}"
DEX="${2:-}"
OUT="${3:-}"

[[ -n "$APK" && -f "$APK" ]] || { echo "apk-to-aab.sh: missing/invalid APK: ${APK:-<none>}" >&2; exit 1; }
[[ -n "$DEX" && -f "$DEX" ]] || { echo "apk-to-aab.sh: missing dex: ${DEX:-<none>}" >&2; exit 1; }
[[ -n "$OUT" ]] || { echo "apk-to-aab.sh: missing output path" >&2; exit 1; }

# ── Android SDK tools ─────────────────────────────────────────────────────
SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
[[ -n "$SDK" ]] || { echo "Set ANDROID_HOME or ANDROID_SDK_ROOT" >&2; exit 1; }

# Pick the highest build-tools version that ships aapt2.
AAPT2=""
for dir in "$SDK"/build-tools/*/aapt2; do
  [[ -x "$dir" ]] && AAPT2="$dir"
done
[[ -n "$AAPT2" ]] || AAPT2="$(command -v aapt2 2>/dev/null || true)"
[[ -n "$AAPT2" && -x "$AAPT2" ]] || {
  echo "aapt2 not found under $SDK/build-tools or PATH" >&2
  exit 1
}

# ── Java + jarsigner ──────────────────────────────────────────────────────
JAVA="${JAVA:-$(command -v java 2>/dev/null || true)}"
[[ -n "$JAVA" ]] || { echo "java not found — install JDK (openjdk 17+)" >&2; exit 1; }

# jarsigner sits next to java in the JDK bin dir.
JARSIGNER="$(dirname "$(realpath "$JAVA")")/jarsigner"
[[ -x "$JARSIGNER" ]] || JARSIGNER="$(command -v jarsigner 2>/dev/null || true)"
[[ -n "$JARSIGNER" && -x "$JARSIGNER" ]] || {
  echo "jarsigner not found alongside $JAVA or in PATH" >&2
  exit 1
}

# ── bundletool ────────────────────────────────────────────────────────────
# Priority: $BUNDLETOOL_JAR env var → `bundletool` on PATH → download.
# In the RBE image BUNDLETOOL_JAR=/opt/bundletool.jar is baked in
# (toolchains/rbe-image/Containerfile); local devs get a cached download.
if [[ -n "${BUNDLETOOL_JAR:-}" && -f "$BUNDLETOOL_JAR" ]]; then
  BUNDLETOOL_CMD=("$JAVA" -jar "$BUNDLETOOL_JAR")
elif command -v bundletool >/dev/null 2>&1; then
  BUNDLETOOL_CMD=(bundletool)
else
  BUNDLETOOL_VERSION="1.17.2"
  CACHE_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/bundletool"
  BUNDLETOOL_CACHE="$CACHE_DIR/bundletool-${BUNDLETOOL_VERSION}.jar"
  if [[ ! -f "$BUNDLETOOL_CACHE" ]]; then
    echo "==> Downloading bundletool ${BUNDLETOOL_VERSION}..." >&2
    mkdir -p "$CACHE_DIR"
    curl -fsSL \
      "https://github.com/google/bundletool/releases/download/${BUNDLETOOL_VERSION}/bundletool-all-${BUNDLETOOL_VERSION}.jar" \
      -o "$BUNDLETOOL_CACHE"
  fi
  BUNDLETOOL_CMD=("$JAVA" -jar "$BUNDLETOOL_CACHE")
fi

# ── Signing config ────────────────────────────────────────────────────────
KEYSTORE="${SLEEK_KEYSTORE:-}"
if [[ -z "$KEYSTORE" ]]; then
  SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
  [[ -f "$SCRIPT_DIR/../ci.keystore" ]] && KEYSTORE="$SCRIPT_DIR/../ci.keystore"
fi
[[ -n "$KEYSTORE" && -f "$KEYSTORE" ]] || {
  echo "Keystore not found — set SLEEK_KEYSTORE to a .jks / .keystore file" >&2
  echo "  The CI keystore lives at android/ci.keystore (relative to the repo root)" >&2
  exit 1
}
STOREPASS="${SLEEK_KEYSTORE_PASSWORD:-android}"
ALIAS="${SLEEK_KEY_ALIAS:-androiddebugkey}"
KEYPASS="${SLEEK_KEY_PASSWORD:-$STOREPASS}"

# ── Conversion ────────────────────────────────────────────────────────────
WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

# Step 1: Convert APK's AXML resources to proto format.
# aapt2 convert --output-format proto rewrites resources.arsc → resources.pb
# and AndroidManifest.xml AXML → proto binary XML (the format AABs require).
# Native .so files pass through unchanged.
echo "==> aapt2 convert --output-format proto..." >&2
"$AAPT2" convert --output-format proto "$APK" -o "$WORKDIR/proto.apk"

# Step 2: Assemble the base module directory tree.
#   manifest/AndroidManifest.xml  — proto binary XML (from proto APK)
#   resources.pb                  — compiled resource table (from proto APK)
#   lib/<abi>/libsleek.so         — native libs (from original APK)
#   root/classes.dex              — SleekActivity (injected dex)
mkdir -p "$WORKDIR/base/manifest" "$WORKDIR/base/root"

python3 - "$WORKDIR/proto.apk" "$WORKDIR/original.apk" "$WORKDIR/base" "$APK" <<'PY'
import sys, zipfile, os
from pathlib import Path

proto_apk, _orig, dst_dir, orig_apk = sys.argv[1], sys.argv[2], Path(sys.argv[3]), sys.argv[4]

# Extract manifest + resources.pb from the proto-format APK
with zipfile.ZipFile(proto_apk) as z:
    manifest_data = z.read("AndroidManifest.xml")
    (dst_dir / "manifest" / "AndroidManifest.xml").write_bytes(manifest_data)
    print(f"  manifest: {len(manifest_data)} bytes", file=sys.stderr)

    if "resources.pb" in z.namelist():
        res_data = z.read("resources.pb")
        (dst_dir / "resources.pb").write_bytes(res_data)
        print(f"  resources.pb: {len(res_data)} bytes", file=sys.stderr)

# Extract native libs from the ORIGINAL APK (.so files are not touched by aapt2 convert)
count = 0
with zipfile.ZipFile(orig_apk) as z:
    for name in z.namelist():
        if name.startswith("lib/") and name.endswith(".so"):
            dest = dst_dir / name
            dest.parent.mkdir(parents=True, exist_ok=True)
            dest.write_bytes(z.read(name))
            print(f"  {name}: {os.path.getsize(dest)} bytes", file=sys.stderr)
            count += 1

if count == 0:
    print("WARNING: no native libs found in APK — the bundle may be invalid", file=sys.stderr)
PY

# SleekActivity dex goes into root/ so the system classloader picks it up.
cp "$DEX" "$WORKDIR/base/root/classes.dex"
echo "  root/classes.dex: SleekActivity ($(wc -c < "$DEX") bytes)" >&2

# Step 3: Zip the base module (bundletool reads this as a module archive).
echo "==> Packaging base module zip..." >&2
(cd "$WORKDIR/base" && zip -qr "$WORKDIR/base.zip" .)

# Step 4: Build the unsigned AAB.
echo "==> bundletool build-bundle..." >&2
"${BUNDLETOOL_CMD[@]}" build-bundle \
  --modules="$WORKDIR/base.zip" \
  --output="$WORKDIR/unsigned.aab"

# Step 5: Sign with jarsigner (V1/JAR signature — accepted by Play Console for AABs).
# Play Store uses Google Play App Signing to re-sign the APKs it delivers to
# devices; the AAB upload just needs a valid upload-key signature here.
echo "==> Signing AAB with jarsigner..." >&2
cp "$WORKDIR/unsigned.aab" "$WORKDIR/signed.aab"
"$JARSIGNER" \
  -keystore "$KEYSTORE" \
  -storepass "$STOREPASS" \
  -keypass "$KEYPASS" \
  -sigalg SHA256withRSA \
  -digestalg SHA-256 \
  "$WORKDIR/signed.aab" \
  "$ALIAS"

cp "$WORKDIR/signed.aab" "$OUT"
echo "==> AAB: $OUT ($(wc -c < "$OUT") bytes)" >&2
