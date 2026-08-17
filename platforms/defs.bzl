# Execution platform for sleek's cargo-backed genrules and the reindeer-
# vendored third-party/ rust_library graph.
#
# `remote_enabled = True`: verified working. Once the third-party/ graph's
# toolchain became buck2-fetched (toolchains/rust_dist.bzl, toolchains/
# cxx_zig_toolchain.bzl) rather than tied to this machine's pixi env, and
# khronos_api's non-portable build.rs got patched (third-party/khronos_api/,
# see its build.rs comment), a clean run got 691/691 third-party:eframe
# actions executing on BuildBuddy's remote workers with zero local
# fallback and zero cache assist.
#
# `container-image` extends BuildBuddy's stock `rbe-ubuntu22-04` (buck2's
# own upstream example image; the older `rbe-ubuntu20-04` image failed —
# its Python can't parse prelude//rust/tools/rustc_action.py's
# `tuple[str, str]` PEP 585 type hints) with pixi + Android NDK r29 baked
# in (see toolchains/rbe-image/) — needed because host/'s own
# cargo_genrule targets (cargo.bzl: sleek-host, sleek-android-lib) aren't
# buckified yet and still shell out to `pixi run -- cargo build` + the NDK
# directly, none of which the plain stock image has. Those genrules used
# to carry `labels = ["network_access"]`, which prelude//
# genrule_local_labels.bzl's require-local allowlist includes and forces
# local regardless of this flag — removed once the custom image made them
# remote-capable (see cargo.bzl); `"network": "bridge"` below is what
# actually lets those build.rs steps reach crates.io/git during a remote
# run.
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
                # Extends BuildBuddy's own stock rbe-ubuntu22-04 with pixi +
                # Android NDK r29 baked in (see toolchains/rbe-image/), so
                # cargo.bzl's cargo_genrule targets (sleek-host,
                # sleek-android-lib — still `pixi run -- cargo build`, not
                # buckified) have what they need without depending on this
                # machine's local pixi/NDK install.
                # Pinned by digest, not `:latest` — a mutable-tag reference
                # let BuildBuddy's RE workers keep pulling/using a
                # previously-cached image even after a fresh `:latest` push
                # (confirmed live: rebuilding the image with build-essential
                # added, re-pushing, and re-running the exact same remote
                # build still hit the pre-build-essential failure — the
                # local image genuinely had libc6-dev/stdint.h, so the
                # remote workers were provably not seeing the new content).
                # Get the current digest with:
                #   skopeo inspect docker://ghcr.io/codegod100/sleek-rbe:latest
                # and update this after every rebuild (see
                # toolchains/rbe-image/README.md).
                "container-image": "docker://ghcr.io/codegod100/sleek-rbe@sha256:5814a2fecabaa59826ee7690e80c059b0a9e6a4064086d9999a9b411d5307bcc",
                # cargo_genrule's build.rs steps hit the network (crates.io,
                # git deps) — off by default on BuildBuddy's containers.
                "dockerNetwork": "bridge",
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
