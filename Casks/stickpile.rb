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

  version "0.8.8"
  sha256 arm:   "b064637b6ba1240d6e51f1a3e2bae6352ec383091253e674a35605620e4f7284",
         intel: "2db02b5acd9a937db2fb4c339f32526caa0f4260166f8996efef765168e61388"

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
