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

# Desktop window (egui needs SLEEK_LD_LIBRARY_PATH from nix develop — not ambient)
host *args:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ -n "${SLEEK_LD_LIBRARY_PATH:-}" ]]; then
      export LD_LIBRARY_PATH="${SLEEK_LD_LIBRARY_PATH}${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    fi
    cargo run --manifest-path host/Cargo.toml {{args}}

# Check / build the Android package as a library (desktop target)
lib:
    cargo build --manifest-path android/Cargo.toml --lib

# APK in android/ → Waydroid (x86_64)
waydroid:
    ./scripts/waydroid.sh run

run: waydroid

install:
    ./scripts/waydroid.sh install

launch:
    ./scripts/waydroid.sh launch

# Phone APK (aarch64) via flake — auto-pushes to Cachix when auth is present
# Opt out: SLEEK_CACHIX_PUSH=0 just android
android:
    ./scripts/nix-build-push.sh .#android -L --out-link result-android

# adb install result of .#android onto a connected phone
install-android *args:
    nix run .#install-android -- {{args}}

# Push store paths to codegod100.cachix.org (needs CACHIX_AUTH_TOKEN)
# Usage: just push            # push ./result
#        just push ./result   # explicit path
#        just push-android    # same as just android (build + auto-push)
cachix_cache := env_var_or_default("CACHIX_CACHE", "codegod100")

push *paths:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v cachix >/dev/null; then
      echo "cachix not on PATH — run: just bootstrap  (or nix profile install nixpkgs#cachix)" >&2
      exit 1
    fi
    if [[ -z "${CACHIX_AUTH_TOKEN:-}" ]] && [[ ! -f "${HOME}/.config/cachix/cachix.dhall" ]]; then
      echo "No Cachix auth. Set Codespace secret CACHIX_AUTH_TOKEN or: cachix authtoken" >&2
      exit 1
    fi
    targets=( {{paths}} )
    if [[ ${#targets[@]} -eq 0 || -z "${targets[0]:-}" ]]; then
      targets=(./result)
    fi
    for t in "${targets[@]}"; do
      echo "cachix push {{cachix_cache}} $t" >&2
      cachix push {{cachix_cache}} "$t"
    done

# Alias: build android with watch-exec auto-push (same as just android)
push-android: android
