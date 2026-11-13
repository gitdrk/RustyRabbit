#!/bin/bash
# Negative control for the ARMv4T instruction check.
cp /mnt/c/Code/Karotz/sim/v5-negative-test.c /opt/trusty/build/
chroot /opt/trusty arm-linux-gnueabi-gcc -marm -march=armv5te -nostdlib -static -O1 \
  -o /build/v5test /build/v5-negative-test.c
for c in arm926 ti925t; do
  qemu-arm -cpu "$c" /opt/trusty/build/v5test 2>&1 | head -2
  echo "-cpu $c -> exit ${PIPESTATUS[0]}"
done
