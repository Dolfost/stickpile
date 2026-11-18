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

  version "0.8.2"
  sha256 arm:   "71bb65776b543ad99d099e53711545e03fae8573f1b3fde689de1a04a5086678",
         intel: "4cd19a810c2a81057d441a3125f764d70917a1d3f87414b34455288e6d9186f9"

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
