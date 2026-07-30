#!/usr/bin/env bash
# Rebuild src/assets/pick_fragment.dex from java/uk/nandi/sleek/PickFragment.java
# Requires ANDROID_HOME (or ANDROID_SDK_ROOT) with platforms/android-34 and build-tools.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
if [[ -z "$SDK" ]]; then
  echo "Set ANDROID_HOME or ANDROID_SDK_ROOT" >&2
  exit 1
fi
ANDROID_JAR="$SDK/platforms/android-34/android.jar"
if [[ ! -f "$ANDROID_JAR" ]]; then
  # fallback: any platform
  ANDROID_JAR="$(echo "$SDK"/platforms/android-*/android.jar | awk '{print $1}')"
fi
D8="$(echo "$SDK"/build-tools/*/d8 | awk '{print $NF}')"
[[ -f "$ANDROID_JAR" ]] || { echo "android.jar not found under $SDK" >&2; exit 1; }
[[ -x "$D8" || -f "$D8" ]] || { echo "d8 not found under $SDK" >&2; exit 1; }

WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT
javac --release 11 -Xlint:-options -classpath "$ANDROID_JAR" -d "$WORKDIR" \
  "$ROOT/java/uk/nandi/sleek/PickFragment.java"
"$D8" --min-api 28 --output "$WORKDIR" "$WORKDIR"/uk/nandi/sleek/*.class
mkdir -p "$ROOT/src/assets"
cp "$WORKDIR/classes.dex" "$ROOT/src/assets/pick_fragment.dex"
echo "Wrote $ROOT/src/assets/pick_fragment.dex ($(wc -c < "$ROOT/src/assets/pick_fragment.dex") bytes)"
