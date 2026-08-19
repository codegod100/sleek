# Sleek — mobile freeq client (Vidya + freeq-sdk)
#   just host
#   just waydroid
#
# `host` / `lib` / `gleam-slash` build via buck2 where noted (see BUCK,
# cargo.bzl, platforms/) — a thin `cargo build` wrapper, cached, and
# self-entering the pixi env (pixi.toml) via `pixi run` — no nix, no manual
# shell-entry step first. Android/Waydroid/Flatpak packaging below is still
# nix-only (flake.nix) — pixi.toml doesn't cover cross-compilation/NDK/APK.

set shell := ["bash", "-euo", "pipefail", "-c"]

# Verbose by default: 2 = more info about errors, +stderr streams actions'
# stderr live (e.g. cargo's own build progress), +full_failed_command prints
# a copy-pasteable repro on failure. Override: BUCK_VERBOSITY=1 just host
buck_verbosity := env_var_or_default("BUCK_VERBOSITY", "2,stderr,full_failed_command")

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

# Desktop window: buck2 build (see BUCK/cargo.bzl, always --release) + run.
# Single entry point — no manual shell-entry step first; re-enters the pixi
# env itself when needed (`pixi run -- just host ...`, and re-dispatches
# through it once, so the env setup below always runs with
# SLEEK_LD_LIBRARY_PATH etc. present — see pixi.toml [activation.env]).
# `--release` is accepted for back-compat with existing callers (flake.nix
# `nix run .#host`, scripts/codespace-host.sh) — the buck2 target is
# release-only already, so it's dropped rather than forwarded to the binary.
# On Codespaces, desktop-lite exposes Fluxbox + noVNC on :1 (port 6080).
host *args:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ -z "${SLEEK_LD_LIBRARY_PATH:-}" ]]; then
      exec pixi run -- just host {{args}}
    fi
    export LD_LIBRARY_PATH="${SLEEK_LD_LIBRARY_PATH}${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    # v4l2r bindgen must use the pixi/nix libclang, never the Android NDK's
    # (missing libz.so.1).
    if [[ -n "${SLEEK_LIBCLANG_PATH:-}" ]]; then
      export LIBCLANG_PATH="${SLEEK_LIBCLANG_PATH}"
    elif [[ -z "${LIBCLANG_PATH:-}" || "${LIBCLANG_PATH}" == *android-ndk* ]]; then
      echo "sleek: LIBCLANG_PATH missing or points at Android NDK." >&2
      echo "  Enter the pixi env first:  pixi shell   or   pixi run -- just host" >&2
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
    app_args=()
    for a in {{args}}; do
      [[ "$a" == "--release" ]] || app_args+=("$a")
    done
    # cargo's own "Compiling …" progress otherwise never shows: buck2 only
    # surfaces a genrule's captured output once the whole action finishes.
    : > host/cargo-build.log
    tail -n +1 -f host/cargo-build.log &
    tail_pid=$!
    trap 'kill "$tail_pid" 2>/dev/null || true' EXIT
    buck2 run -v={{buck_verbosity}} //:sleek-host -- "${app_args[@]}"

# Gleam Wasm slash parser smoke (matches native Rust oracle; no GUI)
gleam-slash:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ -z "${SLEEK_LD_LIBRARY_PATH:-}" ]]; then
      exec pixi run -- just gleam-slash
    fi
    export LD_LIBRARY_PATH="${SLEEK_LD_LIBRARY_PATH}${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    if [[ -z "${GLEAM:-}" && -x "${HOME}/code/gleam/target/debug/gleam" ]]; then
      export GLEAM="${HOME}/code/gleam/target/debug/gleam"
    fi
    cargo run --manifest-path host/Cargo.toml -- --gleam-slash-only

# Check / build the Android package as a library (desktop target, release —
# via buck2, see BUCK/cargo.bzl). No manual shell-entry needed first — the
# genrule re-enters the pixi env itself (`pixi run --`).
lib *args:
    #!/usr/bin/env bash
    set -euo pipefail
    : > android/cargo-build.log
    tail -n +1 -f android/cargo-build.log &
    tail_pid=$!
    trap 'kill "$tail_pid" 2>/dev/null || true' EXIT
    buck2 build -v={{buck_verbosity}} //:sleek-android-lib {{args}}

# Tail cargo's own build output live from a *second* terminal, while
# `just host`/`just lib` (which already tail it themselves) run in the
# first — or while the raw `buck2 build`/`buck2 run` targets run directly.
host-log:
    tail -n +1 -f host/cargo-build.log

lib-log:
    tail -n +1 -f android/cargo-build.log

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

# Desktop Flatpak bundle (uk.nandi.sleek.flatpak) via nix2flatpak
# Opt out of Cachix: SLEEK_CACHIX_PUSH=0 just flatpak
flatpak:
    ./scripts/nix-build-push.sh .#flatpak -L --out-link result-flatpak


# AAB for Play Store (aarch64) — buck2 build via RBE/BuildBuddy.
# Requires bundletool in the RBE image (toolchains/rbe-image/Containerfile).
# Output: buck-out/.../sleek.aab (upload to Google Play Console)
#   buck2 build --show-output //:sleek-android-aab
aab:
    #!/usr/bin/env bash
    set -euo pipefail
    : > android/cargo-aab-build.log
    tail -n +1 -f android/cargo-aab-build.log &
    tail_pid=$!
    trap 'kill "$tail_pid" 2>/dev/null || true' EXIT
    buck2 build -v={{buck_verbosity}} //:sleek-android-aab

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
