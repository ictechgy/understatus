# Homebrew formula for understatus
# Tap: ictechgy/homebrew-understatus
# Formula path in tap repo: Formula/understatus.rb
#
# This formula builds understatus from source using Cargo (Rust).
# macOS only (Apple Silicon + Intel).

class Understatus < Formula
  desc "Claude Code statusline addon: CPU/memory/session info with responsive pulse glyphs"
  homepage "https://github.com/ictechgy/understatus"

  # URL points to the source tarball for the v0.2.0 release tag.
  url "https://github.com/ictechgy/understatus/archive/refs/tags/v0.2.0.tar.gz"

  # SHA-256 of the v0.2.0 source tarball.
  #   curl -L https://github.com/ictechgy/understatus/archive/refs/tags/v0.2.0.tar.gz \
  #     | shasum -a 256
  sha256 "526ca84237e7023fbb2a07867d17b132540884bb528607111f38cd8f0bc639f4"

  license "MIT"

  # Rust toolchain is required at build time only; not needed at runtime.
  depends_on "rust" => :build

  # understatus uses macOS-only APIs (host_processor_info, sysctl, IOKit FFI).
  depends_on :macos

  def install
    # Build the release binary and install it into #{bin}.
    system "cargo", "install", *std_cargo_args
  end

  test do
    # Verify the binary exists and reports the expected crate name in its version string.
    assert_match "understatus", shell_output("#{bin}/understatus --version")
  end
end
