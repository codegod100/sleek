class Sleek < Formula
  desc "Mobile freeq client — egui desktop host (Wayland/X11)"
  homepage "https://github.com/codegod100/sleek"
  head "https://github.com/codegod100/sleek.git", branch: "main"
  license "MIT"

  depends_on "rust" => :build

  # System libraries required on Linux (install via your distro's package manager):
  #   Wayland: libwayland-dev wayland-protocols libxkbcommon-dev
  #   X11:     libx11-dev libxcursor-dev libxi-dev
  #   Audio:   libpipewire-0.3-dev
  #   Other:   libssl-dev libfontconfig-dev

  def install
    system "cargo", "build", "--release",
           "--manifest-path", "host/Cargo.toml",
           "--target-dir", buildpath/"target"
    bin.install buildpath/"target/release/sleek"
  end

  test do
    assert_predicate bin/"sleek", :exist?
  end
end
