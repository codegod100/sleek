class Sleek < Formula
  desc "Mobile freeq client (egui desktop host)"
  homepage "https://tangled.org/nandi.uk/sleek"
  license "MIT"
  url "https://github.com/codegod100/sleek/archive/refs/tags/v0.1.0.tar.gz"
  version "0.1.0"
  sha256 "0501b04085870ae1a34119babaf0f093f3b229a65d5fb0a8b00f202bf60e603f"

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
