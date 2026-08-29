#!/usr/bin/env bash
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

# A build.rs (ring's, among others) compiles many translation units by
# spawning several `cc`/`zig cc` child processes in parallel, all sharing
# this one $TMPDIR-scoped global cache dir — freshly created above, never
# pre-warmed, since a remote-execution action gets a clean filesystem every
# run. Zig's first-use population of a cold global cache (bundled libc
# headers/CRT objects for the target) isn't safe against concurrent writers
# targeting the same cache: a known upstream race (ziglang/zig#14815,
# #18763, #20129) that surfaces as a spurious failure with no useful
# stderr — exactly what cc-rs reports as a bare "did not execute
# successfully" with empty stdout/stderr. Serialize actual zig invocations
# sharing this cache with flock; the cache is warm after the first
# compile, so this only costs concurrency, not correctness.
zig_cache_lock="${ZIG_GLOBAL_CACHE_DIR}.lock"

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

flock "$zig_cache_lock" "$zig" "$subcommand" "${args[@]}" && exit 0
status=$?
echo "zig $subcommand failed; retrying verbosely" >&2
flock "$zig_cache_lock" "$zig" "$subcommand" -v "${args[@]}"
exit "$status"
