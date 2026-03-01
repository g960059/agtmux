# This file is a template. The actual formula lives at:
# https://github.com/g960059/homebrew-agtmux/blob/main/Formula/agtmux.rb
# It is automatically updated by the release workflow via cargo-dist.
class Agtmux < Formula
  desc "Real-time AI agent state monitor for tmux"
  homepage "https://github.com/g960059/agtmux"
  version "0.1.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/g960059/agtmux/releases/download/v#{version}/agtmux-#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_FILL_BY_RELEASE_WORKFLOW"
    end
    on_intel do
      url "https://github.com/g960059/agtmux/releases/download/v#{version}/agtmux-#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_FILL_BY_RELEASE_WORKFLOW"
    end
  end

  depends_on "tmux"

  def install
    bin.install "agtmux"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/agtmux --version")
  end
end
