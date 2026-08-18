# Custom rust_toolchain pointing at a hermetic, buck2-fetched rustc (see
# rust_dist.bzl), not pixi and not the prelude's `system_rust_toolchain`
# (@prelude//toolchains:rust.bzl) — that one hardcodes
# `compiler = RunInfo(args = ["rustc"])`, a bare PATH lookup with no attr to
# override it, which would resolve to whatever the buck2 daemon's own
# ambient env happens to have rather than a pinned version.
#
# The final link step and any build.rs C compilation go through the *cxx*
# toolchain (toolchains/BUCK's `:cxx`, see rust.bzl's sibling there),
# which buck2's rust build rules pull in automatically — nothing here
# needs to know about CC/AR at all, unlike the old pixi-tool.sh wrapper.
load("@prelude//rust:rust_toolchain.bzl", "PanicRuntime", "RustToolchainInfo")

def _hermetic_rust_toolchain_impl(ctx):
    sysroot = ctx.attrs.sysroot[DefaultInfo].default_outputs[0]

    def tool(bin_name):
        return RunInfo(args = [cmd_args(sysroot, format = "{{}}/bin/{}".format(bin_name))])

    return [
        DefaultInfo(),
        RustToolchainInfo(
            compiler = tool("rustc"),
            rustdoc = tool("rustdoc"),
            clippy_driver = tool("clippy-driver"),
            default_edition = "2021",
            panic_runtime = PanicRuntime("unwind"),
            # Stable rustc (1.97.1, see rust_dist.bzl), not nightly — the
            # prelude default (True) assumes nightly and will pass
            # unsupported -Z flags.
            nightly_features = False,
            doctests = False,
            # justfile always builds --release; match cargo's own release
            # profile default (opt-level 3). Without an explicit
            # `-Copt-level=`, prelude//rust:cargo_buildscript.bzl never sets
            # OPT_LEVEL for build.rs invocations, and any build script using
            # cc-rs (wasmtime's cranelift, etc.) panics with "environment
            # variable OPT_LEVEL not defined" — this was already broken
            # under pixi_rust_toolchain, not a regression from this file.
            rustc_flags = ["-Copt-level=3"],
        ),
    ]

hermetic_rust_toolchain = rule(
    impl = _hermetic_rust_toolchain_impl,
    attrs = {
        "sysroot": attrs.dep(providers = [DefaultInfo]),
    },
    is_toolchain_rule = True,
)
