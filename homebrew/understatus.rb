# Homebrew formula for understatus (prebuilt binary)
# Tap: ictechgy/homebrew-understatus
# Formula path in tap repo: Formula/understatus.rb
#
# This formula installs a PREBUILT understatus binary from GitHub Releases.
# No Xcode Command Line Tools and no Rust toolchain are required — the binary
# is downloaded and dropped into place (no compilation). macOS only
# (Apple Silicon + Intel); the binary's deployment target is macOS 11.0, so it
# runs on macOS 11 (Big Sur) and newer.
#
# Previous versions built from source (depends_on "rust" => :build +
# `cargo install`), which forced every user to install the full Xcode Command
# Line Tools just to compile. Switching to the prebuilt binary removes that
# requirement entirely.

class Understatus < Formula
  desc "Claude Code statusline addon: CPU/memory/session info with pulse glyphs"
  homepage "https://github.com/ictechgy/understatus"
  version "0.6.0"
  license "MIT"

  # understatus uses macOS-only APIs (host_processor_info, sysctl, IOKit FFI).
  depends_on :macos

  # Architecture-specific prebuilt tarball + its SHA-256.
  #   curl -L <release tarball> | shasum -a 256
  on_macos do
    on_arm do
      url "https://github.com/ictechgy/understatus/releases/download/v0.6.0/understatus-0.6.0-aarch64-apple-darwin.tar.gz"
      sha256 "c911694482440d4b1c828a61a95ae7cf16cecd2f6a676c0d9d6abd9a6638aad3"
    end
    on_intel do
      url "https://github.com/ictechgy/understatus/releases/download/v0.6.0/understatus-0.6.0-x86_64-apple-darwin.tar.gz"
      sha256 "febcd0f19d5e41934b71d4c14f5abb22280f269382fd270bd27ecb4260ffd452"
    end
  end

  def install
    # The release tarball contains a single "understatus" executable at its root.
    bin.install "understatus"
  end

  test do
    # Verify the binary exists and reports the expected crate name in its version string.
    assert_match "understatus", shell_output("#{bin}/understatus --version")
  end
end
