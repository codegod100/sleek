#!/usr/bin/env bash
set -euo pipefail

manifest_path=""
target_dir=""
out=""
rust_target=""
profile="dev"

while [ "$#" -gt 0 ]; do
    case "$1" in
        --manifest-path) manifest_path="$2"; shift 2 ;;
        --target-dir) target_dir="$2"; shift 2 ;;
        --out) out="$2"; shift 2 ;;
        --rust-target) rust_target="$2"; shift 2 ;;
        --profile) profile="$2"; shift 2 ;;
        *) echo "unknown argument: $1" >&2; exit 1 ;;
    esac
done

test -n "$manifest_path"
test -n "$target_dir"
test -n "$out"

cargo_args=(build --package sleek-host --lib --manifest-path "$manifest_path" --target-dir "$target_dir")
profile_dir=debug
if [ "$profile" = release ]; then
    cargo_args+=(--release)
    profile_dir=release
fi
if [ -n "$rust_target" ]; then
    cargo_args+=(--target "$rust_target")
    profile_dir="$rust_target/$profile_dir"
fi

if command -v cargo >/dev/null 2>&1; then
    cargo "${cargo_args[@]}"
else
    # Buck's RBE image exposes the Rust toolchain through the repository's
    # pinned Pixi environment rather than as an ambient cargo executable.
    pixi run -- cargo "${cargo_args[@]}"
fi
cp "$target_dir/$profile_dir/libsleek_relm4.so" "$out"
