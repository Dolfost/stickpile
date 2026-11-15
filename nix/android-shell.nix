# `nix develop .#android`: what the Android app in android/ needs (see
# docs/android.md): a JDK, the Android SDK and NDK, cargo-ndk, and a Rust
# toolchain with the three Android targets. The packages, checks and the
# default shell keep nixpkgs' own Rust; only this shell uses rust-overlay.
#
# The Android SDK is unfree in nixpkgs: its licence is accepted here, for
# this shell only.
#
# With `emulator = true` (`nix develop .#android-emulator`) it also has the
# Android emulator and an AOSP x86_64 system image (no Google services), to
# run the app on a virtual device; it needs /dev/kvm.
{
  nixpkgs,
  rust-overlay,
  system,
  emulator ? false,
}:
let
  pkgs = import nixpkgs {
    inherit system;
    overlays = [ rust-overlay.overlays.default ];
    config = {
      allowUnfree = true;
      android_sdk.accept_license = true;
    };
  };

  # Keep in step with android/app/build.gradle.kts (compileSdk, ndkVersion)
  # and with the Android Gradle Plugin's default build tools.
  platformVersion = "37.0";
  buildToolsVersion = "36.0.0";
  ndkVersion = "28.2.13676358";

  # The emulator's system image: the newest AOSP ("default") one.
  imageVersion = "36";

  android = pkgs.androidenv.composeAndroidPackages (
    {
      platformVersions = [ platformVersion ];
      buildToolsVersions = [ buildToolsVersion ];
      includeNDK = true;
      ndkVersions = [ ndkVersion ];
      includeCmake = false;
      includeEmulator = false;
      includeSystemImages = false;
    }
    // pkgs.lib.optionalAttrs emulator {
      platformVersions = [
        platformVersion
        imageVersion
      ];
      includeEmulator = true;
      includeSystemImages = true;
      systemImageTypes = [ "default" ];
      abiVersions = [ "x86_64" ];
    }
  );
  sdkRoot = "${android.androidsdk}/libexec/android-sdk";

  # The same Rust release as nixpkgs' rustc, with the Android standard libraries.
  rust = pkgs.rust-bin.stable.${pkgs.rustc.version}.default.override {
    extensions = [ "rust-src" ];
    targets = [
      "aarch64-linux-android"
      "armv7-linux-androideabi"
      "x86_64-linux-android"
    ];
  };
  # JDK 21: what F-Droid's build server (Debian trixie) builds with.
  jdk = pkgs.jdk21;
in
pkgs.mkShell {
  packages = [
    rust
    jdk
    android.androidsdk
    pkgs.cargo-ndk
    pkgs.git
  ];
  ANDROID_HOME = sdkRoot;
  ANDROID_SDK_ROOT = sdkRoot;
  ANDROID_NDK_HOME = "${sdkRoot}/ndk/${ndkVersion}";
  JAVA_HOME = jdk.home;
  # The aapt2 that the Android Gradle Plugin downloads from Google's Maven is
  # a generic Linux binary that cannot run on NixOS: Gradle gets the SDK's
  # own through GRADLE_OPTS. A committed gradle.properties line would do the
  # same for every build, F-Droid's included, and break it there.
  shellHook = ''
    unset LD_LIBRARY_PATH
    export GRADLE_OPTS="-Dorg.gradle.project.android.aapt2FromMavenOverride=${sdkRoot}/build-tools/${buildToolsVersion}/aapt2 ''${GRADLE_OPTS:-}"
  '';
}
