#!/bin/bash
# Runs the syscall probe two ways:
#  1. Full system: real Linux 2.6.28 on QEMU versatilepb (answers "does the syscall exist?")
#  2. User mode: qemu-arm with an ARMv4T CPU model (answers "is every instruction ARMv4T?")
set -uo pipefail
OUT=/mnt/c/Code/Karotz/sim/out

echo "######## 1. Linux 2.6.28 under qemu-system-arm (versatilepb) ########"
timeout 60 qemu-system-arm -M versatilepb -cpu arm926 -m 64 \
  -kernel "$OUT/zImage-2.6.28" -initrd "$OUT/probe.cpio.gz" \
  -append "console=ttyAMA0 rdinit=/init" -nographic -no-reboot 2>&1 \
  | tr -d '\r' | tee "$OUT/system-run.log" \
  | sed -n '/Linux version/p;/KAROTZ PROBE BEGIN/,/KAROTZ PROBE END/p;/Kernel panic/p'

echo
echo "######## 2. qemu-arm user mode, ARMv4T CPU model ########"
CPUS=$(qemu-arm -cpu help 2>&1)   # captured first: grep -q + pipefail misreports a match
CPU=""
for c in arm920t ti925t; do
  if grep -qw "$c" <<<"$CPUS"; then CPU=$c; break; fi
done
if [ -n "$CPU" ]; then
  echo "Using -cpu $CPU"
  qemu-arm -cpu "$CPU" "$OUT/probe" 2>&1 | sed -n '/kuser/p;/MISSING/p;/END/p'
  echo "exit status: ${PIPESTATUS[0]}"
else
  echo "No ARMv4T CPU model in this QEMU build"
fi
