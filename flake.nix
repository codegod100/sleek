{
  description = "Sleek — mobile freeq client (Vidya + freeq-sdk)";

  # Trusted when accept-flake-config = true (bootstrap sets this on Codespaces).
  nixConfig = {
    extra-substituters = [ "https://codegod100.cachix.org" ];
    extra-trusted-public-keys = [
      "codegod100.cachix.org-1:LZFL5VrR644WUjleS3bLbVeOdzlXqzKznQWvD5MVthA="
    ];
  };

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    # Path deps in Cargo.toml are ../../vidya and ../../freeq/freeq-sdk —
    # pin them as flake inputs so `nix build` works without a monorepo checkout.
    # GitHub mirror of tangled.org/nandi.uk/vidya (includes video_player).
    vidya = {
      url = "github:codegod100/vidya";
      flake = false;
    };
    freeq = {
      url = "github:codegod100/freeq";
      flake = false;
    };
    # Convert the hermetic host package into a distributable .flatpak bundle.
    nix2flatpak.url = "github:neobrain/nix2flatpak";
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      vidya,
      freeq,
      nix2flatpak,
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      pkgsFor =
        system:
        import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
          config = {
            allowUnfree = true;
            android_sdk.accept_license = true;
          };
        };

      # Runtime libs for the desktop host (wrapped into LD_LIBRARY_PATH).
      # Keep bindgen-only deps (libclang, linuxHeaders, pkg-config) out of this
      # list — they bloat the closure by ~GB and are not needed at runtime.
      eguiLibs =
        pkgs:
        with pkgs;
        [
          libxkbcommon
          libGL
          vulkan-loader
          openssl
        ]
        ++ lib.optionals stdenv.hostPlatform.isLinux [
          wayland
          libx11
          libxcursor
          libxi
          libxrandr
          # Software GL for Codespaces desktop-lite / headless X11 (llvmpipe)
          mesa
          # freeq AV MoQ media (cpal / iroh-live audio + camera)
          alsa-lib
          # Optional PipeWire backends
          pipewire
        ];

      # Build-only extras for v4l2r bindgen (iroh-live capture-camera).
      eguiBuildLibs =
        pkgs: with pkgs; [
          pkg-config
          llvmPackages.libclang
          linuxHeaders
        ];

      # rusty-capture/v4l2r bindgen on NixOS: must use nix libclang (not Android
      # NDK — that needs libz.so.1 and is the wrong toolchain) plus glibc +
      # linux headers + clang resource dir so videodev2.h can find sys/time.h.
      v4l2BindgenEnv =
        pkgs:
        let
          libclang = pkgs.llvmPackages.libclang;
          clangMajor = pkgs.lib.versions.major libclang.version;
        in
        {
          LIBCLANG_PATH = "${libclang.lib}/lib";
          # also expose for justfile to re-assert after ambient env pollution
          SLEEK_LIBCLANG_PATH = "${libclang.lib}/lib";
          BINDGEN_EXTRA_CLANG_ARGS = pkgs.lib.concatStringsSep " " [
            "-isystem ${pkgs.glibc.dev}/include"
            "-I${pkgs.linuxHeaders}/include"
            "-isystem ${libclang.lib}/lib/clang/${clangMajor}/include"
          ];
          # v4l2r defaults to /usr/include/linux which is missing on NixOS
          V4L2R_VIDEODEV2_H_PATH = "${pkgs.linuxHeaders}/include/linux";
        };

      # Layout expected by android/Cargo.toml path deps:
      #   parent/sleek/{android,host}
      #   parent/vidya
      #   parent/freeq/freeq-sdk
      sleekSrcTree =
        pkgs:
        let
          # Filter before hashing so CI/docs/flake-only commits do not bust
          # android/flatpak store paths (and thus Cachix).
          sleekFiltered = pkgs.lib.cleanSourceWith {
            src = ./.;
            filter =
              path: type:
              let
                base = baseNameOf path;
              in
              # Keep normal cleanSource exclusions, then drop non-build paths.
              pkgs.lib.cleanSourceFilter path type
              && !(builtins.elem base [
                ".tangled"
                ".github"
                ".devcontainer"
                ".jj"
                ".cargo"
                "docs"
                "README.md"
                "AGENTS.md"
                "justfile"
                "flake.nix"
                "flake.lock"
                "result"
                "result-android"
                "result-flatpak"
                "result-sleek"
              ]);
          };
        in
        pkgs.runCommand "sleek-src-tree"
          {
            nativeBuildInputs = [ pkgs.rsync ];
          }
          ''
            mkdir -p $out/{sleek,vidya,freeq}
            cp -a ${sleekFiltered}/. $out/sleek/
            cp -a ${vidya}/. $out/vidya/
            cp -a ${freeq}/. $out/freeq/
            chmod -R u+w $out
            # Drop heavy/irrelevant freeq crates so cargo metadata stays lean
            # (path dep only needs freeq-sdk + its workspace graph).
            rm -rf $out/sleek/{.git,host/target,android/target} 2>/dev/null || true
          '';

      androidApiLevel = "28";
      androidTarget = "aarch64-linux-android";
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          libs = eguiLibs pkgs;
          buildLibs = eguiBuildLibs pkgs;
          # Lean toolchain for store packages (no clippy/rustfmt — not needed to compile).
          rustBuild = pkgs.rust-bin.stable.latest.default;
          # Full toolchain for iterative `nix run .#host` / cargo workflows.
          rust = pkgs.rust-bin.stable.latest.default.override {
            extensions = [
              "rust-src"
              "rustfmt"
              "clippy"
            ];
          };
          # aarch64 for phone APK; x86_64 for Waydroid (host container).
          rustAndroid = pkgs.rust-bin.stable.latest.default.override {
            extensions = [
              "rust-src"
              "rustfmt"
              "clippy"
            ];
            targets = [
              androidTarget
              "x86_64-linux-android"
            ];
          };
          rustPlatform = pkgs.makeRustPlatform {
            cargo = rustBuild;
            rustc = rustBuild;
          };
          rustPlatformAndroid = pkgs.makeRustPlatform {
            cargo = rustAndroid;
            rustc = rustAndroid;
          };
          srcTree = sleekSrcTree pkgs;

          sleek-host = rustPlatform.buildRustPackage (
            {
              pname = "sleek";
              version = "0.1.0";
              src = srcTree;

              # Build the desktop host binary (package name sleek-host, bin name sleek).
              cargoRoot = "sleek/host";
              buildAndTestSubdir = "sleek/host";

              cargoLock = {
                lockFile = ./host/Cargo.lock;
                # Path deps (vidya, freeq-*) have no crates.io source.
                allowBuiltinFetchGit = true;
              };

              nativeBuildInputs = with pkgs; [
                makeWrapper
                copyDesktopItems
                removeReferencesTo
              ]
              ++ buildLibs;
              buildInputs = libs;

              OPENSSL_NO_VENDOR = "1";
              PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";
              doCheck = false;
              # Drop symbols; panic location strings still need remove-references-to.
              stripAll = true;
              # Avoid embedding full DWARF from cargo's default release profile.
              CARGO_PROFILE_RELEASE_DEBUG = "0";
              CARGO_PROFILE_RELEASE_STRIP = "symbols";

              # rustc/libclang paths get baked into the binary/wrapper; scrub so
              # the runtime closure (and thus Cachix/CI substitutes) stay small.
              disallowedReferences = [
                rustBuild
                pkgs.llvmPackages.libclang
                pkgs.llvmPackages.libclang.lib
              ];

              # Binary is named `sleek` (see host/Cargo.toml [[bin]]).
              # Desktop entry + icons for app menus / launchers.
              desktopItems = [
                (pkgs.makeDesktopItem {
                  name = "uk.nandi.sleek";
                  desktopName = "Sleek";
                  genericName = "Chat";
                  comment = "Freeq chat client — channels, DMs, and calls";
                  exec = "sleek";
                  # Name-based initially; preFixup rewrites to an absolute PNG
                  # path so launchers that skip nix-profile hicolor still work.
                  icon = "uk.nandi.sleek";
                  categories = [
                    "Network"
                    "Chat"
                    "InstantMessaging"
                  ];
                  keywords = [
                    "freeq"
                    "irc"
                    "chat"
                    "call"
                    "video"
                  ];
                  startupNotify = true;
                  # Must match ViewportBuilder::with_app_id("uk.nandi.sleek").
                  startupWMClass = "uk.nandi.sleek";
                  terminal = false;
                })
              ];

              postInstall = ''
                wrapProgram $out/bin/sleek \
                  --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath libs}

                # Freedesktop icon theme (scalable + common raster sizes).
                install -Dm644 ${./assets/uk.nandi.sleek.svg} \
                  $out/share/icons/hicolor/scalable/apps/uk.nandi.sleek.svg
                for size in 16 24 32 48 64 128 256 512; do
                  install -Dm644 ${./assets/icons}/uk.nandi.sleek-$size.png \
                    $out/share/icons/hicolor/''${size}x''${size}/apps/uk.nandi.sleek.png
                done
                # Legacy pixmap lookup (some menus only check pixmaps/).
                install -Dm644 ${./assets/icons}/uk.nandi.sleek-256.png \
                  $out/share/pixmaps/uk.nandi.sleek.png

                # AppStream metadata (Flatpak / software centers).
                install -Dm644 ${./assets/uk.nandi.sleek.metainfo.xml} \
                  $out/share/metainfo/uk.nandi.sleek.metainfo.xml
              '';

              # copyDesktopItems runs as a postInstallHook *after* the postInstall
              # body above, so the .desktop file is not present yet during
              # postInstall. Rewrite Icon= here (preFixup) once it has been copied.
              # Absolute Icon= path is reliable across icon themes (Papirus, etc.)
              # and when the launcher does not merge nix-profile into the theme path
              # — bare `Icon=uk.nandi.sleek` name lookup often fails in that case.
              preFixup = ''
                if [ -f $out/share/applications/uk.nandi.sleek.desktop ]; then
                  substituteInPlace $out/share/applications/uk.nandi.sleek.desktop \
                    --replace-fail 'Icon=uk.nandi.sleek' \
                    "Icon=$out/share/icons/hicolor/256x256/apps/uk.nandi.sleek.png"
                else
                  echo "error: uk.nandi.sleek.desktop missing; copyDesktopItems did not run" >&2
                  exit 1
                fi

                # Drop rustc/libclang store paths leaked into the binary/wrapper
                # (std panic locations embed rust-src paths even after strip).
                find "$out" -type f -exec remove-references-to \
                  -t ${rustBuild} \
                  -t ${pkgs.llvmPackages.libclang} \
                  -t ${pkgs.llvmPackages.libclang.lib} \
                  {} +
              '';

              meta = with pkgs.lib; {
                description = "Sleek — desktop freeq client (egui/Vidya)";
                homepage = "https://github.com/codegod100/sleek";
                license = licenses.mit;
                mainProgram = "sleek";
                platforms = platforms.linux;
              };
            }
            // (v4l2BindgenEnv pkgs)
          );

          # Distributable Flatpak bundle of packages.sleek (no Nix required to install).
          #   nix build .#flatpak
          #   flatpak install --user ./result/uk.nandi.sleek.flatpak
          #   flatpak run uk.nandi.sleek
          sleek-flatpak = nix2flatpak.lib.${system}.mkFlatpak {
            appId = "uk.nandi.sleek";
            appName = "Sleek";
            developer = "nandi";
            package = sleek-host;
            # GNOME Platform indexes ship with nix2flatpak; includes Freedesktop base.
            runtime = "org.gnome.Platform/49";
            command = "sleek";
            appdata = ./assets/uk.nandi.sleek.metainfo.xml;
            desktopFile = ./assets/uk.nandi.sleek.desktop;
            permissions = {
              share = [
                "network"
                "ipc"
              ];
              sockets = [
                "fallback-x11"
                "wayland"
                "pulseaudio"
              ];
              # dri = GL; all = camera / mic for freeq AV calls.
              devices = [
                "dri"
                "all"
              ];
              filesystems = [
                "xdg-run/pipewire-0"
                "xdg-download"
              ];
              talk-names = [
                "org.freedesktop.Notifications"
                "org.freedesktop.portal.Desktop"
              ];
            };
            # nixpkgs unstable vs GNOME 49 runtime — ABI check is advisory here.
            skipAbiChecks = true;
          };

          # Minimal Android SDK + NDK for cargo-apk (phone / aarch64 APK).
          androidComposition = pkgs.androidenv.composeAndroidPackages {
            platformVersions = [ "34" ];
            buildToolsVersions = [ "34.0.0" ];
            includeNDK = true;
            includeEmulator = false;
            includeSystemImages = false;
          };

          androidSdk = androidComposition.androidsdk;
          androidSdkRoot = "${androidSdk}/libexec/android-sdk";

          sleek-android = pkgs.stdenv.mkDerivation {
            pname = "sleek-android";
            version = "0.1.0";
            src = srcTree;

            cargoRoot = "sleek/android";
            cargoDeps = rustPlatformAndroid.importCargoLock {
              lockFile = ./android/Cargo.lock;
              allowBuiltinFetchGit = true;
            };

            nativeBuildInputs = [
              rustAndroid
              pkgs.cargo-apk
              pkgs.jdk17_headless
              pkgs.python3
              rustPlatformAndroid.cargoSetupHook
            ];

            # Keep the build deterministic: no host ~/.android or ambient SDK.
            strictDeps = true;
            dontUseCmakeConfigure = true;
            # Output is a plain APK — toolchain must not leak into the closure.
            disallowedReferences = [ rustAndroid ];

            ANDROID_HOME = androidSdkRoot;
            ANDROID_SDK_ROOT = androidSdkRoot;
            # androidsdk composition exposes the primary NDK as ndk-bundle.
            ANDROID_NDK_HOME = "${androidSdkRoot}/ndk-bundle";
            ANDROID_NDK_ROOT = "${androidSdkRoot}/ndk-bundle";

            buildPhase = ''
              runHook preBuild

              export HOME="$TMPDIR/home"
              mkdir -p "$HOME/.android"

              # cargo-apk --release requires [package.metadata.android.signing.release].
              # Use the committed CI keystore so every build shares one signature
              # (ephemeral keys break adb/phone upgrades with "App not installed").
              keystore="$(pwd)/sleek/android/ci.keystore"
              [[ -f "$keystore" ]] || {
                echo "missing CI keystore at $keystore" >&2
                exit 1
              }

              ndk="$ANDROID_NDK_HOME"
              if [[ ! -d "$ndk" ]]; then
                # Fallback: first versioned NDK under sdk/ndk/
                ndk="$(echo "$ANDROID_HOME"/ndk/* | awk '{print $1}')"
                export ANDROID_NDK_HOME="$ndk"
                export ANDROID_NDK_ROOT="$ndk"
              fi
              [[ -d "$ndk" ]] || {
                echo "Android NDK not found under $ANDROID_HOME" >&2
                ls -la "$ANDROID_HOME" >&2 || true
                exit 1
              }

              prebuilt=""
              for host in linux-x86_64 linux-aarch64; do
                if [[ -d "$ndk/toolchains/llvm/prebuilt/$host/bin" ]]; then
                  prebuilt="$ndk/toolchains/llvm/prebuilt/$host/bin"
                  break
                fi
              done
              [[ -n "$prebuilt" ]] || {
                echo "NDK llvm prebuilt toolchain not found under $ndk" >&2
                exit 1
              }
              export PATH="$prebuilt:$PATH"

              export CC_aarch64_linux_android="aarch64-linux-android${androidApiLevel}-clang"
              export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$CC_aarch64_linux_android"
              export AR_aarch64_linux_android=llvm-ar
              export CARGO_TARGET_AARCH64_LINUX_ANDROID_AR=llvm-ar

              echo "cargo apk build --release --target ${androidTarget} -p sleek" >&2
              echo "  ANDROID_HOME=$ANDROID_HOME" >&2
              echo "  ANDROID_NDK_HOME=$ANDROID_NDK_HOME" >&2
              echo "  linker=$CC_aarch64_linux_android" >&2

              pushd sleek/android >/dev/null
              # cargo-apk rejects multi-member workspaces unless a package is selected (-p).
              # --release: optimized, no debuginfo — APK stays installable size.
              # Inject release signing before [patch] (path is absolute; not committed).
              if ! grep -q 'signing.release' Cargo.toml; then
                python3 - "$keystore" <<'PY'
import pathlib, sys
keystore = sys.argv[1]
path = pathlib.Path("Cargo.toml")
text = path.read_text()
block = f"""
[package.metadata.android.signing.release]
path = "{keystore}"
keystore_password = "android"
key_alias = "androiddebugkey"
key_password = "android"
"""
marker = "[patch.crates-io]"
if marker in text:
    text = text.replace(marker, block + "\n" + marker, 1)
else:
    text = text + block
path.write_text(text)
PY
              fi
              cargo apk build --release --target ${androidTarget} -p sleek --lib

              # Inject SleekActivity as classes.dex so freeq:// OAuth deep links work.
              # Prefer the final signed package; never inject into *-unaligned leftovers.
              apk=""
              for cand in \
                target/release/apk/sleek.apk \
                target/sleek.apk \
                target/release/apk/sleek-release.apk; do
                if [[ -f "$cand" ]]; then
                  apk="$cand"
                  break
                fi
              done
              if [[ -z "''${apk:-}" ]]; then
                apk="$(find target -type f -path '*/release/apk/*.apk' ! -name '*-unaligned.apk' 2>/dev/null | head -1 || true)"
              fi
              [[ -n "''${apk:-}" && -f "$apk" ]] || {
                echo "APK not found for dex inject under sleek/android/target" >&2
                exit 1
              }
              export SLEEK_KEYSTORE="$keystore"
              export SLEEK_KEYSTORE_PASSWORD=android
              export SLEEK_KEY_ALIAS=androiddebugkey
              export SLEEK_KEY_PASSWORD=android
              bash scripts/inject-activity-dex.sh "$apk" src/assets/sleek_activity.dex

              popd >/dev/null

              runHook postBuild
            '';

            installPhase = ''
              runHook preInstall
              mkdir -p $out
              # Prefer the final signed package (same rules as deploy-android.sh).
              apk=""
              for cand in \
                sleek/android/target/release/apk/sleek.apk \
                sleek/android/target/sleek.apk \
                sleek/android/target/release/apk/sleek-release.apk; do
                if [[ -f "$cand" ]]; then
                  apk="$cand"
                  break
                fi
              done
              if [[ -z "''${apk:-}" ]]; then
                apk="$(find sleek/android/target -type f -path '*/release/apk/*.apk' ! -name '*-unaligned.apk' 2>/dev/null | head -1 || true)"
              fi
              if [[ -z "''${apk:-}" ]]; then
                apk="$(find sleek/android/target -type f \( -name 'sleek.apk' -o -name '*-release.apk' \) ! -name '*-unaligned.apk' 2>/dev/null | head -1 || true)"
              fi
              [[ -n "''${apk:-}" && -f "$apk" ]] || {
                echo "APK not found under sleek/android/target" >&2
                find sleek/android/target -name '*.apk' -o -name '*.so' 2>/dev/null | head -50 >&2 || true
                exit 1
              }
              cp "$apk" $out/sleek.apk
              apksigner="$(echo "$ANDROID_HOME"/build-tools/*/apksigner | awk '{print $NF}')"
              "$apksigner" verify --verbose "$out/sleek.apk"
              "$apksigner" verify --print-certs "$out/sleek.apk" | grep -q 'CN=Sleek CI'
              # Convenience symlink for tools that look for a generic name.
              ln -s sleek.apk $out/app.apk
              cat > $out/metadata.txt <<EOF
              package=uk.nandi.sleek
              activity=uk.nandi.sleek.SleekActivity
              target=${androidTarget}
              profile=release
              EOF
              runHook postInstall
            '';

            meta = with pkgs.lib; {
              description = "Sleek — Android APK (aarch64 / arm64-v8a)";
              homepage = "https://github.com/codegod100/sleek";
              license = licenses.mit;
              platforms = platforms.linux;
            };
          };

          install-android = pkgs.writeShellApplication {
            name = "install-android";
            runtimeInputs = [ pkgs.android-tools ];
            text = ''
              set -euo pipefail
              APK="${sleek-android}/sleek.apk"
              PKG="uk.nandi.sleek"
              ACTIVITY="$PKG/uk.nandi.sleek.SleekActivity"

              if [[ ! -f "$APK" ]]; then
                echo "error: APK missing at $APK (build .#android first?)" >&2
                exit 1
              fi

              echo "waiting for adb device…" >&2
              # adb get-state fails with "more than one device/emulator" when several
              # are attached (phone + Waydroid). Pick USB phone unless ANDROID_SERIAL set.
              mapfile -t _serials < <(adb devices 2>/dev/null | tr -d '\r' | awk 'NR>1 && $2=="device"{print $1}')
              if [[ ''${#_serials[@]} -eq 0 ]]; then
                echo "No adb device in 'device' state." >&2
                echo "Enable USB debugging and authorize this computer, then re-run." >&2
                adb devices -l >&2 || true
                exit 1
              fi
              if [[ -n "''${ANDROID_SERIAL:-}" ]]; then
                :
              elif [[ ''${#_serials[@]} -eq 1 ]]; then
                export ANDROID_SERIAL="''${_serials[0]}"
              else
                _usb=()
                for s in "''${_serials[@]}"; do
                  [[ "$s" == *:* ]] || _usb+=("$s")
                done
                if [[ ''${#_usb[@]} -eq 1 ]]; then
                  export ANDROID_SERIAL="''${_usb[0]}"
                  echo "multiple adb devices; using USB $ANDROID_SERIAL (set ANDROID_SERIAL to override)" >&2
                else
                  echo "multiple adb devices — set ANDROID_SERIAL to one of:" >&2
                  printf '  %s\n' "''${_serials[@]}" >&2
                  adb devices -l >&2 || true
                  exit 1
                fi
              fi
              echo "adb device: $ANDROID_SERIAL" >&2

              echo "install -r $APK" >&2
              adb install -r "$APK"

              if [[ "''${1:-}" == "--launch" || "''${1:-}" == "launch" || "''${LAUNCH:-}" == "1" ]]; then
                echo "launch $ACTIVITY" >&2
                adb shell am start -n "$ACTIVITY"
              fi

              echo "ok: installed $PKG" >&2
            '';
          };

          # Iterative phone deploy: flake toolchain + in-tree cargo-apk + adb.
          # Unlike .#android / .#install-android (pure Nix store APK), this builds
          # against the working tree so incremental cargo caches apply.
          # Run from the sleek repo root (needs sibling ../../vidya + ../../freeq).
          deploy-android = pkgs.writeShellApplication {
            name = "deploy-android";
            runtimeInputs = [
              rustAndroid
              pkgs.cargo-apk
              pkgs.android-tools
              pkgs.jdk17_headless
              pkgs.python3
              pkgs.findutils
              pkgs.gawk
              pkgs.gnugrep
              pkgs.gnused
              pkgs.coreutils
              pkgs.bash
            ];
            text = ''
              set -euo pipefail

              # Hermetic SDK/NDK from the flake (override with env if you prefer host tools).
              export ANDROID_HOME="''${ANDROID_HOME:-${androidSdkRoot}}"
              export ANDROID_SDK_ROOT="''${ANDROID_SDK_ROOT:-$ANDROID_HOME}"
              export ANDROID_NDK_HOME="''${ANDROID_NDK_HOME:-${androidSdkRoot}/ndk-bundle}"
              export ANDROID_NDK_ROOT="''${ANDROID_NDK_ROOT:-$ANDROID_NDK_HOME}"
              if [[ ! -d "$ANDROID_NDK_HOME" ]]; then
                # Fallback: first versioned NDK under sdk/ndk/
                ndk="$(echo "$ANDROID_HOME"/ndk/* | awk '{print $1}')"
                if [[ -n "''${ndk:-}" && -d "$ndk" ]]; then
                  export ANDROID_NDK_HOME="$ndk"
                  export ANDROID_NDK_ROOT="$ndk"
                fi
              fi

              # Prefer live working-tree script (edits without re-eval); else store copy.
              script=""
              if [[ -f ./scripts/deploy-android.sh ]]; then
                script=./scripts/deploy-android.sh
              elif [[ -f ./android/Cargo.toml && -f ./scripts/deploy-android.sh ]]; then
                script=./scripts/deploy-android.sh
              else
                # Walk up a few levels from CWD looking for the sleek root.
                d="$PWD"
                for _ in 1 2 3 4 5; do
                  if [[ -f "$d/android/Cargo.toml" && -f "$d/scripts/deploy-android.sh" ]]; then
                    script="$d/scripts/deploy-android.sh"
                    break
                  fi
                  d="$(dirname "$d")"
                done
              fi
              if [[ -z "''${script:-}" ]]; then
                script="${./scripts/deploy-android.sh}"
                if [[ ! -f ./android/Cargo.toml ]]; then
                  echo "error: run from the sleek repo root (need android/Cargo.toml + path deps)" >&2
                  echo "  cd /path/to/sleek && nix run .#deploy-android" >&2
                  exit 1
                fi
              fi

              exec bash "$script" "$@"
            '';
          };

          # Iterative Waydroid deploy: flake toolchain + in-tree cargo-apk (x86_64)
          # + adb install/launch. Opens full Android UI by default (portrait phone).
          # Run from the sleek repo root (needs sibling ../../vidya + ../../freeq).
          # Host `waydroid` CLI must be on PATH (system/NixOS package).
          #
          # Display defaults match a tall phone window; override via env when
          # calling `nix run .#waydroid -- …`.
          # `.#waydroid` = debug; `.#waydroid-release` = cargo apk --release.
          waydroidDisplay = {
            width = "1080";
            height = "2400";
            lcdDensity = "420";
          };
          mkWaydroidApp =
            {
              name,
              release ? false,
            }:
            pkgs.writeShellApplication {
              inherit name;
              runtimeInputs = [
                rustAndroid
                pkgs.cargo-apk
                pkgs.android-tools
                pkgs.jdk17_headless
                pkgs.python3
                pkgs.findutils
                pkgs.gawk
                pkgs.gnugrep
                pkgs.gnused
                pkgs.coreutils
                pkgs.bash
                pkgs.procps
              ];
              text = ''
                set -euo pipefail

                # Hermetic SDK/NDK from the flake (override with env if you prefer host tools).
                export ANDROID_HOME="''${ANDROID_HOME:-${androidSdkRoot}}"
                export ANDROID_SDK_ROOT="''${ANDROID_SDK_ROOT:-$ANDROID_HOME}"
                export ANDROID_NDK_HOME="''${ANDROID_NDK_HOME:-${androidSdkRoot}/ndk-bundle}"
                export ANDROID_NDK_ROOT="''${ANDROID_NDK_ROOT:-$ANDROID_NDK_HOME}"
                if [[ ! -d "$ANDROID_NDK_HOME" ]]; then
                  ndk="$(echo "$ANDROID_HOME"/ndk/* | awk '{print $1}')"
                  if [[ -n "''${ndk:-}" && -d "$ndk" ]]; then
                    export ANDROID_NDK_HOME="$ndk"
                    export ANDROID_NDK_ROOT="$ndk"
                  fi
                fi

                # Waydroid window / density (portrait phone). Env overrides win.
                export SLEEK_WAYDROID_WIDTH="''${SLEEK_WAYDROID_WIDTH:-${waydroidDisplay.width}}"
                export SLEEK_WAYDROID_HEIGHT="''${SLEEK_WAYDROID_HEIGHT:-${waydroidDisplay.height}}"
                export SLEEK_WAYDROID_LCD_DENSITY="''${SLEEK_WAYDROID_LCD_DENSITY:-${waydroidDisplay.lcdDensity}}"
                # 1 = open `waydroid show-full-ui` before launch (needed to see the app).
                export SLEEK_WAYDROID_SHOW_UI="''${SLEEK_WAYDROID_SHOW_UI:-1}"
                # 1 = try `waydroid session start` if session is stopped.
                export SLEEK_WAYDROID_START_SESSION="''${SLEEK_WAYDROID_START_SESSION:-1}"
                # Release profile (optimized + signed). Env can still force either way.
                export SLEEK_WAYDROID_RELEASE="''${SLEEK_WAYDROID_RELEASE:-${if release then "1" else "0"}}"

                # Prefer live working-tree script (edits without re-eval); else store copy.
                script=""
                if [[ -f ./scripts/waydroid.sh ]]; then
                  script=./scripts/waydroid.sh
                elif [[ -f ./android/Cargo.toml && -f ./scripts/waydroid.sh ]]; then
                  script=./scripts/waydroid.sh
                else
                  d="$PWD"
                  for _ in 1 2 3 4 5; do
                    if [[ -f "$d/android/Cargo.toml" && -f "$d/scripts/waydroid.sh" ]]; then
                      script="$d/scripts/waydroid.sh"
                      break
                    fi
                    d="$(dirname "$d")"
                  done
                fi
                if [[ -z "''${script:-}" ]]; then
                  script="${./scripts/waydroid.sh}"
                  if [[ ! -f ./android/Cargo.toml ]]; then
                    echo "error: run from the sleek repo root (need android/Cargo.toml + path deps)" >&2
                    echo "  cd /path/to/sleek && nix run .#${name}" >&2
                    exit 1
                  fi
                fi

                exec bash "$script" "$@"
              '';
            };
          run-waydroid = mkWaydroidApp {
            name = "waydroid";
            release = false;
          };
          run-waydroid-release = mkWaydroidApp {
            name = "waydroid-release";
            release = true;
          };

          # Desktop host app entry: flake toolchain + in-tree `cargo run --release`.
          # Used by apps.default / apps.host (`nix run`, `nix run .#host`).
          # Hermetic store binary remains packages.sleek / apps.sleek.
          # Run from the sleek repo root (needs sibling ../../vidya + ../../freeq).
          run-host =
            let
              sleekLibPath = pkgs.lib.makeLibraryPath libs;
              # pkg-config needs .dev outputs for headers + .pc files (runtime libs only).
              pkgConfigPath = pkgs.lib.makeSearchPath "lib/pkgconfig" (
                map (p: p.dev or p) (
                  libs
                  ++ [
                    pkgs.openssl
                  ]
                )
              );
              bindgen = v4l2BindgenEnv pkgs;
            in
            pkgs.writeShellApplication {
              name = "run-host";
              runtimeInputs = [
                rust
                pkgs.pkg-config
                pkgs.llvmPackages.libclang
                # .cargo/config.toml requests -fuse-ld=mold (default `cc` driver).
                pkgs.mold
              ];
              text = ''
                set -euo pipefail

                export OPENSSL_NO_VENDOR=1
                export PKG_CONFIG_PATH="${pkgConfigPath}''${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
                export LD_LIBRARY_PATH="${sleekLibPath}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
                export LIBCLANG_PATH="${bindgen.LIBCLANG_PATH}"
                export SLEEK_LIBCLANG_PATH="${bindgen.SLEEK_LIBCLANG_PATH}"
                export BINDGEN_EXTRA_CLANG_ARGS="${bindgen.BINDGEN_EXTRA_CLANG_ARGS}"
                export V4L2R_VIDEODEV2_H_PATH="${bindgen.V4L2R_VIDEODEV2_H_PATH}"

                # Codespace / desktop-lite: GUI opens on the VNC X display.
                if [[ -n "''${SLEEK_CODESPACE:-}" ]]; then
                  export DISPLAY="''${DISPLAY:-:1}"
                  export LIBGL_ALWAYS_SOFTWARE="''${LIBGL_ALWAYS_SOFTWARE:-1}"
                elif [[ -z "''${DISPLAY:-}" && -z "''${WAYLAND_DISPLAY:-}" ]]; then
                  if [[ -S /tmp/.X11-unix/X1 ]]; then
                    export DISPLAY=:1
                  elif [[ -S /tmp/.X11-unix/X0 ]]; then
                    export DISPLAY=:0
                  fi
                fi

                # Find sleek repo root (host/Cargo.toml + android path dep).
                root=""
                if [[ -f ./host/Cargo.toml && -f ./android/Cargo.toml ]]; then
                  root="$PWD"
                else
                  d="$PWD"
                  for _ in 1 2 3 4 5; do
                    if [[ -f "$d/host/Cargo.toml" && -f "$d/android/Cargo.toml" ]]; then
                      root="$d"
                      break
                    fi
                    d="$(dirname "$d")"
                  done
                fi
                if [[ -z "''${root:-}" ]]; then
                  echo "error: run from the sleek repo root (need host/Cargo.toml + path deps)" >&2
                  echo "  cd /path/to/sleek && nix run .#host" >&2
                  exit 1
                fi
                cd "$root"

                exec cargo run --release --manifest-path host/Cargo.toml "$@"
              '';
            };
        in
        {
          default = sleek-host;
          sleek = sleek-host;
          inherit sleek-host;
          flatpak = sleek-flatpak;
          inherit sleek-flatpak;
          android = sleek-android;
          inherit sleek-android;
          inherit install-android;
          inherit deploy-android;
          waydroid = run-waydroid;
          inherit run-waydroid;
          waydroid-release = run-waydroid-release;
          inherit run-waydroid-release;
          inherit run-host;
        }
      );

      apps = forAllSystems (system: {
        # Desktop host via in-tree `cargo run --release` (iterative; needs repo root + path deps).
        # Hermetic store binary is packages.default / packages.sleek (`nix build .#sleek`).
        default = {
          type = "app";
          program = "${self.packages.${system}.run-host}/bin/run-host";
        };
        host = {
          type = "app";
          program = "${self.packages.${system}.run-host}/bin/run-host";
        };
        # Pure store binary (no cargo; from packages.sleek).
        sleek = {
          type = "app";
          program = "${self.packages.${system}.sleek}/bin/sleek";
        };
        install-android = {
          type = "app";
          program = "${self.packages.${system}.install-android}/bin/install-android";
        };
        deploy-android = {
          type = "app";
          program = "${self.packages.${system}.deploy-android}/bin/deploy-android";
        };
        # In-tree cargo-apk (x86_64) → Waydroid adb install + launch (debug).
        waydroid = {
          type = "app";
          program = "${self.packages.${system}.waydroid}/bin/waydroid";
        };
        # Same as .#waydroid but cargo apk --release (signed local keystore).
        waydroid-release = {
          type = "app";
          program = "${self.packages.${system}.waydroid-release}/bin/waydroid-release";
        };
      });

      devShells = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          libs = eguiLibs pkgs;
          buildLibs = eguiBuildLibs pkgs;
          # Runtime libs for the egui host only — do NOT export as ambient
          # LD_LIBRARY_PATH. On Codespaces/Ubuntu, that makes system
          # git-remote-https load nix openssl/glibc and die with
          # GLIBC_ABI_DT_X86_64_PLT.
          sleekLibPath = pkgs.lib.makeLibraryPath libs;
          cliTools = with pkgs; [
            git
            openssh
            curl
            cacert
          ];
          rust = pkgs.rust-bin.stable.latest.default.override {
            extensions = [
              "rust-src"
              "rustfmt"
              "clippy"
            ];
            targets = [
              "x86_64-linux-android"
              "aarch64-linux-android"
            ];
          };
        in
        {
          default = pkgs.mkShell (
            {
              packages = [
                rust
                pkgs.just
                pkgs.android-tools
                pkgs.cargo-apk
                pkgs.cachix
                pkgs.openssl
                # Faster linking for in-tree .cargo/config.toml (-fuse-ld=mold).
                pkgs.mold
              ]
              ++ buildLibs
              ++ cliTools;
              buildInputs = libs;
              OPENSSL_NO_VENDOR = "1";
              PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";
              # Available to justfile / scripts; not injected into every process.
              SLEEK_LD_LIBRARY_PATH = sleekLibPath;
              shellHook = ''
                # Marker for scripts/enter + codespace-env.sh (avoid nested re-exec).
                export SLEEK_NIX_SHELL=1
                export SLEEK_LD_LIBRARY_PATH="${sleekLibPath}"
                # Never leave a stale ambient LD_LIBRARY_PATH from an older shell
                # or direnv that pointed at nix openssl (breaks system git).
                if [[ -n "''${LD_LIBRARY_PATH:-}" ]]; then
                  case ":''${LD_LIBRARY_PATH}:" in
                    *"/nix/store/"*) unset LD_LIBRARY_PATH ;;
                  esac
                fi
                # After NDK / cargo PATH prepends, keep nix git/curl/ssh first so
                # Codespaces /usr/local/git is never used with mixed loaders.
                export PATH="$HOME/.cargo/bin:$PATH"
                export ANDROID_NDK_HOME="''${ANDROID_NDK_HOME:-$HOME/.local/share/android-ndk-r29}"
                export ANDROID_NDK_ROOT="$ANDROID_NDK_HOME"
                export ANDROID_HOME="''${ANDROID_HOME:-$HOME/.local/share/android-sdk}"
                export PATH="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin:$ANDROID_HOME/platform-tools:$PATH"
                export PATH="${pkgs.lib.makeBinPath cliTools}:$PATH"
                unset GIT_EXEC_PATH
                export CC_x86_64_linux_android="''${CC_x86_64_linux_android:-x86_64-linux-android28-clang}"
                export CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER="$CC_x86_64_linux_android"
                export AR_x86_64_linux_android="''${AR_x86_64_linux_android:-llvm-ar}"
                export CC_aarch64_linux_android="''${CC_aarch64_linux_android:-aarch64-linux-android28-clang}"
                export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$CC_aarch64_linux_android"
                export AR_aarch64_linux_android="''${AR_aarch64_linux_android:-llvm-ar}"
                export SSL_CERT_FILE="''${SSL_CERT_FILE:-${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt}"
                export NIX_SSL_CERT_FILE="''${NIX_SSL_CERT_FILE:-$SSL_CERT_FILE}"
                # Host bindgen (v4l2 camera) — re-assert after NDK path munging so
                # clang-sys never loads NDK libclang (missing libz on host).
                export LIBCLANG_PATH="${(v4l2BindgenEnv pkgs).LIBCLANG_PATH}"
                export SLEEK_LIBCLANG_PATH="$LIBCLANG_PATH"
                export BINDGEN_EXTRA_CLANG_ARGS="${(v4l2BindgenEnv pkgs).BINDGEN_EXTRA_CLANG_ARGS}"
                export V4L2R_VIDEODEV2_H_PATH="${(v4l2BindgenEnv pkgs).V4L2R_VIDEODEV2_H_PATH}"
                if [[ -z "''${SLEEK_QUIET_SHELL:-}" ]]; then
                  echo "sleek — nix run | nix run .#host | nix run .#waydroid | nix build .#android | nix build .#flatpak"
                fi
              '';
            }
            // (v4l2BindgenEnv pkgs)
          );
        }
      );
    };
}
