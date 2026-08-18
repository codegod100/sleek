class Sleek < Formula
  desc "Mobile freeq client (egui desktop host)"
  homepage "https://tangled.org/nandi.uk/sleek"
  license "MIT"
  url "https://tangled.org/nandi.uk/sleek/archive/v0.1.0.tar.gz"
  version "0.1.0"
  sha256 "996e59d181615cf098b86fe4a8d5b48a3a6cb3197ee47f23fe3419f6cc17c29f"

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
