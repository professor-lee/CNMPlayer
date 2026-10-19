#!/bin/bash
# Run as root inside the Arch Linux AUR job container.
set -euo pipefail
: "${AUR_SSH_KEY:?AUR_SSH_KEY is not set}"
pacman -Syu --noconfirm git openssh curl
useradd -m builder
install -d -m700 -o builder -g builder /home/builder/.ssh
printf '%s\n' "$AUR_SSH_KEY" > /home/builder/.ssh/aur_key
chown builder:builder /home/builder/.ssh/aur_key
chmod 600 /home/builder/.ssh/aur_key
ssh-keyscan -H aur.archlinux.org > /home/builder/.ssh/known_hosts
chown builder:builder /home/builder/.ssh/known_hosts
git config --file /home/builder/.gitconfig core.sshCommand \
  'ssh -i /home/builder/.ssh/aur_key -oIdentitiesOnly=yes'
git config --file /home/builder/.gitconfig user.name '白鞋油'
git config --file /home/builder/.gitconfig user.email \
  '89632742+professor-lee@users.noreply.github.com'
chown builder:builder /home/builder/.gitconfig
