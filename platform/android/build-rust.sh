#!/usr/bin/env bash
# Builds the app's Rust library, librustradar_app.so, into app/src/main/jniLibs/<abi>/ for Gradle
# (doc 07 §13.4). Needs cargo-ndk 4 (`cargo install cargo-ndk --locked`), the Rust targets of the
# ABIs and an NDK (ANDROID_NDK_HOME, or the newest under $ANDROID_HOME/ndk).
#
#   platform/android/build-rust.sh [--release] [ABI ...]     ABIs: arm64-v8a (default),
#                                                              armeabi-v7a, x86, x86_64
# then, in platform/android: ./gradlew assembleDebug (or assembleRelease after --release).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
profile=()
if [ "${1:-}" = "--release" ]; then
  profile=(--release)
  shift
fi
abis=("$@")
[ ${#abis[@]} -gt 0 ] || abis=(arm64-v8a)
targets=()
for abi in "${abis[@]}"; do targets+=(-t "$abi"); done

cd "$here/../.."
# -P: the API level of minSdk (app/build.gradle.kts). The C++ of GameActivity and of the Basis
# transcoder links libc++ statically, so no libc++_shared.so is packaged.
cargo ndk "${targets[@]}" -P 24 -o "$here/app/src/main/jniLibs" build ${profile[@]+"${profile[@]}"} -p rustradar-app --lib
