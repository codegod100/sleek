#!/usr/bin/env bash
# Rebuild prebuilt dex blobs from java/uk/nandi/sleek/*.java
#   - src/assets/pick_fragment.dex  (InMemoryDexClassLoader at runtime)
#   - src/assets/sleek_activity.dex (injected as APK classes.dex post cargo-apk)
#
# Requires ANDROID_HOME (or ANDROID_SDK_ROOT) with a platform + build-tools d8.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
if [[ -z "$SDK" ]]; then
  echo "Set ANDROID_HOME or ANDROID_SDK_ROOT" >&2
  exit 1
fi
ANDROID_JAR="$SDK/platforms/android-34/android.jar"
if [[ ! -f "$ANDROID_JAR" ]]; then
  ANDROID_JAR="$(echo "$SDK"/platforms/android-*/android.jar | awk '{print $1}')"
fi
D8="$(echo "$SDK"/build-tools/*/d8 | awk '{print $NF}')"
[[ -f "$ANDROID_JAR" ]] || { echo "android.jar not found under $SDK" >&2; exit 1; }
[[ -x "$D8" || -f "$D8" ]] || { echo "d8 not found under $SDK" >&2; exit 1; }

WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT
mkdir -p "$ROOT/src/assets"

# ── PickFragment (loaded in-process via InMemoryDexClassLoader) ────────────
javac --release 11 -Xlint:-options -classpath "$ANDROID_JAR" -d "$WORKDIR/pick" \
  "$ROOT/java/uk/nandi/sleek/PickFragment.java"
"$D8" --min-api 28 --output "$WORKDIR/pick" "$WORKDIR/pick"/uk/nandi/sleek/*.class
cp "$WORKDIR/pick/classes.dex" "$ROOT/src/assets/pick_fragment.dex"
echo "Wrote $ROOT/src/assets/pick_fragment.dex ($(wc -c < "$ROOT/src/assets/pick_fragment.dex") bytes)"

# ── SleekActivity (must be APK-root classes.dex for system classloader) ────
javac --release 11 -Xlint:-options -classpath "$ANDROID_JAR" -d "$WORKDIR/act" \
  "$ROOT/java/uk/nandi/sleek/SleekActivity.java"
"$D8" --min-api 28 --output "$WORKDIR/act" "$WORKDIR/act"/uk/nandi/sleek/*.class
cp "$WORKDIR/act/classes.dex" "$ROOT/src/assets/sleek_activity.dex"
echo "Wrote $ROOT/src/assets/sleek_activity.dex ($(wc -c < "$ROOT/src/assets/sleek_activity.dex") bytes)"
