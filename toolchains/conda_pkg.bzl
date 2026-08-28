# Fetches a conda-forge package (a plain URL, same as pixi.lock already
# resolves — see pixi.lock for exact package/version/hash pins) as a buck2
# target, replacing pixi's own install of it into `.pixi/envs/default`.
#
# A `.conda` file is a zip containing (among other things) a
# `pkg-<name>-<version>-<build>.tar.zst` member holding the actual
# lib/include/pkgconfig payload — verified against the real files:
#   $ unzip -l alsa-lib-1.2.16.1-hb03c661_0.conda
#     pkg-alsa-lib-1.2.16.1-hb03c661_0.tar.zst
#     info-alsa-lib-1.2.16.1-hb03c661_0.tar.zst   (metadata, unused here)
#
# Any bundled .pc files ship with a conda-relocation placeholder prefix
# (`_placehold_` repeated to pad the string to a fixed length so conda's
# own installer can do an in-place find/replace without relinking) — e.g.
# `prefix=/home/conda/feedstock_root/build_artifacts/alsa-lib_.../_h_env_
# _placehold_placehold_..._pl`. We're not running conda's installer, so
# rewrite prefix=/exec_prefix= to this target's own real, final output
# path instead. That path is knowable at genrule-execution time (verified:
# buck2 genrule actions execute in place at their final buck-out location,
# not a scratch dir that gets renamed afterward — `realpath "$OUT"` inside
# the action's own cmd equals what `buck2 build --show-full-output` later
# reports for the same target).
def conda_pkg(name, url, sha256, visibility = None):
    archive_name = name + "-conda-archive"
    native.http_archive(
        name = archive_name,
        urls = [url],
        sha256 = sha256,
        type = "zip",
    )
    native.genrule(
        name = name,
        srcs = [":" + archive_name],
        out = "out",
        cmd = (
            "set -euo pipefail; " +
            "payload=$(location :{a})/pkg-*.tar.zst; ".format(a = archive_name) +
            "mkdir -p $OUT && tar --use-compress-program=$(exe toolchains//:zstd) -xf $payload -C $OUT; " +
            # \$(...) (not $(...)): buck2 itself parses genrule cmd strings
            # for $(...) macros — has to be escaped to reach the shell as a
            # literal command substitution instead.
            "real=\\$(cd $OUT && pwd); " +
            "if [ -d \"$OUT/lib/pkgconfig\" ]; then " +
            "  sed -i \"s|^prefix=.*|prefix=${real}|; s|^exec_prefix=.*|exec_prefix=${real}|\" $OUT/lib/pkgconfig/*.pc; " +
            "fi"
        ),
        visibility = visibility,
    )
