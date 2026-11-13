#!/usr/bin/env bash
# The Linux packages' acceptance (plan stage 11, row P7): in a fresh ubuntu:22.04, the build's baseline
# (doc 07 §13.3), the .deb installs with apt, which must find every package it depends on, and the
# installed binary starts and draws a picture headless. Run as root inside the container, from the
# repository root, whose committed fixtures (crates/map/tests/data) the shot reads:
#
#   docker run --rm -v "$PWD:/src" -w /src ubuntu:22.04 \
#     bash platform/linux/install-check.sh target/package/RustRadar_<version>_amd64.deb [shot.png]
#
# Mesa's lavapipe (mesa-vulkan-drivers) stands in for a GPU: the package depends on the Vulkan and EGL
# loaders, not on a driver. Nothing but apt uses the network: the shot reads the fixtures (--fixture),
# as every shot does (plan section 8).
set -euo pipefail

deb="${1:?usage: install-check.sh path/to/RustRadar_<version>_amd64.deb [shot.png]}"
shot="${2:-/tmp/rustradar-install-check.png}"
deb="$(realpath "$deb")"

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y --no-install-recommends "$deb" mesa-vulkan-drivers

# installed where today's Tauri package installs, under its name
dpkg-query -W -f='${Package} ${Version} ${Status}\n' rust-radar
test -x /usr/bin/rustradar
test -f /usr/share/applications/RustRadar.desktop
for size in 32x32 128x128 256x256@2; do
    test -f "/usr/share/icons/hicolor/$size/apps/rustradar.png"
done

# the binary starts, with its backend (whose HTTP client needs the system's root certificates), and
# draws Europe (view V2) from the fixtures on the software driver
rm -f "$shot"
log="$(rustradar --screenshot "$shot" --size 1100x720 --view 52,10,4.5,0,0 --basemap dark \
    --fixture crates/map/tests/data --now 1782036000000 2>&1)"
printf '%s\n' "$log"
grep -q "backend started" <<<"$log"
test "$(od -An -tx1 -N8 "$shot" | tr -d ' \n')" = 89504e470d0a1a0a
test "$(stat -c %s "$shot")" -gt 20000
echo "install check passed: $(stat -c %s "$shot") bytes in $shot"
