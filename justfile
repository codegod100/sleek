# Sleek — mobile freeq client (Vidya + freeq-sdk)
#   nix develop
#   just host
#   just waydroid

set shell := ["bash", "-euo", "pipefail", "-c"]

default:
    @just --list

# Desktop window
host *args:
    cargo run --manifest-path host/Cargo.toml {{args}}

# Check / build the Android package as a library (desktop target)
lib:
    cargo build --manifest-path android/Cargo.toml --lib

# APK in android/ → Waydroid
waydroid:
    ./scripts/waydroid.sh run

run: waydroid

install:
    ./scripts/waydroid.sh install

launch:
    ./scripts/waydroid.sh launch
