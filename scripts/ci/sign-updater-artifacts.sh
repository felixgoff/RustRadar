#!/usr/bin/env bash
# The desktop packages of a release, checked and signed for the updater (.github/workflows/rust.yml,
# job `updater`). DIR holds the artifacts of the `package` jobs; it must hold exactly the packages
# v0.3.16's release has, by their names (doc 07 §9.1, §13), and each one the updater installs gets
# its .sig from `cargo xtask sign` (with TAURI_SIGNING_PRIVATE_KEY and
# TAURI_SIGNING_PRIVATE_KEY_PASSWORD): the NSIS installer, the MSI, both .app.tar.gz, the AppImage,
# the deb and the rpm, as today. The dmgs get none, as today.
#
# Each .app.tar.gz is signed under the name tauri-bundler gave it, RustRadar.app.tar.gz, which
# v0.3.16's .sig files name in their trusted comment (as `cargo xtask package app --sign` does), and
# its .sig is put beside it under its asset name.
#
#   XTASK=target/debug/xtask scripts/ci/sign-updater-artifacts.sh DIR VERSION
#
# Prints names and sizes only, never the key.
set -euo pipefail

dir="${1:?usage: sign-updater-artifacts.sh DIR VERSION}"
version="${2:?usage: sign-updater-artifacts.sh DIR VERSION}"
xtask="${XTASK:-target/debug/xtask}"

signed=(
  "RustRadar_${version}_x64-setup.exe"
  "RustRadar_${version}_x64_en-US.msi"
  "RustRadar_${version}_aarch64.app.tar.gz"
  "RustRadar_${version}_x64.app.tar.gz"
  "RustRadar_${version}_amd64.AppImage"
  "RustRadar_${version}_amd64.deb"
  "RustRadar-${version}-1.x86_64.rpm"
)
unsigned=(
  "RustRadar_${version}_aarch64.dmg"
  "RustRadar_${version}_x64.dmg"
)

status=0
for name in "${signed[@]}" "${unsigned[@]}"; do
  if [ ! -f "$dir/$name" ]; then
    echo "::error title=Release packages::$name is missing"
    status=1
  fi
done
# and nothing else: whatever is in DIR goes out with the release
for path in "$dir"/*; do
  name="$(basename "$path")"
  case " ${signed[*]} ${unsigned[*]} " in
    *" $name "*) ;;
    *)
      echo "::error title=Release packages::$name is not one of the release's packages"
      status=1
      ;;
  esac
done
ls -l "$dir"
[ "$status" = 0 ] || exit 1

# one `xtask sign` for all of them: the key is decrypted once (scrypt, about a minute and a half in
# xtask's debug build)
work="$(mktemp -d)"
files=()
for name in "${signed[@]}"; do
  case "$name" in
    *.app.tar.gz)
      mkdir "$work/$name"
      ln "$dir/$name" "$work/$name/RustRadar.app.tar.gz" 2>/dev/null || cp "$dir/$name" "$work/$name/RustRadar.app.tar.gz"
      files+=("$work/$name/RustRadar.app.tar.gz")
      ;;
    *) files+=("$dir/$name") ;;
  esac
done
"$xtask" sign "${files[@]}"
for name in "${signed[@]}"; do
  case "$name" in
    *.app.tar.gz) mv "$work/$name/RustRadar.app.tar.gz.sig" "$dir/$name.sig" ;;
  esac
done
rm -rf "$work"
ls -l "$dir"
