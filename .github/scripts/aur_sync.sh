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
# PKGBUILD 中 URL 为参数化写法（${pkgver} 等），须 source 求值得到展开后的真实
# URL（与 makepkg 同机制），再按架构重算 sha256。
#
# 注意 sed 必须锚定 sha256sums_${arch}= 那一行：早先版本取的是 source_${arch}= 的
# 行号，而校验和在其下一行，替换因此静默失效（no-op），推出的包会带上一版的旧
# 校验和、安装时校验失败。
for arch in x86_64 aarch64; do
  grep -q "^source_${arch}=" PKGBUILD || continue
  url=$(CARCH="${arch}" bash -ec "source ./PKGBUILD; printf '%s\n' \"\${source_${arch}[@]}\"" | grep '\.tar\.xz')
  url="${url##*::}"
  [ -n "$url" ] || { echo "no tar.xz asset resolved for ${arch}"; exit 1; }
  f="${arch}-$(basename "${url}")"
  curl -fsSL "${url}" -o "/tmp/${f}"
  newsum=$(sha256sum "/tmp/${f}" | cut -d' ' -f1)

  sum_line=$(grep -n "^sha256sums_${arch}=" PKGBUILD | cut -d: -f1)
  [ -n "$sum_line" ] || { echo "no sha256sums_${arch}= line in PKGBUILD"; exit 1; }
  sed -i "${sum_line}s/[0-9a-f]\{64\}/${newsum}/" PKGBUILD

  # 校验替换确实生效，避免再次静默失败
  grep -q "^sha256sums_${arch}=.*${newsum}" PKGBUILD \
    || { echo "failed to update sha256sums_${arch}"; exit 1; }
done
makepkg --printsrcinfo > .SRCINFO
if git commit -aqm "functional update"; then
  [ "${DRY_RUN}" = "true" ] || git push -q
else
  echo "cnmplayer-bin: no changes"
fi
