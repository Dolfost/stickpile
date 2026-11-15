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

  version "0.5.2"
  sha256 arm:   "817e9bded63f6515e8f4ad4a68ec53c24c5aaf5ecf0805cc85e1d2ae79761d17",
         intel: "75322671017520f3daa7151c19958b5008195b5db8bc82faf652177bf9dfc83f"

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
