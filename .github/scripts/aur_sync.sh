#!/bin/bash
# AUR 同步脚本：由 release.yml 的 aur job 以 builder 用户执行。
# 用法: aur_sync.sh <tag, e.g. v0.6.0> <dry_run: true|false>
set -euxo pipefail

TAG="$1"
DRY_RUN="$2"
VER="${TAG#v}"

# ---- cnmplayer（源码包）：升 pkgver 并重置 pkgrel，校验和保持 SKIP ----
git clone -q ssh://aur@aur.archlinux.org/cnmplayer.git
cd cnmplayer
sed -i "s/^pkgver=.*/pkgver=${VER}/" PKGBUILD
sed -i "s/^pkgrel=.*/pkgrel=1/" PKGBUILD
makepkg --printsrcinfo > .SRCINFO
if git commit -aqm "functional update"; then
  [ "${DRY_RUN}" = "true" ] || git push -q
else
  echo "cnmplayer: no changes"
fi
cd ..

# ---- cnmplayer-bin（预编译包）：升 pkgver/pkgrel 并重算全部资产 sha256 ----
git ls-remote ssh://aur@aur.archlinux.org/cnmplayer-bin.git > /dev/null 2>&1 \
  || { echo "cnmplayer-bin missing on AUR — push the initial package manually first"; exit 1; }
git clone -q ssh://aur@aur.archlinux.org/cnmplayer-bin.git
cd cnmplayer-bin
sed -i "s/^pkgver=.*/pkgver=${VER}/" PKGBUILD
sed -i "s/^pkgrel=.*/pkgrel=1/" PKGBUILD
# PKGBUILD 中 URL 为参数化写法（${pkgver}/${_asset_arch}），
# 须 source 求值得到展开后的真实 URL（与 makepkg 同机制），再按架构行原位重算 sha256。
# 兼容未来加入 source_aarch64：届时无需改动本脚本。
for arch in x86_64 aarch64; do
  grep -q "^source_${arch}=" PKGBUILD || continue
  line_no=$(grep -n "^source_${arch}=" PKGBUILD | cut -d: -f1)
  url=$(CARCH="${arch}" bash -ec "source ./PKGBUILD; printf '%s\n' \"\${source_${arch}[@]}\"" | grep '\.tar\.xz')
  url="${url##*::}"
  [ -n "$url" ] || { echo "no tar.xz asset resolved for ${arch}"; exit 1; }
  f=$(basename "${url}")
  curl -fsSL "${url}" -o "/tmp/${f}"
  newsum=$(sha256sum "/tmp/${f}" | cut -d' ' -f1)
  sed -i "${line_no}s/[0-9a-f]\{64\}/${newsum}/" PKGBUILD
done
makepkg --printsrcinfo > .SRCINFO
if git commit -aqm "functional update"; then
  [ "${DRY_RUN}" = "true" ] || git push -q
else
  echo "cnmplayer-bin: no changes"
fi
