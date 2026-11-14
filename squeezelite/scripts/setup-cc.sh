#!/bin/bash
# Installs the C toolchain pieces into /opt/musl-armv4t (WSL, root): the gcc wrapper
# armv4t-musl-gcc, its specs file, and librt_armv4t.a (Rust compiler_builtins built for
# ARMv4T, used instead of Ubuntu's libgcc.a, which is ARMv5T and uses `clz`).
# Needs sim/rust/build-musl.sh (musl, libunwind stub) done first.
set -euo pipefail
[ -f /opt/musl-armv4t/lib/libunwind.a ] || { echo 'run sim/rust/build-musl.sh first'; exit 1; }
. "$HOME/.cargo/env"
S=/mnt/c/Code/Karotz/squeezelite/scripts
P=/opt/musl-armv4t
sed 's/\r$//' $S/armv4t-gcc.specs > $P/lib/armv4t-gcc.specs
sed 's/\r$//' $S/armv4t-musl-gcc > $P/bin/armv4t-musl-gcc; chmod +x $P/bin/armv4t-musl-gcc
for t in ar ranlib strip nm objdump readelf; do ln -sf /usr/bin/arm-linux-gnueabi-$t $P/bin/armv4t-musl-$t; done
SPEC=/mnt/c/Code/Karotz/sim/rust/armv4t-karotz-linux-musleabi.json
export CARGO_TARGET_DIR=/root/rtlib-target
export CC_armv4t_karotz_linux_musleabi=arm-linux-gnueabi-gcc
export CFLAGS_armv4t_karotz_linux_musleabi="-march=armv4t -marm -mfloat-abi=soft"
cd /mnt/c/Code/Karotz/squeezelite/rtlib
cargo +nightly build -q --release --target "$SPEC" -Zbuild-std=core,compiler_builtins -Zjson-target-spec
cp $CARGO_TARGET_DIR/armv4t-karotz-linux-musleabi/release/librt_armv4t.a $P/lib/
# Sanity check: run the helpers on an ARMv4T core and scan for newer instructions.
T=$(mktemp -d)
$P/bin/armv4t-musl-gcc -O2 -o $T/rtlib-check /mnt/c/Code/Karotz/squeezelite/test/rtlib-check.c
GOT=$(qemu-arm -cpu ti925t $T/rtlib-check 100 12345678901)
[ "$GOT" = "14 2 123456789 1 50.000000 123456789.010000 25" ] || { echo "rtlib check FAILED: $GOT"; exit 1; }
bash <(sed 's/\r$//' /mnt/c/Code/Karotz/sim/rust/scan-isa.sh) $T/rtlib-check | tail -1
rm -rf $T
echo "toolchain OK: $P/bin/armv4t-musl-gcc"
