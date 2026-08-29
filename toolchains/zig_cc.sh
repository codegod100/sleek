#!/usr/bin/env bash
# TEMPORARY DIAGNOSTIC — remove once we know whether this script's own
# stdout/stderr actually reach cc-rs's captured Command output for a
# cargo-buildscript C/C++ compile, or whether something upstream of here
# (buck2 prelude's auto-generated __cxx_shim.sh / from_any_dir.py, see the
# nested-script comment in cxx_zig_toolchain.bzl) swallows it before cc-rs
# ever sees it. Every openh264-sys2 failure so far has come back with
# empty stdout/stderr despite this script's own unconditional
# `echo ... "retrying verbosely"` on the failure path below, which should
# be impossible to lose without some kind of redirection/truncation
# upstream of this script. Fail fast and loud on the very first
# openh264-sys2 file compiled so we don't have to wait for a real
# (nondeterministic) failure to test this.
if [[ "$*" == *upstream/codec/* ]]; then
    echo "CANARY_STDERR: zig_cc.sh reached for openh264-sys2, pid=$$" >&2
    echo "CANARY_STDOUT: zig_cc.sh reached for openh264-sys2, pid=$$"
    exit 1
fi
# END TEMPORARY DIAGNOSTIC

# Zig rejects nested response files. Expand Buck's response files before
# handing the argument vector to Zig. Cargo build scripts normally pass no
# response files, but using the same wrapper keeps one hermetic compiler.
zig=$1
subcommand=$2
shift 2

# Cargo build scripts invoke the compiler outside Buck's normal action wrapper.
# Give Zig an explicitly writable cache instead of relying on HOME in the
# minimal remote-execution environment.
export ZIG_GLOBAL_CACHE_DIR="${TMPDIR:-/tmp}/sleek-zig-global-cache"
export ZIG_LOCAL_CACHE_DIR="${TMPDIR:-/tmp}/sleek-zig-local-cache"
mkdir -p "$ZIG_GLOBAL_CACHE_DIR" "$ZIG_LOCAL_CACHE_DIR"

# A build.rs (ring's, openh264-sys2's, among others) compiles many
# translation units by spawning several `cc`/`c++`/`zig cc` child processes
# in parallel, all sharing this one $TMPDIR-scoped global cache dir —
# freshly created above, never pre-warmed, since a remote-execution action
# gets a clean filesystem every run. Zig's first-use population of a cold
# global cache (bundled libc headers/CRT objects for the target) isn't safe
# against concurrent writers targeting the same cache: a known upstream
# race (ziglang/zig#14815, #18763, #20129) that surfaces as a spurious
# failure with no useful stderr — exactly what cc-rs reports as a bare
# "did not execute successfully" with empty stdout/stderr, and which file
# loses the race varies run to run.
#
# Originally serialized with flock, but that alone wasn't sufficient on
# BuildBuddy's RE workers (still raced, on a different file each retry) —
# those sandboxes commonly run under gVisor, whose advisory-lock (flock/
# fcntl) emulation has known gaps on some overlay/network filesystem
# backends, so the lock can silently fail to exclude. `mkdir` is atomic at
# the filesystem-namespace level and doesn't depend on advisory-lock
# support at all, so use a spin-wait mkdir-based lock instead. The cache is
# warm after the first compile, so this only costs concurrency on a cold
# cache, not correctness.
zig_cache_lock_dir="${ZIG_GLOBAL_CACHE_DIR}.lockdir"

acquire_zig_cache_lock() {
    local waited=0
    until mkdir "$zig_cache_lock_dir" 2>/dev/null; do
        sleep 0.2
        waited=$((waited + 1))
        if (( waited >= 1500 )); then # ~5 minutes
            echo "zig_cc.sh: timed out waiting for $zig_cache_lock_dir; proceeding without the lock" >&2
            break
        fi
    done
}

release_zig_cache_lock() {
    rmdir "$zig_cache_lock_dir" 2>/dev/null || true
}
trap release_zig_cache_lock EXIT

args=("$@")
while :; do
    expanded=()
    changed=0
    for arg in "${args[@]}"; do
        if [[ $arg == @* ]]; then
            # Buck response files contain shell-quoted arguments.
            eval "words=($(<"${arg:1}"))"
            expanded+=("${words[@]}")
            changed=1
        else
            expanded+=("$arg")
        fi
    done
    args=("${expanded[@]}")
    (( changed )) || break
done

# Remote workers and developer machines can expose different CPU feature sets.
# Never let Zig select its host CPU: artifacts produced remotely must run on the
# x86-64 baseline used by the Buck platform. Drop cc-rs/Buck target overrides;
# leaving the OS target native preserves system-library discovery for GTK while
# `-mcpu=baseline` keeps generated code portable.
filtered=()
skip_next=0
for arg in "${args[@]}"; do
    if (( skip_next )); then
        skip_next=0
        continue
    fi
    case "$arg" in
        -target|--target)
            skip_next=1
            ;;
        -target=*|--target=*)
            ;;
        *)
            filtered+=("$arg")
            ;;
    esac
done
args=("-mcpu=baseline" "${filtered[@]}")

acquire_zig_cache_lock
"$zig" "$subcommand" "${args[@]}"
status=$?
release_zig_cache_lock
if [[ $status -eq 0 ]]; then
    exit 0
fi
echo "zig $subcommand failed; retrying verbosely" >&2
acquire_zig_cache_lock
"$zig" "$subcommand" -v "${args[@]}"
release_zig_cache_lock
exit "$status"
