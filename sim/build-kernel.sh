#!/bin/bash
# Builds Linux 2.6.28 for QEMU's versatilepb board inside the Ubuntu 14.04
# chroot, plus the syscall probe and an initramfs that runs it as /init.
# Run as root in WSL after setup-wsl.sh.
set -euo pipefail
CH=/opt/trusty
SIM=/mnt/c/Code/Karotz/sim
OUT=$SIM/out
mkdir -p "$OUT" "$CH/build"

# Ubuntu's armel gcc defaults to Thumb-2 and stack protector; the 2011 kernel wants neither.
CC_FIX='arm-linux-gnueabi-gcc -marm -fno-stack-protector'

if [ ! -d "$CH/build/linux-2.6.28" ]; then
  wget -q -O "$CH/build/linux-2.6.28.tar.bz2" \
    https://cdn.kernel.org/pub/linux/kernel/v2.6/linux-2.6.28.tar.bz2
  tar -xjf "$CH/build/linux-2.6.28.tar.bz2" -C "$CH/build"
fi

chroot "$CH" bash -euo pipefail -c "
cd /build/linux-2.6.28
K='make ARCH=arm CROSS_COMPILE=arm-linux-gnueabi- CC=\"$CC_FIX\"'
eval \$K versatile_defconfig >/dev/null
# Match the rabbit: EABI userspace, initramfs support, serial console.
./scripts/config --file .config -e AEABI -e OABI_COMPAT -e BLK_DEV_INITRD \
  -e SERIAL_AMBA_PL011 -e SERIAL_AMBA_PL011_CONSOLE 2>/dev/null || {
  sed -i -e 's/^# CONFIG_AEABI is not set/CONFIG_AEABI=y/' \
         -e 's/^# CONFIG_BLK_DEV_INITRD is not set/CONFIG_BLK_DEV_INITRD=y/' .config
  grep -q '^CONFIG_OABI_COMPAT' .config || echo 'CONFIG_OABI_COMPAT=y' >> .config
}
set +o pipefail
yes '' | eval \$K oldconfig >/dev/null 2>&1
set -o pipefail
grep -E '^CONFIG_(AEABI|OABI_COMPAT|BLK_DEV_INITRD|CPU_ARM926T|SERIAL_AMBA_PL011_CONSOLE)=' .config
eval \$K -j\$(nproc) zImage > /build/kernel-build.log 2>&1 || { tail -40 /build/kernel-build.log; exit 1; }
tail -3 /build/kernel-build.log
"
cp "$CH/build/linux-2.6.28/arch/arm/boot/zImage" "$OUT/zImage-2.6.28"

# Probe: ARMv4T, soft-float, no libc.
cp "$SIM/probe.c" "$CH/build/probe.c"
chroot "$CH" arm-linux-gnueabi-gcc -marm -march=armv4t -mfloat-abi=soft -Os \
  -ffreestanding -fno-builtin -fno-stack-protector -nostdlib -static \
  -o /build/probe /build/probe.c
cp "$CH/build/probe" "$OUT/probe"

# Initramfs: /dev/console plus the probe as /init.
cat > "$CH/build/initramfs.list" <<'EOF'
dir /dev 0755 0 0
nod /dev/console 0600 0 0 c 5 1
file /init /build/probe 0755 0 0
EOF
chroot "$CH" bash -c "cd /build/linux-2.6.28 && ./usr/gen_init_cpio /build/initramfs.list | gzip -9 > /build/probe.cpio.gz"
cp "$CH/build/probe.cpio.gz" "$OUT/probe.cpio.gz"
ls -la "$OUT"
