# Flatpak-backed genrule wrapper.
#
# Unlike cargo.bzl's genrules (which shell out to a real `cargo build`),
# this one doesn't compile anything itself — it packages the *already
# buck2-built* //:sleek-host binary into a distributable .flatpak bundle
# via flatpak-builder + flatpak build-export/build-bundle, reusing the
# org.gnome.Platform//49 runtime (baked into toolchains/rbe-image/
# Containerfile at image-build time, same pattern as the Android
# NDK/SDK) instead of vendoring every shared library sleek links against.
#
# Ports flake.nix's old `nix2flatpak.lib.${system}.mkFlatpak` derivation
# (see git history) to a plain flatpak-builder manifest + buck2 genrule —
# nix2flatpak itself has no buck2 equivalent (it works off a Nix closure),
# but the actual flatpak (appId/runtime/permissions/appdata/desktopFile)
# is identical. Confirmed locally (this sandbox already has flatpak-builder
# + org.gnome.Platform//49 + org.gnome.Sdk//49 installed): the resulting
# bundle is ~17MB, not the old Nix build's 617MB — reusing the shared
# runtime instead of bundling a whole Nix closure is a real size win, not
# just a packaging-mechanism swap.
def flatpak_genrule(name, manifest, app_id, host_target = "//:sleek-host"):
    """A genrule that packages `host_target`'s binary into a signed-less .flatpak bundle.

    - manifest: path to the flatpak-builder JSON manifest (e.g.
      "flatpak/uk.nandi.sleek.json"). Its module's `sources` must reference
      the binary at manifest-relative path "../host/target/release/sleek"
      (matching cargo_genrule("sleek-host")'s own real-repo-relative
      output path) — this genrule stages `host_target`'s buck-out artifact
      there before invoking flatpak-builder, same reason
      _STAGE_PATCHED_FORKS exists in cargo.bzl (a target dependency lands
      at its own buck-out path, not the real repo-relative one a manifest
      written for local `flatpak-builder` use expects).
    - app_id: the flatpak app ID (e.g. "uk.nandi.sleek") — used to name
      the final bundle and as build-bundle's ref name.
    """
    manifest_dir = manifest[: manifest.rfind("/") + 1]
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
            # Stage the buck2-built binary at the real repo-relative path
            # the manifest's source list expects (see docstring above) —
            # same shape as cargo_genrule("sleek-host")'s own collect_cmd
            # output, just copied here instead of freshly compiled.
            "mkdir -p host/target/release",
            'cp "$(location {})" host/target/release/sleek'.format(host_target),
            "chmod +x host/target/release/sleek",
            # Subshell, not a bare `cd` — $OUT below is a path relative to
            # this script's *original* cwd (repo root); cd'ing for real
            # would break it, same reason cargo_apk_genrule's own
            # `(cd android/ && ...)` step is a subshell rather than a
            # plain `cd`. Confirmed hitting exactly that (a bare `cd` here
            # made the final cp look for buck-out/... under flatpak/).
            "(",
            "  cd {}".format(manifest_dir),
            # --disable-rofiles-fuse: RE workers may not have /dev/fuse
            # available/permitted; the fuse-backed rofiles optimization is
            # a local-build speedup only, not required for correctness.
            "  flatpak-builder --force-clean --disable-rofiles-fuse build-dir {}".format(manifest.split("/")[-1]),
            "  flatpak build-export export-repo build-dir",
            "  flatpak build-bundle export-repo {} {}".format(bundle_name, app_id),
            ")",
            'cp "{}{}" "$OUT"'.format(manifest_dir, bundle_name),
        ]),
        # Same rationale as cargo.bzl's genrules — see cargo_genrule's own
        # comment. Meaningful for local execution only; RE workers get
        # flatpak/flatpak-builder + the pre-installed runtime from the
        # custom RE image instead (toolchains/rbe-image/Containerfile).
        repo_relative_root = True,
        always_print_stderr = True,
        visibility = ["PUBLIC"],
    )
