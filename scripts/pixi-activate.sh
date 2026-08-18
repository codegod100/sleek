#!/usr/bin/env bash
# pixi activation hook — the pixi-native equivalent of devenv.nix's
# enterShell. Sourced (not exec'd) by `pixi shell` / `pixi run`, so it must
# only export vars, never `set -e` or otherwise take over the shell.

# bindgen (v4l2r, libspa-sys/cpal's pipewire feature, etc.) needs the clang
# resource-dir headers alongside libclang, *and* the real libc headers
# (stdint.h etc.) from conda's own bundled sysroot: it parses headers
# through libclang's low-level parseTranslationUnit API, not the `clang`/
# `cc` *driver* (the CLI binary) — and it's specifically the driver layer
# that does GCC-installation autodetection and injects the matching
# --sysroot/-isystem flags for you. libclang's API skips all of that, so
# without help bindgen can locate and start parsing a header fine (its own
# -I is enough) but then fails deeper in, unable to resolve basic libc
# types the header pulls in transitively (uint32_t, uintptr_t, …) —
# confirmed hitting exactly that running libspa-sys's build.rs under RBE.
# `clang -v -c` on the same header, same env, resolves it via the driver's
# autodetection to this same conda-bundled sysroot, so this is just
# closing that gap, not picking a different toolchain. The version subdir
# tracks the installed clang package, so discover it instead of
# hardcoding (mirrors pkgs.llvmPackages.libclang.version in devenv.nix).
#
# `-isystem <sysroot>/usr/include`, not `--sysroot=<sysroot>` (the literal
# example BINDGEN_EXTRA_CLANG_ARGS's own docs give for this): --sysroot is
# a *driver*-level flag that expands into several different internal
# search paths depending on target/multilib — exactly the translation
# libclang's raw parse API skips doing, so it never actually took effect
# passed that way (confirmed: a throwaway build.rs, isolating one
# variable at a time with a clean rebuild every time to rule out stale-
# build-script-cache false negatives, kept failing exactly as if unset).
# -isystem is a single, unambiguous "add this dir to the system include
# path" instruction with no driver-side indirection required — confirmed
# this one actually works.
#
# And NOT CPATH (tried that too, in between): CPATH is consumed by
# *every* C frontend process-wide, including the real GCC
# (x86_64-conda-linux-gnu-cc, see below) that actually compiles
# ring/aws-lc-sys/etc.'s C code for the *host* target, and — worse — also
# the NDK's own clang compiling for aarch64-linux-android when this same
# pixi env drives `cargo apk build` (see cargo.bzl's cargo_apk_genrule):
# GCC picked up *clang's* emmintrin.h ahead of its own matching one and
# choked on a clang-only builtin name it doesn't know
# (__builtin_ia32_psrldqi128_byteshift); the NDK's clang, cross-compiling,
# picked up *this x86_64 host sysroot's* glibc and failed even earlier
# looking for a 32-bit multilib stub header
# (gnu/stubs.h → gnu/stubs-32.h: file not found) that doesn't exist here
# and has nothing to do with an aarch64/bionic target anyway. Both
# confirmed live. BINDGEN_EXTRA_CLANG_ARGS is the properly *scoped*
# mechanism: it only ever reaches bindgen's own libclang parse calls,
# regardless of how many other different-target compiler invocations
# share this same activated env.
if [[ -n "${CONDA_PREFIX:-}" ]]; then
  clang_res_dir=$(find "${CONDA_PREFIX}/lib/clang" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | sort -V | tail -1)
  sysroot_include="${CONDA_PREFIX}/x86_64-conda-linux-gnu/sysroot/usr/include"
  args=""
  [[ -n "${clang_res_dir}" ]] && args="-I${clang_res_dir}/include"
  [[ -d "${sysroot_include}" ]] && args="${args}${args:+ }-isystem ${sysroot_include}"
  [[ -n "${args}" ]] && export BINDGEN_EXTRA_CLANG_ARGS="${args}"
  unset clang_res_dir sysroot_include args
fi

# v4l2r's build.rs resolves `#include <linux/videodev2.h>` to *this* conda
# toolchain's own bundled compat sysroot (x86_64-conda-linux-gnu/sysroot),
# not `/usr/include` — confirmed: an explicit `-I/usr/include` override via
# V4L2R_VIDEODEV2_H_PATH does not change which copy bindgen actually reads.
# conda's copy is also a much older kernel-uapi snapshot (pinned for glibc
# ABI compat), missing the newer V4L2 stateless-codec control structs
# (H264/VP8/VP9/AV1/FWHT/…) that v4l2r's src/controls/codec.rs unconditionally
# references — a hard build failure. Keep the conda sysroot's copies synced
# with the real ones instead of fighting bindgen's header resolution.
if [[ -n "${CONDA_PREFIX:-}" ]]; then
  v4l2_sysroot="${CONDA_PREFIX}/x86_64-conda-linux-gnu/sysroot/usr/include/linux"
  if [[ -d "${v4l2_sysroot}" ]]; then
    for h in videodev2.h v4l2-controls.h v4l2-common.h const.h; do
      if [[ -f "/usr/include/linux/${h}" ]] && ! cmp -s "/usr/include/linux/${h}" "${v4l2_sysroot}/${h}" 2>/dev/null; then
        cp "/usr/include/linux/${h}" "${v4l2_sysroot}/${h}" 2>/dev/null || true
      fi
    done
  fi
  unset v4l2_sysroot h
fi

# Plain `cc`/`gcc` on PATH resolve to *Bluefin's own system GCC*
# (/usr/bin/cc), not conda's `x86_64-conda-linux-gnu-cc` — conda-forge's
# cross-compiler packages deliberately don't shadow the system compiler.
# So any crate whose C code compiles via the `cc` Rust crate's default
# (aws-lc-sys, ring, blake3, openh264-sys2, libsqlite3-sys, alsa-sys,
# libspa-sys, …) picks up the system's current glibc headers/symbol
# versioning, while rustc's own link step explicitly uses conda's compiler
# and its much older bundled glibc (2.28) — a mismatch. Concretely: system
# glibc's <stdio.h>/<stdlib.h> alias sscanf/strtol to versioned
# __isoc23_sscanf/__isoc23_strtol symbols that conda's glibc doesn't have,
# so mold fails with "undefined symbol: __isoc23_sscanf" at final link.
# Point CC (generic + cc-rs's per-target var) at conda's own compiler so
# every C shim in the graph is compiled *and* linked against the same libc.
if [[ -n "${CONDA_PREFIX:-}" && -x "${CONDA_PREFIX}/bin/x86_64-conda-linux-gnu-cc" ]]; then
  export CC="${CONDA_PREFIX}/bin/x86_64-conda-linux-gnu-cc"
  export CC_x86_64_unknown_linux_gnu="${CC}"
  export AR="${CONDA_PREFIX}/bin/x86_64-conda-linux-gnu-ar"
  export AR_x86_64_unknown_linux_gnu="${AR}"
fi

# ring's C (crypto/curve25519/*, BoringSSL-derived) predates clang making
# -Wimplicit-function-declaration a hard *error* by default (was always
# just a warning historically, flipped around clang 15) — ring's own
# build.rs passes a fixed cc::Build flag list with no -Werror at all, so
# this isn't ring opting into stricter checking, it's conda's clang 22
# silently promoting a pre-existing (harmless in this code) warning to a
# build-breaking error. Confirmed hitting this compiling
# crypto/curve25519/curve25519_64_adx.c under RBE. cc-rs (the Rust `cc`
# crate every C shim above uses) appends this to its own invocation
# automatically — no per-crate opt-in needed.
export CFLAGS="${CFLAGS:-}${CFLAGS:+ }-Wno-error=implicit-function-declaration"

export SLEEK_BROWSER_PROFILE="${TMPDIR:-/tmp}/sleek-chromium"

# This VM advertises Wayland but has no usable Wayland client library.
# Force egui/winit onto the available X11 display instead (same override
# devenv.nix applies for this Bluefin dev box).
export DISPLAY="${DISPLAY:-:0}"
unset WAYLAND_DISPLAY
export WINIT_UNIX_BACKEND="x11"
export LIBGL_ALWAYS_SOFTWARE="${LIBGL_ALWAYS_SOFTWARE:-1}"
# [activation.env]'s SLEEK_LD_LIBRARY_PATH isn't exported yet at this point in
# pixi's hook (activation.env is applied after activation.scripts run), so
# derive straight from CONDA_PREFIX rather than reading it back here.
if [[ -n "${CONDA_PREFIX:-}" ]]; then
  export LD_LIBRARY_PATH="${CONDA_PREFIX}/lib${LD_LIBRARY_PATH:+:${LD_LIBRARY_PATH}}"
fi

echo "Sleek pixi env — run: just host  (or: pixi run host)"
