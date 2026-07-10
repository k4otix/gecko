# Homebrew formula TEMPLATE for `gecko` (plan P6).
#
# This is a template, not a working tap: a real Homebrew tap lives in its own
# `homebrew-gecko` repository (e.g. `k4otix/homebrew-gecko`) so `brew install
# k4otix/gecko/gecko` resolves. To cut a release:
#
#   1. Push a tag (`vX.Y.Z`) or publish a GitHub Release — this repo's
#      `.github/workflows/release.yml` builds and uploads the three platform
#      archives (aarch64-apple-darwin, x86_64-apple-darwin,
#      x86_64-unknown-linux-gnu) to that GitHub Release.
#   2. Compute the sha256 of each uploaded archive:
#        curl -fsSL <asset-url> | shasum -a 256
#   3. Copy this file into the `homebrew-gecko` tap repo (as
#      `Formula/gecko.rb`), fill in `version` and the three `sha256` values
#      below, and commit.
#   4. Users then run: brew install k4otix/gecko/gecko
#
# The formula installs ONLY the `gecko` binary — never the ~1.3GB embedding
# model or TypeDB, both of which are runtime assets fetched by `gecko model
# fetch` / `gecko up` on first use, not build- or install-time dependencies.
class Gecko < Formula
  desc "Graph Execution of Contextual Knowledge Objects — knowledge graph + orchestration engine"
  homepage "https://github.com/k4otix/gecko"
  version "0.0.0" # TODO: set to the released tag, e.g. "0.1.0" (no leading "v")
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/k4otix/gecko/releases/download/v#{version}/gecko-v#{version}-aarch64-apple-darwin.zip"
      sha256 "REPLACE_WITH_SHA256_OF_AARCH64_APPLE_DARWIN_ZIP"
    end
    on_intel do
      url "https://github.com/k4otix/gecko/releases/download/v#{version}/gecko-v#{version}-x86_64-apple-darwin.zip"
      sha256 "REPLACE_WITH_SHA256_OF_X86_64_APPLE_DARWIN_ZIP"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/k4otix/gecko/releases/download/v#{version}/gecko-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_SHA256_OF_X86_64_UNKNOWN_LINUX_GNU_TAR_GZ"
    end
  end

  def install
    bin.install "gecko"
  end

  def caveats
    <<~EOS
      gecko is installed, but its runtime assets are NOT bundled:

        gecko model fetch      # one-time download of the embedding model (~1.3GB)
        gecko up && gecko sync # start the pinned TypeDB and sync a bundle

      For an air-gapped install, see docs/INSTALL.md in the gecko repo for the
      `external` TypeDB mode + `[semantic_index] model_path` configuration.
    EOS
  end

  test do
    system "#{bin}/gecko", "--version"
  end
end
