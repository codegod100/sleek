# Hermetic Rust distribution: fetches the official static.rust-lang.org
# release tarballs for rustc/rust-std/clippy and merges their rustup-style
# component trees into one sysroot directory, so toolchains/rust.bzl's
# compiler/rustdoc/clippy_driver RunInfos can point at plain
# `<sysroot>/bin/<tool>` paths — no pixi, no machine-specific absolute
# paths (unlike toolchains/pixi-tool.sh), reproducible from the pinned
# version + sha256 alone. Bump _VERSION/_DATE and the sha256 map together
# by reading https://static.rust-lang.org/dist/channel-rust-<version>.toml
# — don't trust a summarized/paraphrased read of that manifest for the
# hashes, grep the raw file (a paraphrasing tool has been observed
# fabricating plausible-looking hex digests here).
#
# Component layout (verified against the real tarballs): each archive's
# top-level dir is `<pkg>-<version>-<triple>/<component-name>/{bin,lib,...}`
# — rustc's component is named "rustc", rust-std's is
# "rust-std-<triple>", clippy's is "clippy-preview". strip_prefix drops
# everything up to and including the component dir, so all three land at
# the same relative paths (bin/, lib/, lib/rustlib/<triple>/{bin,lib}) with
# no collisions and can be merged into one tree — this is exactly what
# rustup itself does when installing a toolchain + a component into the
# same `~/.rustup/toolchains/<name>/` prefix. rustc finds its own sysroot
# relative to its own binary path (two dirs up from bin/rustc), so once
# merged, no --sysroot/RUSTC_SYSROOT override is needed.
_VERSION = "1.97.1"
_DATE = "2026-07-16"
_TRIPLE = "x86_64-unknown-linux-gnu"

# sha256 of each *.tar.xz, pinned from the raw channel manifest:
# https://static.rust-lang.org/dist/channel-rust-1.97.1.toml
_SHA256 = {
    "rustc": "9819d0a32d56bd339585319c80260e332779f5541fd66838ab7e016d6c814819",
    "rust-std": "1c1e704ae80126b7de34f72ea2825f7fd01736dec20732faed47374b95282fba",
    "clippy": "3441df8fb54db985f8c8a3e8356b8874a3f92cc8cca8565cfe36f1dc15935e72",
}

def _dist_url(pkg):
    return "https://static.rust-lang.org/dist/{}/{}-{}-{}.tar.xz".format(_DATE, pkg, _VERSION, _TRIPLE)

def rust_dist_sysroot(name, visibility = None):
    """Declares `<name>-rustc`/`-rust-std`/`-clippy` http_archive fetches plus a
    `name` genrule that merges them into one rustup-shaped sysroot dir."""
    native.http_archive(
        name = name + "-rustc",
        urls = [_dist_url("rustc")],
        sha256 = _SHA256["rustc"],
        strip_prefix = "rustc-{}-{}/rustc".format(_VERSION, _TRIPLE),
    )
    native.http_archive(
        name = name + "-rust-std",
        urls = [_dist_url("rust-std")],
        sha256 = _SHA256["rust-std"],
        strip_prefix = "rust-std-{}-{}/rust-std-{}".format(_VERSION, _TRIPLE, _TRIPLE),
    )
    native.http_archive(
        name = name + "-clippy",
        urls = [_dist_url("clippy")],
        sha256 = _SHA256["clippy"],
        strip_prefix = "clippy-{}-{}/clippy-preview".format(_VERSION, _TRIPLE),
    )
    native.genrule(
        name = name,
        srcs = [":{}-rustc".format(name), ":{}-rust-std".format(name), ":{}-clippy".format(name)],
        out = "sysroot",
        cmd = (
            "mkdir -p $OUT && " +
            "cp -RL $(location :{n}-rustc)/. $OUT/ && ".format(n = name) +
            "cp -RL $(location :{n}-rust-std)/. $OUT/ && ".format(n = name) +
            "cp -RL $(location :{n}-clippy)/. $OUT/".format(n = name)
        ),
        visibility = visibility,
    )
