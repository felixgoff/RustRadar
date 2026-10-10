#!/usr/bin/env bash
# Checks that $KEY (the TAURI_SIGNING_PRIVATE_KEY secret) is a whole Tauri
# private key file, and names the likely mistake if not, before a release
# build spends minutes and then fails to sign. Prints only lengths, never the
# key. Run by the bundle job in .github/workflows/rust.yml.
set -u

fail() {
  echo "::error title=Signing key::$1"
  exit 1
}

# leading and trailing whitespace is harmless (GitHub often keeps a final newline)
key="${KEY:-}"
key="${key#"${key%%[![:space:]]*}"}"
key="${key%"${key##*[![:space:]]}"}"

[ -n "$key" ] || fail "TAURI_SIGNING_PRIVATE_KEY is not set. Add it under Settings > Secrets and variables > Actions: the whole contents of rustradar.key."

case "$key" in
  *[[:space:]]*) fail "TAURI_SIGNING_PRIVATE_KEY contains spaces or line breaks (${#key} characters). It must be the single line in rustradar.key, pasted unchanged." ;;
esac

# a key file is base64 of two lines: a comment, then the minisign key
decoded="$(printf '%s' "$key" | base64 -d 2>/dev/null)" ||
  fail "TAURI_SIGNING_PRIVATE_KEY isn't a Tauri key file (${#key} characters, not base64). Paste the whole of rustradar.key: one line of about 350 characters."
comment="$(printf '%s\n' "$decoded" | sed -n 1p)"
line="$(printf '%s\n' "$decoded" | sed -n 2p)"
case "$comment" in
  "untrusted comment:"*) ;;
  *) fail "TAURI_SIGNING_PRIVATE_KEY isn't a Tauri key file (${#key} characters). Paste the whole of rustradar.key: one line of about 350 characters." ;;
esac

# a minisign public key is 42 bytes, a private key 158
bytes="$(printf '%s' "$line" | base64 -d 2>/dev/null | wc -c | tr -d ' ')"
case "$bytes" in
  42) fail "TAURI_SIGNING_PRIVATE_KEY holds the PUBLIC key (rustradar.key.pub). Paste the private one, rustradar.key, instead." ;;
  158) ;;
  *) fail "TAURI_SIGNING_PRIVATE_KEY looks cut short or damaged (${#key} characters). Paste the whole of rustradar.key again." ;;
esac

echo "Signing key looks right (${#key} characters)."
