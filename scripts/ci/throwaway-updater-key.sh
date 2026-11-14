#!/usr/bin/env bash
# A throwaway updater key for a run of .github/workflows/rust.yml that publishes nothing (a push to
# rust-port, a pull request, a run by hand): its packages are signed with a key made here, which goes
# with the runner, so that `cargo xtask sign` and `cargo xtask latest-json --pubkey` run as they do on
# a release, without the release's key (TAURI_SIGNING_PRIVATE_KEY reaches only the runs that publish).
#
#   scripts/ci/throwaway-updater-key.sh DIR
#
# Writes, with minisign (apt-get install minisign), in Tauri's forms:
#   DIR/updater.key      the private key: base64 of minisign's key file, one line, as the Tauri CLI
#                        writes it (TAURI_SIGNING_PRIVATE_KEY may name this file)
#   DIR/updater.key.pub  the public key: base64 of minisign's .pub file, as tauri.conf.json holds one
#   DIR/password         the key's password (TAURI_SIGNING_PRIVATE_KEY_PASSWORD)
# Prints the public key only.
set -euo pipefail

dir="${1:?usage: throwaway-updater-key.sh DIR}"
mkdir -p "$dir"
chmod 700 "$dir"

password="$(head -c 32 /dev/urandom | base64 | tr -d '/+=\n')"
# minisign reads the password, and its confirmation, from standard input when that is no terminal
printf '%s\n%s\n' "$password" "$password" |
  minisign -G -f -p "$dir/minisign.pub" -s "$dir/minisign.key" >/dev/null
base64 -w0 "$dir/minisign.key" >"$dir/updater.key"
base64 -w0 "$dir/minisign.pub" >"$dir/updater.key.pub"
printf '%s' "$password" >"$dir/password"
rm -f "$dir/minisign.key"
chmod 600 "$dir/updater.key" "$dir/password"

echo "throwaway updater key: $(sed -n 2p "$dir/minisign.pub")"
