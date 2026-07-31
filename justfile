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

# Clone sibling freeq+vidya (path deps) and run desktop host on VNC :1
#   just codespace-host       # foreground
#   just codespace-host --bg  # background + /tmp/sleek-logs/host.log
codespace-host *args:
    bash scripts/codespace-host.sh {{args}}

# Desktop window (egui needs SLEEK_LD_LIBRARY_PATH from nix develop — not ambient)
# On Codespaces, desktop-lite exposes Fluxbox + noVNC on :1 (port 6080).
host *args:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ -n "${SLEEK_LD_LIBRARY_PATH:-}" ]]; then
      export LD_LIBRARY_PATH="${SLEEK_LD_LIBRARY_PATH}${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    fi
    # v4l2r bindgen must use nix libclang, never Android NDK (missing libz.so.1).
    if [[ -n "${SLEEK_LIBCLANG_PATH:-}" ]]; then
      export LIBCLANG_PATH="${SLEEK_LIBCLANG_PATH}"
    elif [[ -z "${LIBCLANG_PATH:-}" || "${LIBCLANG_PATH}" == *android-ndk* ]]; then
      echo "sleek: LIBCLANG_PATH missing or points at Android NDK." >&2
      echo "  Enter the flake shell first:  nix develop   or   just enter" >&2
      exit 1
    fi
    # Codespace / desktop-lite: GUI opens on the VNC X display.
    if [[ -n "${SLEEK_CODESPACE:-}" ]]; then
      export DISPLAY="${DISPLAY:-:1}"
      # No GPU in Codespaces — force llvmpipe via mesa from the flake libs.
      export LIBGL_ALWAYS_SOFTWARE="${LIBGL_ALWAYS_SOFTWARE:-1}"
    elif [[ -z "${DISPLAY:-}" && -z "${WAYLAND_DISPLAY:-}" ]]; then
      if [[ -S /tmp/.X11-unix/X1 ]]; then
        export DISPLAY=:1
      elif [[ -S /tmp/.X11-unix/X0 ]]; then
        export DISPLAY=:0
      fi
    fi
    cargo run --manifest-path host/Cargo.toml {{args}}

# Check / build the Android package as a library (desktop target)
lib:
    cargo build --manifest-path android/Cargo.toml --lib

# APK in android/ → Waydroid (x86_64) via flake (Rust + NDK + adb)
#   just waydroid              # debug
#   just waydroid build
#   just waydroid launch
#   just waydroid-release      # release (optimized + signed)
#   just waydroid-release build
waydroid *args:
    nix run .#waydroid -- {{args}}

waydroid-release *args:
    nix run .#waydroid-release -- {{args}}

run: waydroid

install:
    nix run .#waydroid -- install

launch:
    nix run .#waydroid -- launch

# Phone APK (aarch64) via flake — auto-pushes to Cachix when auth is present
# Opt out: SLEEK_CACHIX_PUSH=0 just android
android:
    ./scripts/nix-build-push.sh .#android -L --out-link result-android

# adb install result of .#android onto a connected phone
install-android *args:
    nix run .#install-android -- {{args}}

# In-tree cargo-apk (aarch64) + adb install (iterative phone deploy)
#   just deploy-android
#   just deploy-android -- --launch
#   just deploy-android -- --release --launch
deploy-android *args:
    nix run .#deploy-android -- {{args}}

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
