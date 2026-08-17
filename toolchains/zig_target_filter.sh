#!/usr/bin/env bash
# Wrapper between cc-rs (build.rs C compilation, invoked via
# prelude//rust:cargo_buildscript.bzl's $CC/$CXX shims) and `zig cc`/`zig c++`.
#
# zig's own `-target`/`--target=` flag parses a "target query" mini-language
# (`<arch>-<os>-<abi>[.<glibc-version>]`, 3 dash-separated components), not
# a standard 4-component LLVM triple. cc-rs detects zig as a clang (it
# self-reports "clang version ..." for --version) and unconditionally
# appends `--target=<TARGET env var>` — a real LLVM triple like
# `x86_64-unknown-linux-gnu` — as one of its own trailing args. Zig rejects
# that outright ("unable to parse target query ...: UnknownOperatingSystem",
# because it reads the LLVM triple's "unknown" vendor slot as an OS name).
# Since the two `-target` flags conflict and the last one on the command
# line wins, cc-rs's always loses out to ours anyway when it comes first —
# so just strip whatever cc-rs appended and keep the target this script was
# built for. See toolchains/cxx_zig_toolchain.bzl for how this is wired in.
#
# Checked into the repo as a plain source file (not buck2-generated) on
# purpose: prelude//rust:cargo_buildscript.bzl's `_make_cc_shim` rewrites
# artifact paths it can see directly into a `${..}/`-prefixed marker so the
# script still resolves when invoked from build.rs's arbitrary cwd — that
# only works for paths visible to it, so both this script's own location
# and the zig binary path passed to it in $1 need to arrive as ordinary
# buck2 artifact references (cmd_args), not be pre-baked into some other
# generated wrapper's opaque body.
set -euo pipefail

zig="$1"
subcommand="$2"
target="$3"
shift 3

args=()
skip_next=0
for arg in "$@"; do
  if [[ "${skip_next}" == 1 ]]; then
    skip_next=0
    continue
  fi
  case "${arg}" in
    --target=*) continue ;;
    -target) skip_next=1 ;;
    *) args+=("${arg}") ;;
  esac
done

exec "${zig}" "${subcommand}" -target "${target}" "${args[@]}"
