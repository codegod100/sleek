load(":cargo.bzl", "cargo_aab_genrule", "cargo_apk_genrule", "cargo_genrule")
load(":flatpak.bzl", "flatpak_genrule")

# `buck2 build //:sleek-host` / `buck2 run //:sleek-host`
# Equivalent to `just host` (desktop egui/eframe window), minus the DISPLAY /
# Codespace VNC wiring justfile's `host` recipe adds around the cargo call.
cargo_genrule(
    name = "sleek-host",
    manifest = "host/Cargo.toml",
    cargo_args = "--release",
    collect_cmd = 'cp host/target/release/sleek "$OUT"',
    out = "sleek",
    executable = True,
)

# Short local spelling: `buck2 run :sleek`.
alias(
    name = "sleek",
    actual = ":sleek-host",
)

# `buck2 build //:sleek-android-lib`
# Equivalent to `just lib` — the android/ package built as a library for the
# desktop target (sanity-check build; NOT the cross-compiled aarch64 APK —
# see //:sleek-android-apk below, or `just android` / cargo-apk + the NDK
# via nix, for that).
cargo_genrule(
    name = "sleek-android-lib",
    manifest = "android/Cargo.toml",
    cargo_args = "--lib --release",
    collect_cmd = """
mkdir -p "$OUT"
cp android/target/release/libsleek.so "$OUT/libsleek.so"
cp android/target/release/libsleek.rlib "$OUT/libsleek.rlib"
""",
    outs = {
        "rlib": ["libsleek.rlib"],
        "so": ["libsleek.so"],
    },
    default_outs = ["libsleek.so"],
)

# `buck2 build //:sleek-android-apk`
# The real, installable aarch64 APK — buck2/RBE equivalent of `nix build
# .#android` (see flake.nix's sleek-android derivation, which this mirrors
# step for step). Output is a single signed sleek.apk:
#   buck2 build --show-output //:sleek-android-apk
#   adb install -r buck-out/.../sleek.apk
cargo_apk_genrule(
    name = "sleek-android-apk",
    manifest = "android/Cargo.toml",
    package = "sleek",
)

# `buck2 build //:sleek-android-aab`
# Signed Android App Bundle for Google Play Store submission — same cargo-apk
# pipeline as //:sleek-android-apk, then converted to AAB format via aapt2
# (proto resources) + bundletool, and signed with the CI keystore. Requires
# bundletool.jar on the worker (baked into the RBE image; see
# toolchains/rbe-image/Containerfile — rebuilt after each Containerfile change).
#   buck2 build --show-output //:sleek-android-aab
#   # Upload buck-out/.../sleek.aab to Google Play Console
cargo_aab_genrule(
    name = "sleek-android-aab",
    manifest = "android/Cargo.toml",
    package = "sleek",
)

# `buck2 build //:sleek-flatpak`
# Packages //:sleek-host into a distributable .flatpak bundle (GNOME
# Platform/49 runtime, uk.nandi.sleek app ID) — buck2/RBE equivalent of
# `nix build .#flatpak` (see flake.nix's nix2flatpak-based sleek-flatpak
# derivation, which this mirrors in appId/runtime/permissions but builds
# via plain flatpak-builder instead — nix2flatpak has no buck2 analog).
# Output: buck-out/.../uk.nandi.sleek.flatpak
#   buck2 build --show-output //:sleek-flatpak
#   flatpak install --user buck-out/.../uk.nandi.sleek.flatpak
flatpak_genrule(
    name = "sleek-flatpak",
    manifest = "flatpak/uk.nandi.sleek.json",
    app_id = "uk.nandi.sleek",
)
