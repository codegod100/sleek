#!/usr/bin/env bash
# pixi activation hook — the pixi-native equivalent of devenv.nix's
# enterShell. Sourced (not exec'd) by `pixi shell` / `pixi run`, so it must
# only export vars, never `set -e` or otherwise take over the shell.

# bindgen (v4l2r, etc.) needs the clang resource-dir headers alongside
# libclang. The version subdir tracks the installed clang package, so
# discover it instead of hardcoding (mirrors pkgs.llvmPackages.libclang.version
# in devenv.nix).
if [[ -n "${CONDA_PREFIX:-}" ]]; then
  clang_res_dir=$(find "${CONDA_PREFIX}/lib/clang" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | sort -V | tail -1)
  if [[ -n "${clang_res_dir}" ]]; then
    export BINDGEN_EXTRA_CLANG_ARGS="-I${clang_res_dir}/include"
  fi
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
