#!/bin/sh
# Build Stickpile.app on a Mac: the Rust staticlib (crates/stickpile-mac),
# its Swift bindings (UniFFI), the Swift app in app/Sources, the bundle.
# Needs Rust and the Command Line Tools (or Xcode). Run from anywhere.
#
#   SKIP_CARGO=1   use the staticlib and uniffi-bindgen already built
#   TARGET=aarch64-apple-darwin | x86_64-apple-darwin   (default: the host's,
#                  built into target/release; with TARGET into target/$TARGET/release)
#   OUT=DIR        where Stickpile.app and Stickpile.zip go, relative to the
#                  repo root (default: dist)
set -eu

APP_DIR=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$APP_DIR/../../.." && pwd)
cd "$ROOT"
export PATH="$HOME/.cargo/bin:$PATH"
# C code in the staticlib (libwebp) is built for the same oldest macOS as the app.
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-13.0}"

TARGET_DIR=${CARGO_TARGET_DIR:-target}
if [ -n "${TARGET:-}" ]; then
	CARGO_TARGET="--target $TARGET"
	LIB_DIR="$TARGET_DIR/$TARGET/release"
	ARCH=$TARGET
else
	CARGO_TARGET=""
	LIB_DIR="$TARGET_DIR/release"
	ARCH=$(uname -m)
fi
case "$ARCH" in
aarch64-apple-darwin | arm64) SWIFT_TARGET=arm64-apple-macos13.0 ;;
x86_64-apple-darwin | x86_64) SWIFT_TARGET=x86_64-apple-macos13.0 ;;
*)
	echo "build.sh: unsupported target $ARCH" >&2
	exit 2
	;;
esac

BUILD="$APP_DIR/build/$SWIFT_TARGET"
GEN="$BUILD/generated"
OUT_DIR="$ROOT/${OUT:-dist}"
LIB="$LIB_DIR/libstickpile_mac.a"
# What the staticlib needs from the system (rustc --print native-static-libs).
NATIVE_CACHE="$LIB_DIR/stickpile-mac-native-libs.txt"
mkdir -p "$BUILD"

# The bindings generator runs here, so it is built for this machine: when
# cross-building (an Intel app on Apple silicon) it lives in another directory.
HOST=$(rustc -vV | sed -n 's/^host: //p')
if [ -n "${TARGET:-}" ] && [ "$TARGET" != "$HOST" ]; then
	BINDGEN="$TARGET_DIR/release/uniffi-bindgen"
else
	BINDGEN="$LIB_DIR/uniffi-bindgen"
fi

if [ "${SKIP_CARGO:-}" != 1 ]; then
	# shellcheck disable=SC2086
	cargo build --release --locked $CARGO_TARGET -p stickpile-mac --features bindgen
	if [ "$BINDGEN" != "$LIB_DIR/uniffi-bindgen" ]; then
		cargo build --release --locked -p stickpile-mac --features bindgen --bin uniffi-bindgen
	fi
	if [ ! -s "$NATIVE_CACHE" ] || [ Cargo.lock -nt "$NATIVE_CACHE" ]; then
		# shellcheck disable=SC2086
		cargo rustc --release --locked $CARGO_TARGET -p stickpile-mac --features bindgen --lib \
			--crate-type staticlib -- --print native-static-libs 2>"$BUILD.native.log" || {
			cat "$BUILD.native.log" >&2
			exit 1
		}
		sed -n 's/.*native-static-libs: //p' "$BUILD.native.log" | tail -n 1 >"$NATIVE_CACHE"
	fi
fi
[ -f "$LIB" ] || {
	echo "build.sh: $LIB is missing (run without SKIP_CARGO=1)" >&2
	exit 1
}
# swiftc links libSystem itself.
NATIVE_LIBS=$(sed 's/-lSystem//g' "$NATIVE_CACHE" 2>/dev/null || true)
if [ -z "$NATIVE_LIBS" ]; then
	NATIVE_LIBS="-framework CoreFoundation -framework Security -framework SystemConfiguration -liconv -lc -lm"
fi

rm -rf "$GEN"
mkdir -p "$GEN"
"$BINDGEN" "$LIB" "$GEN" --swift-sources --headers --modulemap \
	--module-name stickpile_macFFI --modulemap-filename module.modulemap

echo "swiftc: $SWIFT_TARGET"
# shellcheck disable=SC2086
swiftc -O -parse-as-library -swift-version 5 -target "$SWIFT_TARGET" -module-name Stickpile \
	-Xcc -fmodule-map-file="$GEN/module.modulemap" -I "$GEN" \
	"$APP_DIR"/Sources/*.swift "$GEN/stickpile_mac.swift" \
	-Xlinker "$LIB" $NATIVE_LIBS \
	-framework AppKit -framework Carbon -framework ServiceManagement \
	-Xlinker -dead_strip \
	-o "$BUILD/Stickpile"

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
BUNDLE="$OUT_DIR/Stickpile.app"
rm -rf "$BUNDLE"
mkdir -p "$BUNDLE/Contents/MacOS" "$BUNDLE/Contents/Resources"
cp "$BUILD/Stickpile" "$BUNDLE/Contents/MacOS/Stickpile"
sed "s/@VERSION@/$VERSION/g" "$APP_DIR/Info.plist" >"$BUNDLE/Contents/Info.plist"
iconutil -c icns "$APP_DIR/Resources/AppIcon.iconset" -o "$BUNDLE/Contents/Resources/AppIcon.icns"
codesign --force --deep -s - "$BUNDLE"

ZIP="$OUT_DIR/Stickpile.zip"
rm -f "$ZIP"
ditto -c -k --keepParent "$BUNDLE" "$ZIP"
echo "$ZIP"
