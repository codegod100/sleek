# Flatpak-backed genrule wrapper.
#
# Unlike cargo.bzl's genrules (which shell out to a real `cargo build`),
# this one doesn't compile anything itself — it packages the *already
# buck2-built* //:sleek-host binary into a distributable .flatpak bundle,
# reusing the org.gnome.Platform//49 runtime (baked into toolchains/
# rbe-image/Containerfile at image-build time, same pattern as the Android
# NDK/SDK) instead of vendoring every shared library sleek links against.
#
# Ports flake.nix's old `nix2flatpak.lib.${system}.mkFlatpak` derivation
# (see git history) to buck2 — nix2flatpak itself has no buck2 equivalent
# (it works off a Nix closure), but the actual flatpak (appId/runtime/
# permissions/appdata/desktopFile) is identical. Confirmed locally: the
# resulting bundle is ~17MB, not the old Nix build's 617MB — reusing the
# shared runtime instead of bundling a whole Nix closure is a real size
# win, not just a packaging-mechanism swap.
#
# Deliberately does NOT use flatpak-builder to drive this. flatpak-builder
# always runs each module's build-commands via `flatpak build <dir> <cmd>`,
# which sandboxes that execution through bwrap — and bwrap needs to create
# a Linux user namespace (CLONE_NEWUSER) to do that. BuildBuddy's RE worker
# containers refuse this (`bwrap: Creating new namespace failed: Operation
# not permitted`) — expected for a shared multi-tenant executor, since
# unprivileged user-namespace creation is itself a common container-escape
# surface, so it's normal for a hardened container runtime to block it
# regardless of in-container capabilities. No flatpak-builder flag disables
# the sandboxing (`flatpak-builder --help` only has `--sandbox`, which
# makes an already-sandboxed run *stricter*, not optional) — confirmed a
# rebuilt RE image with flatpak-builder installed still hits this.
#
# Since this module's own build-commands are just `install -Dm...` file
# copies (no actual compilation happens inside the flatpak sandbox), the
# fix is to skip flatpak-builder's sandboxed execution path entirely and
# drive the same lower-level `flatpak build-*` commands it would otherwise
# wrap around bwrap for us — `build-init`/`build-finish`/`build-export`/
# `build-bundle` are pure OSTree/metadata operations and never invoke
# bwrap, confirmed by running this exact sequence locally with strace
# absent from the picture (no namespace-clone error, `bwrap` never
# appears in the process tree). Mirrors what flatpak-builder does under
# the hood for a "simple" buildsystem module, just without the sandboxed
# `install` step:
#   1. `flatpak build-init` — sets up build-dir/{files,var,metadata}.
#   2. plain `install -Dm...` calls straight into build-dir/files/... (the
#      exact commands from flatpak/uk.nandi.sleek.json's build-commands,
#      translated from their `/app/...` destination to `build-dir/files/
#      ...` — build-init's build-dir *is* what /app maps to once exported).
#   3. `flatpak build-finish` — sets finish-args/command (same as the
#      manifest's finish-args + command fields).
#   4. `flatpak build-export` + `flatpak build-bundle` — unchanged from
#      the original flatpak-builder-based version.
#
# The manifest (flatpak/uk.nandi.sleek.json) is kept as the single source
# of truth for app-id/runtime/finish-args/build-commands even though
# nothing here parses it automatically — this genrule's cmd is written to
# match it by hand. If the manifest changes, update this genrule to match
# (there's no flatpak-builder invocation left to keep them in sync
# automatically).
def flatpak_genrule(name, manifest, app_id, host_target = "//:sleek-host"):
    """A genrule that packages `host_target`'s binary into a signed-less .flatpak bundle.

    - manifest: path to the flatpak-builder-shaped JSON manifest (e.g.
      "flatpak/uk.nandi.sleek.json") — kept as documentation/source-of-
      truth for the runtime/finish-args/build-commands this genrule's cmd
      hand-replicates (see module docstring above for why flatpak-builder
      itself isn't invoked). Declared as a src so a manifest edit without
      a matching genrule update at least shows up in the same diff.
    - app_id: the flatpak app ID (e.g. "uk.nandi.sleek") — used to name
      the final bundle and as build-init/build-bundle's ref name.
    """
    bundle_name = app_id + ".flatpak"

    native.genrule(
        name = name,
        srcs = [
            manifest,
            "assets/uk.nandi.sleek.desktop",
            "assets/uk.nandi.sleek.metainfo.xml",
            "assets/uk.nandi.sleek.svg",
            host_target,
        ],
        out = bundle_name,
        cmd = "\n".join([
            "set -euo pipefail",
            # Stage the buck2-built binary at a fixed name *before*
            # entering the subshell below — $(location host_target)
            # expands to a path relative to this script's original cwd
            # (repo root), and would resolve wrong once we cd into
            # flatpak-work/ (confirmed hitting exactly that: `install: No
            # such file or directory` from a relative buck-out/... path
            # that no longer existed under flatpak-work/).
            'cp "$(location {})" sleek-host-bin'.format(host_target),
            "chmod +x sleek-host-bin",
            # repo_relative_root = True means this cmd runs with the real
            # repo root as its cwd, not an isolated sandbox dir — so a
            # leftover flatpak-work/ from a prior local run (build-init
            # refuses to reinitialize an existing build-dir) must be swept
            # first for the genrule to be safely rerunnable.
            "rm -rf flatpak-work",
            # Subshell, not a bare `cd` — $OUT below is a path relative to
            # this script's *original* cwd (repo root); cd'ing for real
            # would break it, same reason cargo_apk_genrule's own
            # `(cd android/ && ...)` step is a subshell rather than a
            # plain `cd`.
            "(",
            "  mkdir -p flatpak-work && cd flatpak-work",
            # build-init sets up build-dir/{files,var,metadata} — no bwrap
            # involved, unlike flatpak-builder's own build-command
            # execution (see module docstring).
            "  flatpak build-init build-dir {} org.gnome.Sdk org.gnome.Platform 49".format(app_id),
            # Hand-replicated from flatpak/uk.nandi.sleek.json's
            # build-commands — /app/... there maps to build-dir/files/...
            # here (what build-init's build-dir becomes once exported).
            # The asset srcs below are plain source files, which
            # repo_relative_root = True (see bottom of this genrule)
            # already makes available at their real repo-relative paths —
            # same as how the original flatpak-builder-based cmd
            # referenced them.
            "  install -Dm755 ../sleek-host-bin build-dir/files/bin/sleek",
            "  install -Dm644 ../assets/uk.nandi.sleek.desktop build-dir/files/share/applications/{}.desktop".format(app_id),
            "  install -Dm644 ../assets/uk.nandi.sleek.metainfo.xml build-dir/files/share/metainfo/{}.metainfo.xml".format(app_id),
            "  install -Dm644 ../assets/uk.nandi.sleek.svg build-dir/files/share/icons/hicolor/scalable/apps/{}.svg".format(app_id),
            # Same finish-args as the manifest's finish-args + command.
            "  flatpak build-finish build-dir \\",
            "    --command=sleek \\",
            "    --share=network --share=ipc \\",
            "    --socket=fallback-x11 --socket=wayland --socket=pulseaudio \\",
            "    --device=dri --device=all \\",
            "    --filesystem=xdg-run/pipewire-0 --filesystem=xdg-download \\",
            "    --talk-name=org.freedesktop.Notifications --talk-name=org.freedesktop.portal.Desktop",
            "  flatpak build-export export-repo build-dir",
            "  flatpak build-bundle export-repo {} {}".format(bundle_name, app_id),
            ")",
            'cp "flatpak-work/{}" "$OUT"'.format(bundle_name),
        ]),
        # Same rationale as cargo.bzl's genrules — see cargo_genrule's own
        # comment. Meaningful for local execution only; RE workers get
        # flatpak + the pre-installed runtime from the custom RE image
        # instead (toolchains/rbe-image/Containerfile).
        repo_relative_root = True,
        always_print_stderr = True,
        visibility = ["PUBLIC"],
    )
