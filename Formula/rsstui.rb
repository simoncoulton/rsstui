class Rsstui < Formula
  desc "Terminal RSS/Atom reader with inline images"
  homepage "https://github.com/simoncoulton/rsstui"
  version "0.1.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/simoncoulton/rsstui/releases/download/v0.1.0/rsstui-v0.1.0-aarch64-apple-darwin.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
    on_intel do
      url "https://github.com/simoncoulton/rsstui/releases/download/v0.1.0/rsstui-v0.1.0-x86_64-apple-darwin.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/simoncoulton/rsstui/releases/download/v0.1.0/rsstui-v0.1.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
    on_intel do
      url "https://github.com/simoncoulton/rsstui/releases/download/v0.1.0/rsstui-v0.1.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
  end

  def install
    bin.install "rsstui"
  end

  test do
    assert_path_exists bin/"rsstui"
  end
end
