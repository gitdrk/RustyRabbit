#!/bin/bash
# Tests karotzd in both emulation layers:
#  1. qemu-arm -cpu ti925t (ARMv4T core): any ARMv5+ instruction that runs => SIGILL
#  2. Linux 2.6.28 on qemu-system-arm: karotzd is PID 1, runs its self-test, powers off
set -uo pipefail
SIM=/mnt/c/Code/Karotz/sim
OUT=$SIM/out
BIN=$OUT/karotzd
UNSTRIPPED=/root/karotzd-target-dbg/armv4t-karotz-linux-musleabi/release/karotzd

echo "######## 0. static instruction scan ########"
[ -f "$UNSTRIPPED" ] && bash "$SIM/rust/scan-isa.sh" "$UNSTRIPPED"

echo
echo "######## 1. qemu-arm -cpu ti925t (ARMv4T) ########"
# Note: "spawn child" can fail here with "Exec format error": user-mode QEMU only
# emulates this process, and the host can't exec an ARM child without binfmt_misc.
# Layer 2 tests spawning on the real 2.6.28 kernel.
(cd /tmp && timeout 60 qemu-arm -cpu ti925t "$BIN" selftest)
echo "exit status: $?"

echo
echo "######## 2. Linux 2.6.28 (versatilepb), karotzd as /init ########"
CH=/opt/trusty
cat > "$CH/build/karotzd.list" <<EOF
dir /dev 0755 0 0
nod /dev/console 0600 0 0 c 5 1
nod /dev/urandom 0644 0 0 c 1 9
nod /dev/null 0666 0 0 c 1 3
dir /proc 0555 0 0
dir /tmp 1777 0 0
file /init $BIN 0755 0 0
EOF
"$CH/build/linux-2.6.28/usr/gen_init_cpio" "$CH/build/karotzd.list" | gzip -9 > "$OUT/karotzd.cpio.gz"
timeout 120 qemu-system-arm -M versatilepb -cpu arm926 -m 64 \
  -kernel "$OUT/zImage-2.6.28" -initrd "$OUT/karotzd.cpio.gz" \
  -append "console=ttyAMA0 rdinit=/init" -nographic -no-reboot 2>&1 \
  | tr -d '\r' | tee "$OUT/karotzd-system-run.log" \
  | sed -n '/Linux version/p;/KAROTZD SELFTEST BEGIN/,/KAROTZD SELFTEST END/p;/Kernel panic/p;/Illegal/p;/Segmentation/p'
