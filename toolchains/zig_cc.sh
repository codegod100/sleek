#!/usr/bin/env bash

stage=initializing
report_failure() {
    status=$?
    if (( status != 0 )); then
        {
            echo "zig compiler wrapper failed: stage=${stage} status=${status}"
            echo "  subcommand=${subcommand:-unset}"
            echo "  cwd=$(pwd)"
            echo "  cache=${zig_cache_root:-unset}"
            echo "  zig=${zig:-unset}"
        } >&2
    fi
}
trap report_failure EXIT

# Zig rejects nested response files. Expand Buck's response files before
# handing the argument vector to Zig. Cargo build scripts normally pass no
# response files, but using the same wrapper keeps one hermetic compiler.
zig=$1
subcommand=$2
shift 2

# Cargo build scripts invoke the compiler outside Buck's normal action wrapper.
# Give Zig an explicitly writable cache instead of relying on HOME in the
# minimal remote-execution environment. Share Zig's immutable global cache,
# but isolate the per-compilation local cache: build scripts can launch many
# compiler children concurrently, and each child owns different intermediate
# state. The shell PID is unique within one buildscript action.
zig_cache_root="${OUT_DIR:-${TMPDIR:-/tmp}/sleek-zig-cache}/zig-cache"
export ZIG_GLOBAL_CACHE_DIR="$zig_cache_root/global"
export ZIG_LOCAL_CACHE_DIR="$zig_cache_root/local/$$"
stage=creating-cache
mkdir -p "$ZIG_GLOBAL_CACHE_DIR" "$ZIG_LOCAL_CACHE_DIR" || exit $?

stage=expanding-response-files
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

stage=compiling
"$zig" "$subcommand" "${args[@]}"
status=$?
if [[ $status -eq 0 ]]; then
    exit 0
fi
echo "zig $subcommand failed with status $status; retrying verbosely" >&2
stage=verbose-retry
"$zig" "$subcommand" -v "${args[@]}"
retry_status=$?
exit "$retry_status"
