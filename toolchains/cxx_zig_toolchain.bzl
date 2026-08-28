# A copy of @prelude//toolchains/cxx/zig:defs.bzl's `cxx_zig_toolchain`,
# minus its `cmd_script` wrapping of the compiler/linker/archiver — see
# below for why that wrapping breaks cargo-buildscript C compilation.
#
# Upstream wraps each `zig <subcommand>` invocation in a small generated
# shell script (toolchains/cxx/zig/defs.bzl's `zig_cc`/`zig_cxx`/etc, via
# prelude//utils:cmd_script.bzl) whose body hardcodes a plain,
# project-root-relative path to the zig binary — valid only when the
# script itself is executed with cwd = project root, which holds for
# ordinary buck2-invoked compile actions.
#
# prelude//rust:cargo_buildscript.bzl's `_make_cc_shim` breaks that
# assumption on purpose: build.rs can invoke $CC from any directory, so it
# generates a `from_any_dir.py`-based shim that `os.chdir()`s to whatever
# cwd the build script chose *before* exec'ing the compiler. `_make_cc_shim`
# rewrites any artifact path it can see directly in the assembled `cmd`
# into a `${..}/`-prefixed marker that from_any_dir.py substitutes with a
# cwd-correct path — but it can only rewrite paths it sees; the zig
# binary's path baked inside the *separate*, already-materialized
# `zig_cc.sh` script is invisible to it. Net effect: build.rs C compilation
# (e.g. wasmtime's cranelift `helpers.c`) fails with "zig: No such file or
# directory" — a real repro, not a hypothetical:
#   $ cd .../build-script-run/output_artifacts/cwd && ./__cc_shim.sh ...
#   .../zig_cc.sh: line 2: buck-out/.../zig: No such file or directory
#
# Fix: put the zig artifact reference directly into CCompilerInfo.compiler
# etc (`cmd_args(zig, "cc")`, no intermediate script), so `_make_cc_shim`'s
# own `${..}` rewriting sees and fixes it up like any other path — no
# opaque nested script for it to fail to see through.
load(
    "@prelude//cxx:cxx_toolchain_types.bzl",
    "BinaryUtilitiesInfo",
    "CCompilerInfo",
    "CxxCompilerInfo",
    "CxxInternalTools",
    "LinkerInfo",
    "LinkerType",
    "ShlibInterfacesMode",
    "StripFlagsInfo",
    "cxx_toolchain_infos",
)
load("@prelude//cxx:headers.bzl", "HeaderMode")
load("@prelude//cxx:linker.bzl", "is_pdb_generated")
load("@prelude//linking:link_info.bzl", "LinkStyle")
load("@prelude//toolchains/cxx/zig:defs.bzl", "ZigDistributionInfo")

def _get_linker_type(os: str) -> LinkerType:
    if os == "linux":
        return LinkerType("gnu")
    fail("cxx_zig_toolchain (local copy): only linux is wired up here — see upstream defs.bzl for the other OSes this dropped.")

def _cxx_zig_toolchain_impl(ctx: AnalysisContext) -> list[Provider]:
    dist = ctx.attrs.distribution[ZigDistributionInfo]
    zig = ctx.attrs.distribution[RunInfo]

    # `-target`/`compiler_flags` deliberately does NOT carry `ctx.attrs.target`
    # here — see zig_target_filter.sh for why cc-rs's own appended
    # `--target=<llvm-triple>` has to be filtered out and re-supplied by
    # the filter script instead of laid down as a flag here that cc-rs's
    # later, conflicting one would just override.
    # Keep compilation hermetic. In particular, cargo build scripts execute in
    # a deliberately sparse environment where an unqualified `cc`/`c++` is not
    # guaranteed to exist on either developer machines or remote workers.
    zig_cc = cmd_args(ctx.attrs._zig_cc, zig, "cc")
    zig_cxx = cmd_args(ctx.attrs._zig_cc, zig, "c++")
    target_flags = cmd_args()
    zig_ar = cmd_args(zig, "ar")
    zig_ranlib = cmd_args(zig, "ranlib")
    return [ctx.attrs.distribution[DefaultInfo]] + cxx_toolchain_infos(
        internal_tools = ctx.attrs._cxx_internal_tools[CxxInternalTools],
        platform_name = dist.arch,
        c_compiler_info = CCompilerInfo(
            compiler = RunInfo(args = zig_cc),
            # cc-rs treats "clang" as a cross compiler and appends the Rust
            # target `x86_64-unknown-linux-gnu`, which Zig intentionally does
            # not accept (`x86_64-linux-gnu` is its spelling). The GCC command
            # shape is compatible and avoids that invalid synthetic flag.
            compiler_type = "gcc",
            # No `-target` here — zig_cc already carries it (see above);
            # duplicating it would just mean zig_target_filter.sh strips
            # its own re-add back out again as "yet another stray -target".
            compiler_flags = cmd_args(target_flags, ctx.attrs.c_compiler_flags),
            preprocessor_flags = cmd_args(ctx.attrs.c_preprocessor_flags),
        ),
        cxx_compiler_info = CxxCompilerInfo(
            compiler = RunInfo(args = zig_cxx),
            compiler_type = "gcc",
            compiler_flags = cmd_args(target_flags, ctx.attrs.cxx_compiler_flags),
            preprocessor_flags = cmd_args(ctx.attrs.cxx_preprocessor_flags),
        ),
        linker_info = LinkerInfo(
            archiver = RunInfo(args = zig_ar),
            archiver_type = "gnu",
            archiver_supports_argfiles = True,
            archive_objects_locally = False,
            binary_extension = "",
            generate_linker_maps = False,
            link_binaries_locally = False,
            link_libraries_locally = False,
            link_style = LinkStyle(ctx.attrs.link_style),
            link_weight = 1,
            linker = RunInfo(args = zig_cxx),
            linker_flags = cmd_args(ctx.attrs.linker_flags),
            object_file_extension = "o",
            shlib_interfaces = ShlibInterfacesMode("disabled"),
            shared_dep_runtime_ld_flags = ctx.attrs.shared_dep_runtime_ld_flags,
            shared_library_name_default_prefix = "lib",
            shared_library_name_format = "{}.so",
            shared_library_versioned_name_format = "{}.so.{}",
            static_dep_runtime_ld_flags = ctx.attrs.static_dep_runtime_ld_flags,
            static_library_extension = "a",
            static_pic_dep_runtime_ld_flags = ctx.attrs.static_pic_dep_runtime_ld_flags,
            independent_shlib_interface_linker_flags = ctx.attrs.shared_library_interface_flags,
            type = _get_linker_type(dist.os),
            use_archiver_flags = True,
            is_pdb_generated = is_pdb_generated(_get_linker_type(dist.os), ctx.attrs.linker_flags),
        ),
        binary_utilities_info = BinaryUtilitiesInfo(
            bolt_msdk = None,
            dwp = None,
            nm = RunInfo(args = ["nm"]),  # not included in the zig distribution.
            objcopy = RunInfo(args = ["objcopy"]),  # not included in the zig distribution.
            ranlib = RunInfo(args = zig_ranlib),
            strip = RunInfo(args = ["strip"]),  # not included in the zig distribution.
        ),
        header_mode = HeaderMode("symlink_tree_only"),
        strip_flags_info = StripFlagsInfo(
            strip_debug_flags = ctx.attrs.strip_debug_flags,
            strip_non_global_flags = ctx.attrs.strip_non_global_flags,
            strip_all_flags = ctx.attrs.strip_all_flags,
        ),
    )

cxx_zig_toolchain = rule(
    impl = _cxx_zig_toolchain_impl,
    attrs = {
        "c_compiler_flags": attrs.list(attrs.arg(), default = []),
        "c_preprocessor_flags": attrs.list(attrs.arg(), default = []),
        "cxx_compiler_flags": attrs.list(attrs.arg(), default = []),
        "cxx_preprocessor_flags": attrs.list(attrs.arg(), default = []),
        "distribution": attrs.exec_dep(providers = [RunInfo, ZigDistributionInfo]),
        "link_style": attrs.enum(LinkStyle.values(), default = "static"),
        "linker_flags": attrs.list(attrs.arg(), default = []),
        "shared_dep_runtime_ld_flags": attrs.list(attrs.arg(), default = []),
        "shared_library_interface_flags": attrs.list(attrs.string(), default = []),
        "static_dep_runtime_ld_flags": attrs.list(attrs.arg(), default = []),
        "static_pic_dep_runtime_ld_flags": attrs.list(attrs.arg(), default = []),
        "strip_all_flags": attrs.option(attrs.list(attrs.arg()), default = None),
        "strip_debug_flags": attrs.option(attrs.list(attrs.arg()), default = None),
        "strip_non_global_flags": attrs.option(attrs.list(attrs.arg()), default = None),
        "target": attrs.option(attrs.string(), default = None),
        "_cxx_internal_tools": attrs.default_only(attrs.dep(providers = [CxxInternalTools], default = "prelude//cxx/tools:internal_tools")),
        "_target_filter": attrs.default_only(attrs.source(default = "toolchains//:zig_target_filter.sh")),
        "_zig_cc": attrs.default_only(attrs.source(default = "toolchains//:zig_cc.sh")),
    },
    is_toolchain_rule = True,
)
