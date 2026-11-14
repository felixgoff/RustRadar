#!/usr/bin/env bash
# The AppImage starts and draws a picture headless (job `linux-install` of .github/workflows/rust.yml;
# the .deb has platform/linux/install-check.sh): run as a user runs it, without FUSE
# (--appimage-extract-and-run), on Mesa's lavapipe (mesa-vulkan-drivers) for a GPU, Europe (view V2)
# from the committed fixtures, as every shot is (plan section 8), from the repository root.
#
#   scripts/ci/appimage-check.sh RustRadar_<version>_amd64.AppImage [shot.png]
set -euo pipefail

appimage="$(realpath "${1:?usage: appimage-check.sh APPIMAGE [shot.png]}")"
shot="$(realpath -m "${2:-/tmp/rustradar-appimage-check.png}")"
# AppImageKit's AppRun starts the app in $APPDIR/usr, so every path on its command line is absolute
fixture="$(realpath crates/map/tests/data)"

chmod +x "$appimage"
mkdir -p "$(dirname "$shot")"
rm -f "$shot"
log="$("$appimage" --appimage-extract-and-run --screenshot "$shot" --size 1100x720 --view 52,10,4.5,0,0 \
  --basemap dark --fixture "$fixture" --now 1782036000000 2>&1)"
printf '%s\n' "$log"
grep -q "backend started" <<<"$log"
test "$(od -An -tx1 -N8 "$shot" | tr -d ' \n')" = 89504e470d0a1a0a
test "$(stat -c %s "$shot")" -gt 20000
echo "AppImage check passed: $(stat -c %s "$shot") bytes in $shot"
