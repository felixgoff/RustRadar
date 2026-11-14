#!/usr/bin/env bash
# The release APK's identity as Android reads it (plan stage 11, row P8's acceptance; job `apk-check`
# of .github/workflows/rust.yml), with the SDK's aapt: `aapt dump badging` must show the application
# id com.felix.rustradar (doc 07 §13.6), the release version and the versionCode
# major * 1000000 + minor * 1000 + patch (the Tauri CLI's rule, so that Android installs the native
# APK over today's: v0.3.16's has 3016), minSdk 24, the INTERNET permission, the launcher activity
# and the four ABIs of today's universal APK.
#
#   scripts/ci/check-apk.sh APK VERSION
#
# aapt is the newest under $ANDROID_HOME/build-tools (GitHub's runners have several).
set -euo pipefail

apk="${1:?usage: check-apk.sh APK VERSION}"
version="${2:?usage: check-apk.sh APK VERSION}"

sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
aapt="$(find "$sdk/build-tools" -mindepth 2 -maxdepth 2 -name aapt -type f 2>/dev/null | sort -V | tail -n 1)"
if [ -z "$aapt" ]; then
  echo "::error title=APK::no aapt under ${sdk:-\$ANDROID_HOME}/build-tools"
  exit 1
fi
echo "$aapt"

IFS=. read -r major minor patch <<<"$version"
code=$((major * 1000000 + minor * 1000 + patch))

badging="$("$aapt" dump badging "$apk")"
printf '%s\n' "$badging" | grep -E "^(package|sdkVersion|targetSdkVersion|uses-permission|application-label|launchable-activity|native-code):"

status=0
expect() {
  if ! grep -qE "$1" <<<"$badging"; then
    echo "::error title=APK::$(basename "$apk"): no $2 (expected /$1/)"
    status=1
  fi
}
expect "^package: name='com\\.felix\\.rustradar' versionCode='$code' versionName='${version//./\\.}'( |\$)" \
  "package com.felix.rustradar at versionCode $code, versionName $version"
expect "^sdkVersion:'24'\$" "minSdk 24"
expect "^uses-permission: name='android\\.permission\\.INTERNET'" "INTERNET permission"
expect "^launchable-activity: name='com\\.felix\\.rustradar\\.MainActivity'" "launcher activity com.felix.rustradar.MainActivity"
expect "^native-code: 'arm64-v8a' 'armeabi-v7a' 'x86' 'x86_64'\$" "native code for arm64-v8a, armeabi-v7a, x86 and x86_64"
[ "$status" = 0 ] || exit 1
echo "APK check passed: $(basename "$apk") is com.felix.rustradar $version (versionCode $code), $(stat -c %s "$apk") bytes"
