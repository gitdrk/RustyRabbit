#!/bin/bash
# Layer 2: squeezelite on a real Linux 2.6.28 kernel (qemu-system-arm versatilepb).
# vmharness is /init; it serves the test files, acts as LMS, and hashes the PCM.
# Run test-qemu.sh first (it makes the test files and the qemu-arm outputs).
set -uo pipefail
SIM=/mnt/c/Code/Karotz/sim
T=/mnt/c/Code/Karotz/squeezelite/test
BIN=/mnt/c/Code/Karotz/squeezelite/out/squeezelite-karotz
W=/root/sqtest
cd $W
/opt/musl-armv4t/bin/armv4t-musl-gcc -O2 -Wall -o vmharness $T/vmharness.c || exit 1
bash <(sed 's/\r$//' $SIM/rust/scan-isa.sh) vmharness | tail -1
cat > vm.list <<EOF
dir /dev 0755 0 0
nod /dev/console 0600 0 0 c 5 1
nod /dev/null 0666 0 0 c 1 3
nod /dev/urandom 0644 0 0 c 1 9
dir /proc 0555 0 0
dir /tmp 1777 0 0
file /init $W/vmharness 0755 0 0
file /squeezelite $BIN 0755 0 0
file /tone.flac $W/tone.flac 0644 0 0
file /tone.mp3 $W/tone.mp3 0644 0 0
file /tone.ogg $W/tone.ogg 0644 0 0
EOF
/opt/trusty/build/linux-2.6.28/usr/gen_init_cpio vm.list | gzip -9 > vm.cpio.gz
ls -la vm.cpio.gz
timeout 900 qemu-system-arm -M versatilepb -cpu arm926 -m 64 \
  -kernel $SIM/out/zImage-2.6.28 -initrd vm.cpio.gz \
  -append "console=ttyAMA0 rdinit=/init panic=1" -nographic -no-reboot 2>&1 \
  | tr -d '\r' | tee vm-run.log \
  | sed -n '/Linux version/p;/VMTEST/p;/RESULT/p;/Kernel panic/p;/Illegal/p;/Segmentation/p;/squeezelite/p;/WARN/p'
echo "== same hash on the PC side (original WAV, qemu-arm outputs)"
python3 $T/hash_raw.py tone.wav out-flac.raw out-mp3.raw out-ogg.raw
