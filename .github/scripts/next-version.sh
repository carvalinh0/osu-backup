#!/usr/bin/env bash
# Calcula a próxima versão a partir dos commits desde a última tag vX.Y.Z.
#
# Regras (Conventional Commits):
#   BREAKING CHANGE no corpo ou "tipo!:" no assunto -> major
#   "feat:" / "feat(escopo):"                         -> minor
#   qualquer outro commit                              -> patch
#
# Sem nenhuma tag, usa a versão de Cargo.toml como primeira release.
# Saída (key=value) vai para $GITHUB_OUTPUT quando definido, senão para stdout.
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
