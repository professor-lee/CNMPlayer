#!/bin/bash
# Render repository-owned recipes, then publish only changed AUR metadata.
# Usage: aur_sync.sh render <tag> <output-dir>
#        aur_sync.sh release <tag> <dry-run: true|false>
#        aur_sync.sh git <dry-run: true|false>
set -euo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
RECIPES="$ROOT/packaging/aur"

render_release() {
  local tag=$1 output=$2 version source_sum x86_64_sum aarch64_sum
  [[ $tag =~ ^v[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]] \
    || { echo "Invalid release tag: $tag" >&2; return 1; }
  version=${tag#v}
  mkdir -p "$output/cnmplayer" "$output/cnmplayer-bin"
  curl -fL --silent --show-error \
    "https://github.com/professor-lee/CNMPlayer/archive/refs/tags/$tag.tar.gz" \
    -o "$output/source.tar.gz"
  curl -fL --silent --show-error \
    "https://github.com/professor-lee/CNMPlayer/releases/download/$tag/CNMPlayer_${tag}_linux_amd64.tar.xz" \
    -o "$output/amd64.tar.xz"
  curl -fL --silent --show-error \
    "https://github.com/professor-lee/CNMPlayer/releases/download/$tag/CNMPlayer_${tag}_linux_aarch64.tar.xz" \
    -o "$output/aarch64.tar.xz"
  source_sum=$(sha256sum "$output/source.tar.gz"); source_sum=${source_sum%% *}
  x86_64_sum=$(sha256sum "$output/amd64.tar.xz"); x86_64_sum=${x86_64_sum%% *}
  aarch64_sum=$(sha256sum "$output/aarch64.tar.xz"); aarch64_sum=${aarch64_sum%% *}
  sed -e "s/@PKGVER@/$version/g" -e "s/@SOURCE_SHA256@/$source_sum/g" \
    "$RECIPES/cnmplayer/PKGBUILD.in" > "$output/cnmplayer/PKGBUILD"
  sed -e "s/@PKGVER@/$version/g" \
    -e "s/@X86_64_SHA256@/$x86_64_sum/g" -e "s/@AARCH64_SHA256@/$aarch64_sum/g" \
    "$RECIPES/cnmplayer-bin/PKGBUILD.in" > "$output/cnmplayer-bin/PKGBUILD"
  rm -- "$output/source.tar.gz" "$output/amd64.tar.xz" "$output/aarch64.tar.xz"
  local package
  for package in cnmplayer cnmplayer-bin; do
    if grep -Eq '@[A-Z_0-9]+@' "$output/$package/PKGBUILD"; then
      echo "Unresolved template placeholder in $package" >&2
      return 1
    fi
    (cd "$output/$package" && makepkg --printsrcinfo > .SRCINFO)
  done
}

stage_package() {
  local package=$1 recipe=$2
  git clone -q "https://aur.archlinux.org/$package.git" "$WORK/$package"
  git -C "$WORK/$package" remote set-url --push origin "ssh://aur@aur.archlinux.org/$package.git"
  cp -- "$recipe" "$WORK/$package/PKGBUILD"
  (cd "$WORK/$package" && makepkg --printsrcinfo > .SRCINFO)
  git -C "$WORK/$package" add PKGBUILD .SRCINFO
}

publish_package() {
  local package=$1
  if git -C "$WORK/$package" diff --cached --quiet; then
    echo "$package: no changes"
  elif [[ $DRY_RUN == true ]]; then
    echo "$package: dry run; changes not committed or pushed"
    git -C "$WORK/$package" diff --cached --stat
  else
    git -C "$WORK/$package" commit -m "Update $package packaging from CNMPlayer"
    git -C "$WORK/$package" push origin HEAD:master
  fi
}

MODE=${1:-}
case "$MODE" in
  render)
    [[ $# == 3 ]] || { echo "Usage: $0 render <tag> <output-dir>" >&2; exit 2; }
    render_release "$2" "$3"
    exit
    ;;
  release)
    [[ $# == 3 ]] || { echo "Usage: $0 release <tag> <true|false>" >&2; exit 2; }
    TAG=$2
    DRY_RUN=$3
    ;;
  git)
    [[ $# == 2 ]] || { echo "Usage: $0 git <true|false>" >&2; exit 2; }
    DRY_RUN=$2
    ;;
  *) echo "Usage: $0 {render <tag> <output-dir>|release <tag> <true|false>|git <true|false>}" >&2; exit 2 ;;
esac
[[ $DRY_RUN == true || $DRY_RUN == false ]] \
  || { echo "dry-run must be true or false" >&2; exit 2; }
WORK=$(mktemp -d)
trap 'rm -rf -- "$WORK"' EXIT
if [[ $MODE == release ]]; then
  render_release "$TAG" "$WORK/rendered"
  # Prepare both packages before publishing either one.
  for package in cnmplayer cnmplayer-bin; do
    stage_package "$package" "$WORK/rendered/$package/PKGBUILD"
  done
  for package in cnmplayer cnmplayer-bin; do publish_package "$package"; done
else
  stage_package cnmplayer-git "$RECIPES/cnmplayer-git/PKGBUILD"
  publish_package cnmplayer-git
fi
