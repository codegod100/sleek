# Execution platform for sleek's cargo-backed genrules.
#
# `remote_enabled = False`, always: BuildBuddy's stock RBE workers don't have
# cargo, the pinned rustc, the Android NDK, or the `gleam` wasm compiler our
# build.rs falls back to — actions always run on this machine.
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
            remote_enabled = False,
            remote_cache_enabled = buildbuddy_enabled,
            remote_execution_use_case = "buck2-default",
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
