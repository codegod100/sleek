{
  description = "Sleek — mobile freeq client (Vidya + freeq-sdk)";

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
          rustPlatform = pkgs.makeRustPlatform {
            cargo = rust;
            rustc = rust;
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
        in
        {
          default = sleek-host;
          sleek = sleek-host;
          inherit sleek-host;
        }
      );

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/sleek";
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
            targets = [ "x86_64-linux-android" ];
          };
        in
        {
          default = pkgs.mkShell {
            packages = [
              rust
              pkgs.just
              pkgs.android-tools
              pkgs.cargo-apk
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
              export SSL_CERT_FILE="''${SSL_CERT_FILE:-${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt}"
              export NIX_SSL_CERT_FILE="''${NIX_SSL_CERT_FILE:-$SSL_CERT_FILE}"
              if [[ -z "''${SLEEK_QUIET_SHELL:-}" ]]; then
                echo "sleek — just host | just waydroid | just lib | nix build | ./scripts/enter"
              fi
            '';
          };
        }
      );
    };
}
