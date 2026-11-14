#!/usr/bin/env bash
# Builds the app's Rust library, librustradar_app.a, for what Xcode builds (doc 07 §13.5). The
# RustRadar target runs it before compiling (project.yml); Xcode then links the library.
# Standalone:  platform/ios/build-rust.sh [iphonesimulator|iphoneos] [Debug|Release] [OUT_DIR]
# Needs the Rust targets aarch64-apple-ios and aarch64-apple-ios-sim.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
platform="${PLATFORM_NAME:-${1:-iphonesimulator}}"
configuration="${CONFIGURATION:-${2:-Debug}}"
out="${BUILT_PRODUCTS_DIR:-${3:-$here/build}}"

case "$platform" in
  iphoneos) target=aarch64-apple-ios ;;
  # Apple silicon simulators only (project.yml excludes x86_64 there)
  iphonesimulator) target=aarch64-apple-ios-sim ;;
  *) echo "error: no Rust target for the platform $platform" >&2; exit 1 ;;
esac
profile_dir=debug
flags=()
if [ "$configuration" = "Release" ]; then
  profile_dir=release
  # a release builds exactly the committed Cargo.lock
  flags=(--release --locked)
fi

# Xcode runs build phases without the login shell's PATH
export PATH="$HOME/.cargo/bin:$PATH"
cd "$here/../.."
# Xcode's SDKROOT names the iOS SDK; cargo's build scripts run on the Mac and must not link
# against it (cc-rs finds the iOS SDK itself through xcrun).
env -u SDKROOT IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-15.0}" \
  cargo build -p rustradar-app --lib --target "$target" ${flags[@]+"${flags[@]}"}
mkdir -p "$out"
cp "target/$target/$profile_dir/librustradar_app.a" "$out/librustradar_app.a"
