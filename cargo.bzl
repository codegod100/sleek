# Cargo-backed genrule wrapper.
#
# sleek's dependency graph (eframe/egui with vendored patched forks, wasmtime,
# git-pinned iroh/noq/web-transport, a build.rs that shells out to `gleam`)
# is too large and too non-crates.io to hand-translate into native buck2
# rust_library/rust_binary targets. Instead, each target here just runs the
# real `cargo build` — reusing its own on-disk incremental target/ cache —
# and buck2 tracks only the final artifact(s). See platforms/defs.bzl for how
# that still gets BuildBuddy remote-cache sharing despite running locally.
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
    return native.glob(
        [
            "host/**",
            "android/**",
            "third-party/cpal/**",
            "third-party/egui-winit/**",
            "third-party/egui_glow/**",
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
    )

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
    native.genrule(
        name = name,
        srcs = _srcs(),
        out = out,
        outs = outs,
        default_outs = default_outs,
        cmd = 'set -euo pipefail\n{} {} 2>&1 | tee "{}"\n{}'.format(_ENTER, build_cmd, log_path, collect_cmd),
        # Real repo layout (not a symlinked sandbox): cargo/rustc need the
        # actual `.git`, `target/` incremental cache, and CARGO_HOME — and
        # `pixi run` needs to find pixi.toml from the real root.
        repo_relative_root = True,
        # cargo build.rs steps hit the network (crates.io, git deps); this
        # also opts the action into cacheable-when-local-only handling, so
        # results still get uploaded to the BuildBuddy remote cache.
        labels = ["network_access"],
        always_print_stderr = True,
        executable = executable,
        visibility = ["PUBLIC"],
    )
