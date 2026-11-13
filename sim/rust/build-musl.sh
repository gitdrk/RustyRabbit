#!/bin/bash
# Builds musl libc as a static library for ARMv4T, soft-float, ARM (not Thumb) mode.
# musl supports ARMv4T and uses the kernel's kuser helpers for atomics and TLS
# on CPUs older than ARMv6, which is what the rabbit's ARM920T needs.
set -euo pipefail
command -v cc >/dev/null || { apt-get install -y -qq gcc libc6-dev >/dev/null; }
MUSL_VER=1.2.5
PREFIX=/opt/musl-armv4t
SRC=/root/build/musl-$MUSL_VER
mkdir -p /root/build
if [ ! -d "$SRC" ]; then
  curl -sSfL -o /root/build/musl.tgz https://musl.libc.org/releases/musl-$MUSL_VER.tar.gz
  tar -xzf /root/build/musl.tgz -C /root/build
fi
cd "$SRC"
make distclean >/dev/null 2>&1 || true
# No vDSO: Linux 2.6.28 on ARM has none (ARM got one in 4.1), so this code is dead
# on the rabbit. Removing it also stops qemu-arm's own Thumb vDSO from being called,
# whose `pop {pc}` return doesn't switch back to ARM state on an ARMv4T core.
sed -i '/^#define VDSO_/d' arch/arm/syscall_arch.h
CC=arm-linux-gnueabi-gcc AR=arm-linux-gnueabi-ar RANLIB=arm-linux-gnueabi-ranlib \
CFLAGS="-march=armv4t -marm -mfloat-abi=soft -Os" \
  ./configure --prefix=$PREFIX --target=arm-linux-musleabi --disable-shared \
  CROSS_COMPILE=arm-linux-gnueabi- >/dev/null
make -j"$(nproc)" >/root/build/musl-build.log 2>&1 || { tail -30 /root/build/musl-build.log; exit 1; }
make install >/dev/null
# Rust linker wrapper and the C++ compiler needed to build LLVM's libunwind.
mkdir -p $PREFIX/bin
sed 's/\r$//' /mnt/c/Code/Karotz/sim/rust/armv4t-musl-cc > $PREFIX/bin/armv4t-musl-cc
chmod +x $PREFIX/bin/armv4t-musl-cc
# Stub libunwind (karotzd uses panic=abort; see unwind-stub.c).
arm-linux-gnueabi-gcc -march=armv4t -marm -mfloat-abi=soft -Os -c \
  -o /root/build/unwind-stub.o /mnt/c/Code/Karotz/sim/rust/unwind-stub.c
rm -f $PREFIX/lib/libunwind.a
arm-linux-gnueabi-ar rcs $PREFIX/lib/libunwind.a /root/build/unwind-stub.o
ls -la $PREFIX/lib
