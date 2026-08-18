class Sleek < Formula
  desc "Mobile freeq client (egui desktop host)"
  homepage "https://github.com/codegod100/sleek"
  license "MIT"
  head "https://github.com/codegod100/sleek.git", branch: "main"

  depends_on "pkg-config" => :build
  depends_on "rust" => :build

  on_linux do
    depends_on "alsa-lib"
    depends_on "libxkbcommon"
    depends_on "libxi"
    depends_on "libxrandr"
    depends_on "mesa"
    depends_on "openssl@3"
    depends_on "vulkan-loader"
    depends_on "wayland"
  end

  def install
    ENV["OPENSSL_NO_VENDOR"] = "1"
    system "cargo", "install", *std_cargo_args(path: "host")
  end

  test do
    assert_predicate bin/"sleek", :executable?
  end
end
