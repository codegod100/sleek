# Cargo-backed genrule wrapper.
#
# sleek's dependency graph (eframe/egui with vendored patched forks, wasmtime,
# git-pinned iroh/noq/web-transport, a build.rs that shells out to `gleam`)
# is too large and too non-crates.io to hand-translate into native buck2
# rust_library/rust_binary targets. Instead, each target here just runs the
# real `cargo build` and buck2 tracks only the final artifact(s). Runs
# locally by default (reusing its own on-disk incremental target/ cache —
# fast), but is also remote-execution-capable now (platforms/defs.bzl's
# custom RE image bakes in pixi + the Android NDK) — remotely there's no
# incremental cache to reuse, so it's a full `cargo build` from scratch
# each time, slower but still correct.
#
# Toolchain comes from pixi (pixi.toml), not nix: `pixi run --` self-activates
# the env for just this one command, whether or not a `pixi shell` is already
# active — no ambient-env check needed, and no dependency on `/nix` existing
# at all. This also sidesteps a buck2 gotcha: the buck2 daemon captures its
# environment at spawn time, not per-command, so anything that instead relied
# on the *invoking shell* already being activated would silently break the
# moment a daemon is left running from before that activation.
_ENTER = "pixi run --"

# Source tree every cargo-backed target depends on: both workspaces
# (host/, android/), the vendored+patched crates (now under third-party/ —
# also reindeer's native-rules vendor dir, see reindeer.toml) and git-rev
# pins they `[patch]` onto, the cargo config that wires SSL/registry
# settings, and the pixi manifest/lockfile `pixi run` reads to build its env.
#
# native.glob (not the bare `glob`) — this runs inside a .bzl loaded by a
# BUCK file, not the BUCK file itself.
def _srcs():
    # native.glob() cannot cross a package boundary: third-party/ carries its
    # own reindeer-generated BUCK file (reindeer.toml), which makes it a
    # separate buck2 package from this one — a glob() pattern rooted here
    # (e.g. "third-party/egui-winit/**") silently matches zero files, no
    # error, even though the real files are right there on disk. This was
    # invisible locally (cargo_genrule's repo_relative_root = True bypasses
    # buck2's staged sandbox entirely and reads the real filesystem), but
    # under RBE only what's actually in `srcs` gets uploaded to the remote
    # worker, so those patched forks were silently absent there — a `cargo
    # build` remotely couldn't find them.
    #
    # cpal/egui-winit/egui_glow/khronos_api are genuine reindeer-buckified
    # crates (real reindeer-managed git checkouts and/or real dependency
    # edges inside third-party/BUCK's own graph — e.g. ":egui-winit-0.31"/
    # ":khronos_api-3" being real `srcs` deps of other targets there, and
    # cpal's own generated `cpal-0.18` target's srcs list is package-
    # relative, so even moving *just* cpal out broke third-party/BUCK's
    # ability to parse at all), so none of them can move out of that
    # package. Instead reindeer.toml's `buckfile_imports` (raw Starlark
    # spliced verbatim into every regenerated third-party/BUCK) defines a
    # small filegroup — `//third-party:patched-fork-srcs` — that glob()s
    # them from *inside* their own package (no boundary to cross there) and
    # exports the result as a target label, which *does* cross package
    # boundaries fine (unlike a bare path string — also verified: silently
    # dropped, same as glob).
    #
    # vidya/freeq have no such coupling — plain filesystem path deps, never
    # touched by reindeer/Cargo.lock at all — so they live in
    # patched-crates/, a plain non-reindeer-owned directory in *this*
    # package, where glob() just works directly.
    #
    # assets/** (app icons) — android/src/app.rs include_bytes!()s
    # ../assets/icons/*.png relative to its own crate dir, i.e. this
    # top-level assets/ dir. Missing here for the same reason freeq/vidya
    # were: worked locally via repo_relative_root's real-filesystem
    # bypass, silently absent for remote workers.
    return native.glob(
        [
            "host/**",
            "android/**",
            "assets/**",
            "patched-crates/vidya/**",
            "patched-crates/freeq/**",
            "patches/**",
            ".cargo/**",
            "pixi.toml",
            "pixi.lock",
            "scripts/pixi-activate.sh",
        ],
        exclude = [
            "host/target/**",
            "android/target/**",
        ],
    ) + [
        "//:patched-fork-srcs",
    ]

# `//third-party:patched-fork-srcs` (see _srcs()) is a *target*, not a raw
# glob-matched file — repo_relative_root's genrule staging only remaps raw
# file srcs onto their real repo-relative path; a target dependency
# instead lands at its own buck-out artifact path. Depending on the Buck
# materializer, that artifact either starts at cpal/ or preserves the source
# prefix as third-party/cpal/. So stage it ourselves via $(location) before
# cargo ever runs. `-n` (no-clobber): locally repo_relative_root already
# runs in the real repo root, where
# third-party/{cpal,egui-winit,egui_glow,khronos_api} already exist for
# real — this is a no-op there. Remotely third-party/ doesn't exist at all
# yet (that's the whole bug this works around), so the copy is what
# actually populates it. Shared by every cargo-backed genrule below since
# they all build off the same [patch]-carrying Cargo.tomls.
_STAGE_PATCHED_FORKS = (
    'mkdir -p third-party\n' +
    'forks="$(location //:patched-fork-srcs)"\n' +
    '[ ! -d "$forks/third-party" ] || forks="$forks/third-party"\n' +
    'cp -rn "$forks"/* third-party/\n'
)

def cargo_genrule(name, manifest, cargo_args, collect_cmd, out = None, outs = None, default_outs = None, executable = False):
    """A genrule that shells out to `cargo build` (via `pixi run`) and copies its artifact(s) to $OUT.

    - manifest: path to the Cargo.toml to build (e.g. "host/Cargo.toml").
    - cargo_args: extra argv appended to `cargo build --manifest-path
      <manifest>` (e.g. "--release" or "--lib --release") — words, no shell
      metacharacters, passed straight through to `pixi run --` as `"$@"`.
    - collect_cmd: shell copying the resulting artifact(s) from cargo's real
      target/ dir into $OUT (single-file `out`) or $OUT/... (dir-shaped `outs`).

    Buck2 buffers a genrule's own stdout/stderr and only surfaces it once the
    whole action finishes — cargo's own "Compiling …" progress otherwise
    never shows up while a build is running. So cargo's output is teed to
    <manifest's dir>/cargo-build.log too (`tail -f host/cargo-build.log` —
    or `just host-log` / `just lib-log`), and `always_print_stderr` makes
    buck2 print the captured copy promptly rather than only on failure.
    """
    build_cmd = "cargo build --manifest-path {} {}".format(manifest, cargo_args)
    log_path = manifest[:-len("Cargo.toml")] + "cargo-build.log"

    native.genrule(
        name = name,
        srcs = _srcs(),
        out = out,
        outs = outs,
        default_outs = default_outs,
        # `| tail -c 200000` on the *displayed* copy only — the full,
        # untruncated output still goes to log_path via tee. Without this,
        # a real cargo build's full "Compiling ..." output (hundreds of
        # crates) blows past buck2's own stdout-capture cap and gets
        # replaced with a bare "Result too large to display" — including
        # for the actual error, on failure. Confirmed hitting exactly that
        # trying to see why a remote sleek-android-lib build failed.
        cmd = 'set -euo pipefail\n{}{} {} 2>&1 | tee "{}" | tail -c 200000\n{}'.format(_STAGE_PATCHED_FORKS, _ENTER, build_cmd, log_path, collect_cmd),
        # Real repo layout (not a symlinked sandbox): cargo/rustc need the
        # actual `.git`, `target/` incremental cache, and CARGO_HOME — and
        # `pixi run` needs to find pixi.toml from the real root. Meaningful
        # for local execution only — a remote worker has no such thing
        # regardless of this flag; see platforms/defs.bzl for the RE image
        # (toolchains/rbe-image/) that gives it what it needs there instead
        # (network access + pixi + the Android NDK baked in).
        repo_relative_root = True,
        # NOT `labels = ["network_access"]` (used to be): that label is in
        # prelude//genrule_local_labels.bzl's require-local allowlist, which
        # forces buck2 to run the action locally *unconditionally*,
        # regardless of platforms/defs.bzl's remote_enabled — the whole
        # point of the custom RE image below is to let this run remotely,
        # so it can't carry that label. `"network": "bridge"` in
        # platforms/defs.bzl's remote_execution_properties is what actually
        # gives build.rs (crates.io, git deps) network access instead.
        always_print_stderr = True,
        executable = executable,
        visibility = ["PUBLIC"],
    )

def cargo_aab_genrule(name, manifest, package, target = "aarch64-linux-android"):
    """Produces a signed .aab (Android App Bundle) for Google Play Store submission.

    Mirrors cargo_apk_genrule step-for-step to build the release APK, then
    converts it to AAB format:
      1. aapt2 convert --output-format proto  — AXML → proto binary XML
      2. bundletool build-bundle              — assemble the AAB
      3. jarsigner                            — sign with the CI upload key

    The resulting bundle is suitable for direct upload to Google Play Console.
    See android/scripts/apk-to-aab.sh for the conversion details.

    Requires bundletool.jar on the remote worker — baked into the RBE image at
    $BUNDLETOOL_JAR (toolchains/rbe-image/Containerfile). Locally, the script
    downloads it on first use to $XDG_CACHE_HOME/bundletool/ if not already set.
    """
    android_dir = manifest[:-len("Cargo.toml")]
    log_path = android_dir + "cargo-aab-build.log"

    # Same signing injection as cargo_apk_genrule — see that function's comment.
    inject_signing = 'python3 {}scripts/inject-release-signing.py {} "$PWD/{}ci.keystore"\n'.format(android_dir, manifest, android_dir)

    # NOT --manifest-path: same cargo-apk quirk as cargo_apk_genrule — see comment there.
    build_cmd = "cargo apk build --release --target {} -p {} --lib".format(target, package)

    # Same APK discovery logic as cargo_apk_genrule.
    find_apk = (
        'apk=""\n' +
        'for cand in "{ad}target/release/apk/{pkg}.apk" "{ad}target/{pkg}.apk" "{ad}target/release/apk/{pkg}-release.apk"; do\n'.format(ad = android_dir, pkg = package) +
        '  if [ -f "$cand" ]; then apk="$cand"; break; fi\n' +
        "done\n" +
        'if [ -z "$apk" ]; then\n' +
        '  apk="`find {}target -type f -path "*/release/apk/*.apk" ! -name "*-unaligned.apk" 2>/dev/null | head -1 || true`"\n'.format(android_dir) +
        "fi\n" +
        '[ -n "$apk" ] && [ -f "$apk" ] || { echo "APK not found under ' + android_dir + 'target" >&2; exit 1; }\n'
    )

    # Convert APK → AAB using android/scripts/apk-to-aab.sh.
    # The script handles aapt2 convert, bundletool, and jarsigner internally.
    convert_aab = (
        'export SLEEK_KEYSTORE="$PWD/{ad}ci.keystore"\n'.format(ad = android_dir) +
        "export SLEEK_KEYSTORE_PASSWORD=android\n" +
        "export SLEEK_KEY_ALIAS=androiddebugkey\n" +
        "export SLEEK_KEY_PASSWORD=android\n" +
        'bash {ad}scripts/apk-to-aab.sh "$apk" {ad}src/assets/sleek_activity.dex "$OUT"\n'.format(ad = android_dir)
    )

    native.genrule(
        name = name,
        srcs = _srcs(),
        out = "sleek.aab",
        cmd = 'set -euo pipefail\n{stage}{sign}(cd {ad} && {enter} {build}) 2>&1 | tee "{log}" | tail -c 200000\n{find_apk}{convert_aab}'.format(
            stage = _STAGE_PATCHED_FORKS,
            sign = inject_signing,
            ad = android_dir,
            enter = _ENTER,
            build = build_cmd,
            log = log_path,
            find_apk = find_apk,
            convert_aab = convert_aab,
        ),
        # Same rationale as cargo_apk_genrule — see its own comment.
        repo_relative_root = True,
        always_print_stderr = True,
        visibility = ["PUBLIC"],
    )

def cargo_apk_genrule(name, manifest, package, target = "aarch64-linux-android"):
    """A genrule that runs `cargo apk build --release` and produces a signed sleek.apk.

    - manifest: path to android's Cargo.toml (e.g. "android/Cargo.toml").
    - package: crate name to build (`-p <package>`) — cargo-apk rejects
      multi-member workspaces without one selected.
    - target: Android Rust target triple. Default aarch64-linux-android
      (real phones); toolchains/rbe-image/Containerfile also provisions
      an x86_64-linux-android linker (Waydroid/emulator) if ever needed.

    Mirrors flake.nix's `sleek-android` derivation (`nix build .#android`)
    step for step — same cargo-apk invocation, the same committed CI
    keystore (android/ci.keystore — shared across builds so adb/phone
    upgrades don't break with "App not installed", the way ephemeral keys
    would), the same dex-injection script for SleekActivity (so `freeq://`
    OAuth deep links work; cargo-apk alone only ships bare NativeActivity)
    — just as a buck2 genrule (RBE-capable, see toolchains/rbe-image/) in
    place of a Nix builder.
    """
    android_dir = manifest[:-len("Cargo.toml")]
    log_path = android_dir + "cargo-apk-build.log"

    # cargo-apk's --release needs [package.metadata.android.signing.release]
    # in Cargo.toml, with an *absolute* keystore path — can't be a
    # committed static value (repo checkout location isn't fixed: local
    # dev, buck2's RBE execroot, etc. are all different absolute paths) —
    # so inject it at build time instead, same as flake.nix's build does
    # (shared script, see its own comment for why it's not just inlined
    # Python here).
    # $PWD (bash builtin), not $(pwd) — buck2's genrule `cmd` scans for
    # $(...) anywhere in the string and tries to resolve it as one of
    # *its own* macros ($(location ...), $(exe ...), …); an unrecognized
    # one like $(pwd) or $(find ...) below isn't silently passed through
    # to the shell, it errors ("no mapping for pwd") — confirmed hitting
    # exactly that earlier standing up a throwaway diagnostic genrule.
    inject_signing = 'python3 {}scripts/inject-release-signing.py {} "$PWD/{}ci.keystore"\n'.format(android_dir, manifest, android_dir)

    # NOT --manifest-path: cargo-apk's own --manifest-path mode rejects
    # android/Cargo.toml's self-referencing `[workspace] members = ["."]`
    # (added specifically *for* cargo-apk — see that file's own comment)
    # with "Did not expect a [workspace]" — a real quirk/limitation of
    # this cargo-apk version, confirmed hitting it. flake.nix's build
    # sidesteps this the same way: `cd` into android/ first, invoke
    # plain `cargo apk build` there. Mirrored via a `(cd ... && ...)`
    # subshell below so cwd reverts for the rest of this script
    # afterwards (find_apk/inject_dex stay repo-root-relative).
    build_cmd = "cargo apk build --release --target {} -p {} --lib".format(target, package)

    # cargo-apk's own output path has shifted across versions (bare
    # target/sleek.apk vs target/release/apk/sleek.apk) — flake.nix's
    # derivation already hedges across both known shapes plus a fallback
    # find(); mirrored verbatim rather than assuming just one.
    find_apk = (
        'apk=""\n' +
        'for cand in "{ad}target/release/apk/{pkg}.apk" "{ad}target/{pkg}.apk" "{ad}target/release/apk/{pkg}-release.apk"; do\n'.format(ad = android_dir, pkg = package) +
        '  if [ -f "$cand" ]; then apk="$cand"; break; fi\n' +
        "done\n" +
        'if [ -z "$apk" ]; then\n' +
        # Backticks, not $(find ...) — same buck2 macro-collision reason
        # as $PWD above.
        '  apk="`find {}target -type f -path "*/release/apk/*.apk" ! -name "*-unaligned.apk" 2>/dev/null | head -1 || true`"\n'.format(android_dir) +
        "fi\n" +
        '[ -n "$apk" ] && [ -f "$apk" ] || { echo "APK not found under ' + android_dir + 'target" >&2; exit 1; }\n'
    )

    inject_dex = (
        'export SLEEK_KEYSTORE="$PWD/{ad}ci.keystore"\n'.format(ad = android_dir) +
        "export SLEEK_KEYSTORE_PASSWORD=android\n" +
        "export SLEEK_KEY_ALIAS=androiddebugkey\n" +
        "export SLEEK_KEY_PASSWORD=android\n" +
        'bash {ad}scripts/inject-activity-dex.sh "$apk" {ad}src/assets/sleek_activity.dex\n'.format(ad = android_dir) +
        'cp "$apk" "$OUT"\n'
    )

    native.genrule(
        name = name,
        srcs = _srcs(),
        out = "sleek.apk",
        cmd = 'set -euo pipefail\n{stage}{sign}(cd {ad} && {enter} {build}) 2>&1 | tee "{log}" | tail -c 200000\n{find_apk}{inject_dex}'.format(
            stage = _STAGE_PATCHED_FORKS,
            sign = inject_signing,
            ad = android_dir,
            enter = _ENTER,
            build = build_cmd,
            log = log_path,
            find_apk = find_apk,
            inject_dex = inject_dex,
        ),
        # Same rationale as cargo_genrule — see its own comment.
        repo_relative_root = True,
        always_print_stderr = True,
        visibility = ["PUBLIC"],
    )

def relm4_apk_genrule(name):
    """Build the GTK/Relm4 Android application in the Android RBE image."""
    native.genrule(
        name = name,
        srcs = _srcs() + native.glob([
            "gtk-android-builder/**",
            "meson.build",
            "scripts/build-relm4-android.py",
            "scripts/cargo-build-relm4-cdylib.sh",
        ]),
        out = "sleek-relm4.apk",
        cmd = (
            "set -euo pipefail\n" +
            _STAGE_PATCHED_FORKS +
            "python3 scripts/build-relm4-android.py " +
            "--android-home /opt/android/sdk " +
            "--ndk /opt/android/ndk " +
            "--output \"$OUT\"\n"
        ),
        repo_relative_root = True,
        always_print_stderr = True,
        visibility = ["PUBLIC"],
    )
