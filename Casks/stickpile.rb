# Homebrew cask for the macOS app (docs/macos.md). This repository is its own
# tap:
#
#   brew tap dolfost/stickpile https://github.com/Dolfost/stickpile
#   brew install --cask stickpile
#
# The release workflow sets the version and the checksums on every release
# (job "Homebrew cask" in .github/workflows/release.yml).
cask "stickpile" do
  arch arm: "arm64", intel: "x86_64"

  version "0.7.0"
  sha256 arm:   "0a3ba2ec7a634ad8744e17373947f828cdc390b571d08e47b5def9ea388a925c",
         intel: "4aedf74b9fc36c4fdfbe3f98b85ad1891c2f01d0fe14ba3b5a06a0cdbf76174a"

  url "https://github.com/Dolfost/stickpile/releases/download/v#{version}/Stickpile-v#{version}-#{arch}-macos.zip"
  name "stickpile"
  desc "Quick-search popup for a self-hosted sticker and meme library"
  homepage "https://github.com/Dolfost/stickpile"

  livecheck do
    url :url
    strategy :github_latest
  end

  depends_on macos: ">= :ventura"

  app "Stickpile.app"

  # The app is signed ad hoc, not notarized by Apple: without this, Gatekeeper
  # refuses to start the copy Homebrew downloaded.
  postflight do
    system_command "/usr/bin/xattr",
                   args: ["-dr", "com.apple.quarantine", "#{appdir}/Stickpile.app"]
  end

  uninstall quit: "io.github.dolfost.Stickpile"

  zap trash: [
    "~/Library/Application Support/stickpile",
    "~/Library/Caches/stickpile",
    "~/Library/Logs/stickpile",
  ]
end
