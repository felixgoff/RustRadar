#!/usr/bin/env bash
# Prints the version a release build publishes: the one in the repository's
# files, unless a release with that version (or a later one) already exists,
# in which case the next patch after the latest release.
#
# Installed apps update only to a *newer* version. A merge to main that
# forgets `bun run bump` would otherwise rebuild the existing release under
# the same number, and nobody would get it.
#
#   scripts/release-version.sh            uses the git tags on origin
#   TAGS="v0.2.4 v0.2.5" scripts/...      uses these instead (tests)
set -euo pipefail

current="$(node -p 'require("./package.json").version')"

if [ -n "${TAGS+x}" ]; then
  tags="$TAGS"
else
  tags="$(git ls-remote --tags --refs origin 'refs/tags/v*' | sed 's#.*refs/tags/##')"
fi

# the highest x.y.z among the tags (pre-release and odd tags ignored)
latest="$(printf '%s\n' $tags | sed -n 's/^v\([0-9]\+\.[0-9]\+\.[0-9]\+\)$/\1/p' | sort -V | tail -n 1)"

if [ -z "$latest" ]; then
  echo "$current"
  exit 0
fi

# the files' version wins when it is newer than every release
newest="$(printf '%s\n%s\n' "$current" "$latest" | sort -V | tail -n 1)"
if [ "$newest" = "$current" ] && [ "$current" != "$latest" ]; then
  echo "$current"
  exit 0
fi

IFS=. read -r major minor patch <<<"$latest"
echo "$major.$minor.$((patch + 1))"
