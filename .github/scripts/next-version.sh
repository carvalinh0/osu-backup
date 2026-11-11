#!/usr/bin/env bash
# Computes the next version from the commits since the last vX.Y.Z tag.
#
# Rules (Conventional Commits):
#   BREAKING CHANGE in the body or "type!:" in the subject -> major
#   "feat:" / "feat(scope):"                                -> minor
#   any other commit                                        -> patch
#
# With no tag at all, the version in Cargo.toml is used as the first release.
# Output (key=value) goes to $GITHUB_OUTPUT when set, otherwise to stdout.
set -euo pipefail

emit() {
  if [ -n "${GITHUB_OUTPUT:-}" ]; then
    printf '%s=%s\n' "$1" "$2" >> "$GITHUB_OUTPUT"
  else
    printf '%s=%s\n' "$1" "$2"
  fi
}

last_tag=$(git describe --tags --abbrev=0 --match 'v[0-9]*.[0-9]*.[0-9]*' 2>/dev/null || true)

if [ -z "$last_tag" ]; then
  version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
  emit release true
  emit version "$version"
  emit tag "v$version"
  exit 0
fi

range="$last_tag..HEAD"
if [ -z "$(git rev-list "$range")" ]; then
  emit release false
  exit 0
fi

bump=patch
while IFS= read -r -d $'\x1e' entry; do
  subject=$(printf '%s' "$entry" | head -n1)
  if printf '%s' "$entry" | grep -q 'BREAKING CHANGE' ||
    printf '%s' "$subject" | grep -Eq '^[a-zA-Z]+(\([^)]*\))?!:'; then
    bump=major
    break
  fi
  if printf '%s' "$subject" | grep -Eq '^feat(\([^)]*\))?:'; then
    bump=minor
  fi
done < <(git log --format='%s%n%b%x1e' "$range")

base=${last_tag#v}
IFS=. read -r major minor patch <<< "$base"
case $bump in
  major) major=$((major + 1)); minor=0; patch=0 ;;
  minor) minor=$((minor + 1)); patch=0 ;;
  patch) patch=$((patch + 1)) ;;
esac

version="$major.$minor.$patch"
emit release true
emit version "$version"
emit tag "v$version"
