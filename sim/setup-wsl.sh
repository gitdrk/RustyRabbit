#!/bin/bash
# Sets up the Karotz emulation toolchain inside WSL (run as root).
# - QEMU (system + user mode) from the host Ubuntu
# - An Ubuntu 14.04 chroot at /opt/trusty with gcc 4.x, old enough to build Linux 2.6.28
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive

apt-get update -qq
apt-get install -y -qq debootstrap qemu-system-arm qemu-user cpio xz-utils bzip2 wget >/dev/null
echo "[host] $(qemu-system-arm --version | head -1)"

if [ ! -x /opt/trusty/usr/bin/arm-linux-gnueabi-gcc ]; then
  if [ ! -d /opt/trusty/etc ]; then
    debootstrap --variant=buildd --arch=amd64 --components=main,universe \
      trusty /opt/trusty http://archive.ubuntu.com/ubuntu/ \
      || debootstrap --variant=buildd --arch=amd64 --components=main,universe \
           trusty /opt/trusty http://old-releases.ubuntu.com/ubuntu/
  fi
  cat > /opt/trusty/etc/apt/sources.list <<'EOF'
deb http://archive.ubuntu.com/ubuntu/ trusty main universe
EOF
  mount --bind /proc /opt/trusty/proc 2>/dev/null || true
  chroot /opt/trusty bash -c "apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --force-yes gcc-arm-linux-gnueabi make bc perl >/dev/null"
  umount /opt/trusty/proc 2>/dev/null || true
fi
echo "[chroot] $(chroot /opt/trusty arm-linux-gnueabi-gcc --version | head -1)"
echo "[chroot] $(chroot /opt/trusty make --version | head -1)"
