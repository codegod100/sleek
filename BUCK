load(":cargo.bzl", "cargo_genrule")

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

# `buck2 build //:sleek-android-lib`
# Equivalent to `just lib` — the android/ package built as a library for the
# desktop target (sanity-check build; NOT the cross-compiled aarch64 APK,
# which still goes through `just android` / cargo-apk + the NDK via nix).
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
