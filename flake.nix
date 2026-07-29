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
    vidya = {
      url = "git+https://tangled.org/nandi.uk/vidya";
      flake = false;
    };
    freeq = {
      url = "github:codegod100/freeq";
      flake = false;
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      vidya,
      freeq,
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

      eguiLibs =
        pkgs:
        with pkgs;
        [
          libxkbcommon
          libGL
          vulkan-loader
          pkg-config
          openssl
        ]
        ++ lib.optionals stdenv.hostPlatform.isLinux [
          wayland
          libx11
          libxcursor
          libxi
          libxrandr
        ];

      # Layout expected by android/Cargo.toml path deps:
      #   parent/sleek/{android,host}
      #   parent/vidya
      #   parent/freeq/freeq-sdk
      sleekSrcTree =
        pkgs:
        pkgs.runCommand "sleek-src-tree"
          {
            # Avoid .git / target noise from the working tree.
            nativeBuildInputs = [ pkgs.rsync ];
          }
          ''
            mkdir -p $out/{sleek,vidya,freeq}
            # cleanSource drops .git; keep Cargo.lock under host/
            cp -a ${pkgs.lib.cleanSource ./.}/. $out/sleek/
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
          rust = pkgs.rust-bin.stable.latest.default.override {
            extensions = [
              "rust-src"
              "rustfmt"
              "clippy"
            ];
          };
          rustAndroid = pkgs.rust-bin.stable.latest.default.override {
            extensions = [
              "rust-src"
              "rustfmt"
              "clippy"
            ];
            targets = [ androidTarget ];
          };
          rustPlatform = pkgs.makeRustPlatform {
            cargo = rust;
            rustc = rust;
          };
          rustPlatformAndroid = pkgs.makeRustPlatform {
            cargo = rustAndroid;
            rustc = rustAndroid;
          };
          srcTree = sleekSrcTree pkgs;

          sleek-host = rustPlatform.buildRustPackage {
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
              pkg-config
              makeWrapper
            ];
            buildInputs = libs;

            OPENSSL_NO_VENDOR = "1";
            PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";

            doCheck = false;

            # Binary is named `sleek` (see host/Cargo.toml [[bin]]).
            postInstall = ''
              wrapProgram $out/bin/sleek \
                --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath libs}
            '';

            meta = with pkgs.lib; {
              description = "Sleek — desktop freeq client (egui/Vidya)";
              homepage = "https://github.com/codegod100/sleek";
              license = licenses.mit;
              mainProgram = "sleek";
              platforms = platforms.linux;
            };
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
              rustPlatformAndroid.cargoSetupHook
            ];

            # Keep the build deterministic: no host ~/.android or ambient SDK.
            strictDeps = true;
            dontUseCmakeConfigure = true;

            ANDROID_HOME = androidSdkRoot;
            ANDROID_SDK_ROOT = androidSdkRoot;
            # androidsdk composition exposes the primary NDK as ndk-bundle.
            ANDROID_NDK_HOME = "${androidSdkRoot}/ndk-bundle";
            ANDROID_NDK_ROOT = "${androidSdkRoot}/ndk-bundle";

            buildPhase = ''
              runHook preBuild

              export HOME="$TMPDIR/home"
              mkdir -p "$HOME/.android"

              # cargo-apk debug profile auto-creates $HOME/.android/debug.keystore
              # when missing; ensure parent dir exists and HOME is writable.

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

              echo "cargo apk build --target ${androidTarget} -p sleek" >&2
              echo "  ANDROID_HOME=$ANDROID_HOME" >&2
              echo "  ANDROID_NDK_HOME=$ANDROID_NDK_HOME" >&2
              echo "  linker=$CC_aarch64_linux_android" >&2

              pushd sleek/android >/dev/null
              # cargo-apk rejects workspaces unless a package is selected (-p).
              cargo apk build --target ${androidTarget} -p sleek --lib
              popd >/dev/null

              runHook postBuild
            '';

            installPhase = ''
              runHook preInstall
              mkdir -p $out
              apk="$(find sleek/android/target -type f \( -name 'sleek.apk' -o -name '*-debug.apk' \) | head -1 || true)"
              if [[ -z "''${apk:-}" ]]; then
                apk="$(find sleek/android/target -type f -name '*.apk' | head -1 || true)"
              fi
              [[ -n "''${apk:-}" && -f "$apk" ]] || {
                echo "APK not found under sleek/android/target" >&2
                find sleek/android/target -name '*.apk' -o -name '*.so' 2>/dev/null | head -50 >&2 || true
                exit 1
              }
              cp "$apk" $out/sleek.apk
              # Convenience symlink for tools that look for a generic name.
              ln -s sleek.apk $out/app.apk
              cat > $out/metadata.txt <<EOF
              package=uk.nandi.sleek
              activity=android.app.NativeActivity
              target=${androidTarget}
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
              ACTIVITY="$PKG/android.app.NativeActivity"

              if [[ ! -f "$APK" ]]; then
                echo "error: APK missing at $APK (build .#android first?)" >&2
                exit 1
              fi

              echo "waiting for adb device…" >&2
              if ! adb get-state >/dev/null 2>&1; then
                echo "No adb device in 'device' state." >&2
                echo "Enable USB debugging and authorize this computer, then re-run." >&2
                adb devices -l >&2 || true
                exit 1
              fi

              echo "install -r $APK" >&2
              adb install -r "$APK"

              if [[ "''${1:-}" == "--launch" || "''${1:-}" == "launch" || "''${LAUNCH:-}" == "1" ]]; then
                echo "launch $ACTIVITY" >&2
                adb shell am start -n "$ACTIVITY"
              fi

              echo "ok: installed $PKG" >&2
            '';
          };
        in
        {
          default = sleek-host;
          sleek = sleek-host;
          inherit sleek-host;
          android = sleek-android;
          inherit sleek-android;
          inherit install-android;
        }
      );

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/sleek";
        };
        install-android = {
          type = "app";
          program = "${self.packages.${system}.install-android}/bin/install-android";
        };
      });

      devShells = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          libs = eguiLibs pkgs;
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
          default = pkgs.mkShell {
            packages = [
              rust
              pkgs.just
              pkgs.android-tools
              pkgs.cargo-apk
              pkgs.cachix
              pkgs.pkg-config
              pkgs.openssl
            ]
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
              if [[ -z "''${SLEEK_QUIET_SHELL:-}" ]]; then
                echo "sleek — just host | just waydroid | nix build .#android | nix run .#install-android"
              fi
            '';
          };
        }
      );
    };
}
