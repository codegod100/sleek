#!/usr/bin/env bash
# Capture README screenshots for the Vidya demo (needs a working display).
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --manifest-path host/Cargo.toml

OUT=docs/screenshots
mkdir -p "$OUT"
BIN=./host/target/debug/vidya-demo-host
[[ -x "$BIN" ]] || BIN=./target/debug/vidya-demo-host

# Optional: pull egui runtime libs from nixpkgs when not on NixOS fully linked
if command -v nix >/dev/null 2>&1; then
  LIBS=$(nix eval --raw --impure --expr \
    'with import <nixpkgs> {}; lib.makeLibraryPath [ libxkbcommon libGL wayland libx11 libxcursor libxi libxrandr vulkan-loader ]' \
    2>/dev/null || true)
  if [[ -n "${LIBS:-}" ]]; then
    export LD_LIBRARY_PATH="${LIBS}${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  fi
fi

specs=(
  "overview:dark:overview-dark.png"
  "actions:dark:actions-dark.png"
  "palette:dark:palette-dark.png"
  "forms:dark:forms-dark.png"
  "surfaces:dark:surfaces-dark.png"
)

for spec in "${specs[@]}"; do
  IFS=':' read -r section mode file <<<"$spec"
  path="$OUT/$file"
  echo "→ $section / $mode → $path"
  timeout 40 "$BIN" --section "$section" --mode "$mode" --screenshot "$(pwd)/$path"
  ls -la "$path"
done

echo "done."
