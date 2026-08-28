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

"$zig" "$subcommand" "${args[@]}" && exit 0
status=$?
echo "zig $subcommand failed; retrying verbosely" >&2
"$zig" "$subcommand" -v "${args[@]}"
exit "$status"
