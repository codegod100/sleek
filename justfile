# Sleek — mobile freeq client (Vidya + freeq-sdk)
#   nix develop   |  ./scripts/enter
#   just host
#   just waydroid

set shell := ["bash", "-euo", "pipefail", "-c"]

default:
    @just --list

# Re-exec into flake shell (or run a command inside it)
enter *args:
    ./scripts/enter {{args}}

# Codespace / VM: install nix, direnv, bashrc hook, warm flake
bootstrap:
    bash scripts/codespace-bootstrap.sh

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
