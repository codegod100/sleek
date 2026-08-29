#!/usr/bin/env bash

# Zig rejects nested response files. Expand Buck's response files before
# handing the argument vector to Zig. Cargo build scripts normally pass no
# response files, but using the same wrapper keeps one hermetic compiler.
zig=$1
subcommand=$2
shift 2

# Cargo build scripts invoke the compiler outside Buck's normal action wrapper.
# Give Zig an explicitly writable cache instead of relying on HOME in the
# minimal remote-execution environment. Scope it to the build script's OUT_DIR:
# /tmp is shared by otherwise-independent actions on a remote worker, and using
# one worker-global cache lets concurrent crates corrupt Zig's cold cache.
zig_cache_root="${OUT_DIR:-${TMPDIR:-/tmp}/sleek-zig-cache}"
export ZIG_GLOBAL_CACHE_DIR="$zig_cache_root/global"
export ZIG_LOCAL_CACHE_DIR="$zig_cache_root/local"
mkdir -p "$ZIG_GLOBAL_CACHE_DIR" "$ZIG_LOCAL_CACHE_DIR"

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

"$zig" "$subcommand" "${args[@]}"
status=$?
if [[ $status -eq 0 ]]; then
    exit 0
fi
echo "zig $subcommand failed; retrying verbosely" >&2
"$zig" "$subcommand" -v "${args[@]}"
retry_status=$?
exit "$retry_status"
