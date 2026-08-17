# Execution platform for sleek's cargo-backed genrules and the reindeer-
# vendored third-party/ rust_library graph.
#
# `remote_enabled = False` for now, but verified working and left wired up
# below (remote_execution_properties etc.) — flip to True to re-enable.
# Once the third-party/ toolchain became buck2-fetched (toolchains/
# rust_dist.bzl, toolchains/cxx_zig_toolchain.bzl) rather than tied to this
# machine's pixi env, RE actually worked: flipping this to True and adding
# the config below got 353/355 third-party:eframe actions running on
# BuildBuddy's stock `rbe-ubuntu22-04` image with zero sleek-specific setup
# (`rbe-ubuntu20-04`, buck2's own upstream example image, failed instead —
# its older Python can't parse prelude//rust/tools/rustc_action.py's
# `tuple[str, str]` PEP 585 type hints). host/'s own cargo_genrule targets
# (cargo.bzl) still need pixi/cargo/the Android NDK/gleam, none of which are
# on that image — but those genrules already carry
# `labels = ["network_access"]`, which prelude//genrule_local_labels.bzl's
# require-local allowlist includes, so buck2 forces them local automatically
# regardless of this flag. No separate carve-out needed.
#
# The 2 remaining failures (of 355) were both `khronos_api-3`'s build
# script: it bakes the *absolute* path to its OUT_DIR into generated Rust
# (`include_bytes!("/buildbuddy-execroot/buck-out/.../extension.xml")`),
# which stops resolving once a different action/container reads that
# generated file — a real incompatibility in that one crate's build.rs with
# RE's ephemeral-per-action-container model, not a config problem. Needs a
# targeted fixup (force that one buildscript_run local, or patch it to emit
# relative paths) before flipping this back on for real.
#
# `remote_cache_enabled` follows `[buildbuddy] enabled` (default true —
# see .buckconfig; needs BUILDBUDDY_API_KEY in the environment, or opt out
# in a git-ignored `.buckconfig.local`). When on, successful local runs also
# get uploaded to BuildBuddy's CAS/Action Cache, so a second machine/CI run
# with identical inputs gets a cache hit instead of re-running `cargo build`.
# When off, buck2 never talks to `[buck2_re_client]` at all — important
# because an unreachable/misconfigured RE client makes buck2 hard-fail the
# action rather than silently skipping the cache check (verified: flipping
# this to False is the only thing that avoids that failure without a key).
def _platforms(ctx):
    # Empty constraints (the original version of this rule) leave
    # `ctx.attrs._exec_os_type` unresolvable for any rule that reads it —
    # concretely, prelude//rust:cargo_buildscript.bzl's `targets.exec_triple`
    # comes back None and never sets the build script's HOST env var, and
    # any build.rs using cc-rs (wasmtime's cranelift, etc.) panics with
    # "environment variable HOST not defined". Mirror
    # prelude//platforms:defs.bzl's own `execution_platform` rule instead:
    # merge in the host cpu/os constraint values so exec-platform lookups
    # like this one resolve to real linux/x86_64 answers.
    constraints = dict()
    constraints.update(ctx.attrs.cpu_configuration[ConfigurationInfo].constraints)
    constraints.update(ctx.attrs.os_configuration[ConfigurationInfo].constraints)
    configuration = ConfigurationInfo(constraints = constraints, values = {})

    buildbuddy_enabled = read_config("buildbuddy", "enabled", "true").lower() == "true"

    platform = ExecutionPlatformInfo(
        label = ctx.label.raw_target(),
        configuration = configuration,
        executor_config = CommandExecutorConfig(
            local_enabled = True,
            remote_enabled = True,
            # Prefer a local run already in flight over racing a remote one —
            # matches buck2's own BuildBuddy example (examples/remote_execution/
            # buildbuddy/platforms/defs.bzl upstream).
            use_limited_hybrid = True,
            # BuildBuddy SaaS's own default executor image — plain Ubuntu
            # 22.04, nothing sleek-specific baked in. Sufficient because the
            # third-party/ graph's toolchain (toolchains/rust_dist.bzl,
            # toolchains/cxx_zig_toolchain.bzl) is buck2-fetched, not tied to
            # this machine — RE workers self-provision rustc/zig the same way
            # this machine does. host/'s own cargo_genrule targets still need
            # pixi/cargo/NDK, which this image doesn't have — but those
            # already carry `labels = ["network_access"]` (cargo.bzl), which
            # forces them local regardless of this executor config, so they
            # aren't scheduled onto this image at all.
            remote_execution_properties = {
                "OSFamily": "Linux",
                "container-image": "docker://gcr.io/flame-public/rbe-ubuntu22-04:latest",
            },
            remote_cache_enabled = buildbuddy_enabled,
            remote_execution_use_case = "buck2-default",
            remote_output_paths = "output_paths",
        ),
    )

    return [DefaultInfo(), ExecutionPlatformRegistrationInfo(platforms = [platform])]

platforms = rule(
    attrs = {
        "cpu_configuration": attrs.dep(providers = [ConfigurationInfo], default = "prelude//cpu:x86_64"),
        "os_configuration": attrs.dep(providers = [ConfigurationInfo], default = "prelude//os:linux"),
    },
    impl = _platforms,
)
