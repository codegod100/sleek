{ pkgs, lib, ... }:
{
  packages = with pkgs; [
    rustc
    cargo
    rustfmt
    clippy
    just
    pkg-config
    openssl
    mold
    llvmPackages.libclang
    pipewire
    alsa-lib
    gtk4
    libx11
    libxkbcommon
    libGL
    libxcursor
    libxi
    libxrandr
    mesa
    git
    curl
    cacert
  ];

  env.OPENSSL_NO_VENDOR = "1";
  env.PKG_CONFIG_PATH = lib.makeSearchPath "lib/pkgconfig" [ pkgs.openssl.dev pkgs.pipewire.dev pkgs.alsa-lib.dev pkgs.gtk4.dev ];
  env.SLEEK_BROWSER_PROFILE = "${builtins.getEnv "TMPDIR"}/sleek-chromium";
  env.LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
  env.SLEEK_LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
  env.BINDGEN_EXTRA_CLANG_ARGS = "-I${pkgs.llvmPackages.libclang.lib}/lib/clang/${pkgs.llvmPackages.libclang.version}/include";
  env.SLEEK_LD_LIBRARY_PATH = lib.makeLibraryPath [
    pkgs.libx11 pkgs.libxkbcommon pkgs.libGL pkgs.libxcursor pkgs.libxi
    pkgs.libxrandr pkgs.mesa pkgs.pipewire pkgs.alsa-lib pkgs.gtk4
  ];

  enterShell = ''
    export PATH="$HOME/.cargo/bin:$PATH"
    # Bluefin's prompt probes OSC 11 (terminal background color). In this
    # terminal the response leaks as literal control characters; disable the
    # probe while inside devenv without changing the host shell.
    export NO_COLOR="''${NO_COLOR:-1}"
    export STARSHIP_LOG="''${STARSHIP_LOG:-error}"
    # This VM advertises Wayland but has no usable Wayland client library.
    # Force egui/winit onto the available X11 display instead.
    export DISPLAY="''${DISPLAY:-:0}"
    unset WAYLAND_DISPLAY
    export WINIT_UNIX_BACKEND="x11"
    export LIBGL_ALWAYS_SOFTWARE="''${LIBGL_ALWAYS_SOFTWARE:-1}"
    export LD_LIBRARY_PATH="''${SLEEK_LD_LIBRARY_PATH}:''${LD_LIBRARY_PATH:-}"
    echo "Sleek devenv — run: just host"
  '';
}
