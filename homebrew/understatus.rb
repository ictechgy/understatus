# Homebrew formula for understatus
# Tap: ictechgy/homebrew-understatus
# Formula path in tap repo: Formula/understatus.rb
#
# This formula builds understatus from source using Cargo (Rust).
# macOS only (Apple Silicon + Intel).

class Understatus < Formula
  desc "Claude Code statusline addon: CPU/memory/session info with responsive pulse glyphs"
  homepage "https://github.com/ictechgy/understatus"

  # URL points to the source tarball for the v0.1.0 release tag.
  url "https://github.com/ictechgy/understatus/archive/refs/tags/v0.1.0.tar.gz"

  # IMPORTANT: Replace this placeholder with the actual SHA-256 of the source tarball
  # AFTER the v0.1.0 tag has been pushed to GitHub.
  #
  # How to obtain the correct sha256:
  #   curl -L https://github.com/ictechgy/understatus/archive/refs/tags/v0.1.0.tar.gz \
  #     | shasum -a 256
  # Or use `brew fetch --build-from-source ictechgy/understatus/understatus` once the
  # tap is registered, which prints the sha256 automatically.
  sha256 "9fd9f0d3d521a9b33d0d965a65db411040251740be93c8448109db71ac42cebe"

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
