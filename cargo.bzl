# Cargo-backed genrule wrapper.
#
# sleek's dependency graph (eframe/egui with vendored patched forks, wasmtime,
# git-pinned iroh/noq/web-transport, a build.rs that shells out to `gleam`)
# is too large and too non-crates.io to hand-translate into native buck2
# rust_library/rust_binary targets. Instead, each target here just runs the
# real `cargo build` and buck2 tracks only the final artifact(s). Runs
# locally by default (reusing its own on-disk incremental target/ cache —
# fast), but is also remote-execution-capable now (platforms/defs.bzl's
# custom RE image bakes in pixi + the Android NDK) — remotely there's no
# incremental cache to reuse, so it's a full `cargo build` from scratch
# each time, slower but still correct.
#
# Toolchain comes from pixi (pixi.toml), not nix: `pixi run --` self-activates
# the env for just this one command, whether or not a `pixi shell` is already
# active — no ambient-env check needed, and no dependency on `/nix` existing
# at all. This also sidesteps a buck2 gotcha: the buck2 daemon captures its
# environment at spawn time, not per-command, so anything that instead relied
# on the *invoking shell* already being activated would silently break the
# moment a daemon is left running from before that activation.
_ENTER = "pixi run --"

# Source tree every cargo-backed target depends on: both workspaces
# (host/, android/), the vendored+patched crates (now under third-party/ —
# also reindeer's native-rules vendor dir, see reindeer.toml) and git-rev
# pins they `[patch]` onto, the cargo config that wires SSL/registry
# settings, and the pixi manifest/lockfile `pixi run` reads to build its env.
#
# native.glob (not the bare `glob`) — this runs inside a .bzl loaded by a
# BUCK file, not the BUCK file itself.
def _srcs():
    # native.glob() cannot cross a package boundary: third-party/ carries its
    # own reindeer-generated BUCK file (reindeer.toml), which makes it a
    # separate buck2 package from this one — a glob() pattern rooted here
    # (e.g. "third-party/egui-winit/**") silently matches zero files, no
    # error, even though the real files are right there on disk. This was
    # invisible locally (cargo_genrule's repo_relative_root = True bypasses
    # buck2's staged sandbox entirely and reads the real filesystem), but
    # under RBE only what's actually in `srcs` gets uploaded to the remote
    # worker, so those patched forks were silently absent there — a `cargo
    # build` remotely couldn't find them.
    #
    # cpal/egui-winit/egui_glow/khronos_api are genuine reindeer-buckified
    # crates (real reindeer-managed git checkouts and/or real dependency
    # edges inside third-party/BUCK's own graph — e.g. ":egui-winit-0.31"/
    # ":khronos_api-3" being real `srcs` deps of other targets there, and
    # cpal's own generated `cpal-0.18` target's srcs list is package-
    # relative, so even moving *just* cpal out broke third-party/BUCK's
    # ability to parse at all), so none of them can move out of that
    # package. Instead reindeer.toml's `buckfile_imports` (raw Starlark
    # spliced verbatim into every regenerated third-party/BUCK) defines a
    # small filegroup — `//third-party:patched-fork-srcs` — that glob()s
    # them from *inside* their own package (no boundary to cross there) and
    # exports the result as a target label, which *does* cross package
    # boundaries fine (unlike a bare path string — also verified: silently
    # dropped, same as glob).
    #
    # vidya/freeq have no such coupling — plain filesystem path deps, never
    # touched by reindeer/Cargo.lock at all — so they live in
    # patched-crates/, a plain non-reindeer-owned directory in *this*
    # package, where glob() just works directly.
    return native.glob(
        [
            "host/**",
            "android/**",
            "patched-crates/vidya/**",
            "patched-crates/freeq/**",
            "patches/**",
            ".cargo/**",
            "pixi.toml",
            "pixi.lock",
            "scripts/pixi-activate.sh",
        ],
        exclude = [
            "host/target/**",
            "android/target/**",
        ],
    ) + [
        "//third-party:patched-fork-srcs",
    ]

def cargo_genrule(name, manifest, cargo_args, collect_cmd, out = None, outs = None, default_outs = None, executable = False):
    """A genrule that shells out to `cargo build` (via `pixi run`) and copies its artifact(s) to $OUT.

    - manifest: path to the Cargo.toml to build (e.g. "host/Cargo.toml").
    - cargo_args: extra argv appended to `cargo build --manifest-path
      <manifest>` (e.g. "--release" or "--lib --release") — words, no shell
      metacharacters, passed straight through to `pixi run --` as `"$@"`.
    - collect_cmd: shell copying the resulting artifact(s) from cargo's real
      target/ dir into $OUT (single-file `out`) or $OUT/... (dir-shaped `outs`).

    Buck2 buffers a genrule's own stdout/stderr and only surfaces it once the
    whole action finishes — cargo's own "Compiling …" progress otherwise
    never shows up while a build is running. So cargo's output is teed to
    <manifest's dir>/cargo-build.log too (`tail -f host/cargo-build.log` —
    or `just host-log` / `just lib-log`), and `always_print_stderr` makes
    buck2 print the captured copy promptly rather than only on failure.
    """
    build_cmd = "cargo build --manifest-path {} {}".format(manifest, cargo_args)
    log_path = manifest[:-len("Cargo.toml")] + "cargo-build.log"

    # `//third-party:patched-fork-srcs` (see _srcs()) is a *target*, not a
    # raw glob-matched file — repo_relative_root's genrule staging only
    # remaps raw file srcs onto their real repo-relative path; a target
    # dependency instead lands at its own buck-out artifact path
    # (confirmed by hand: buck-out/.../third-party/__patched-fork-srcs__/
    # patched-fork-srcs/cpal/..., not third-party/cpal/...). So stage it
    # ourselves via $(location) before cargo ever runs. `-n` (no-clobber):
    # locally repo_relative_root already runs in the real repo root, where
    # third-party/{cpal,egui-winit,egui_glow,khronos_api} already exist for
    # real — this is a no-op there. Remotely third-party/ doesn't exist at
    # all yet (that's the whole bug this works around), so the copy is what
    # actually populates it.
    stage_patched_forks = 'mkdir -p third-party\ncp -rn "$(location //third-party:patched-fork-srcs)"/* third-party/\n'

    native.genrule(
        name = name,
        srcs = _srcs(),
        out = out,
        outs = outs,
        default_outs = default_outs,
        # `| tail -c 200000` on the *displayed* copy only — the full,
        # untruncated output still goes to log_path via tee. Without this,
        # a real cargo build's full "Compiling ..." output (hundreds of
        # crates) blows past buck2's own stdout-capture cap and gets
        # replaced with a bare "Result too large to display" — including
        # for the actual error, on failure. Confirmed hitting exactly that
        # trying to see why a remote sleek-android-lib build failed.
        cmd = 'set -euo pipefail\n{}{} {} 2>&1 | tee "{}" | tail -c 200000\n{}'.format(stage_patched_forks, _ENTER, build_cmd, log_path, collect_cmd),
        # Real repo layout (not a symlinked sandbox): cargo/rustc need the
        # actual `.git`, `target/` incremental cache, and CARGO_HOME — and
        # `pixi run` needs to find pixi.toml from the real root. Meaningful
        # for local execution only — a remote worker has no such thing
        # regardless of this flag; see platforms/defs.bzl for the RE image
        # (toolchains/rbe-image/) that gives it what it needs there instead
        # (network access + pixi + the Android NDK baked in).
        repo_relative_root = True,
        # NOT `labels = ["network_access"]` (used to be): that label is in
        # prelude//genrule_local_labels.bzl's require-local allowlist, which
        # forces buck2 to run the action locally *unconditionally*,
        # regardless of platforms/defs.bzl's remote_enabled — the whole
        # point of the custom RE image below is to let this run remotely,
        # so it can't carry that label. `"network": "bridge"` in
        # platforms/defs.bzl's remote_execution_properties is what actually
        # gives build.rs (crates.io, git deps) network access instead.
        always_print_stderr = True,
        executable = executable,
        visibility = ["PUBLIC"],
    )
