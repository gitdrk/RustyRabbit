#!/bin/bash
# Builds karotzd for the rabbit (ARMv4T, static musl) and copies it to sim/out.
set -euo pipefail
. "$HOME/.cargo/env"
T=armv4t-karotz-linux-musleabi
SPEC=/mnt/c/Code/Karotz/sim/rust/$T.json
OUT=/mnt/c/Code/Karotz/sim/out
export CARGO_TARGET_DIR=/root/karotzd-target
# Any C code (libunwind, compiler-builtins C parts) must also be ARMv4T.
export CC_armv4t_karotz_linux_musleabi=arm-linux-gnueabi-gcc
export CXX_armv4t_karotz_linux_musleabi=arm-linux-gnueabi-g++
export CFLAGS_armv4t_karotz_linux_musleabi="-march=armv4t -marm -mfloat-abi=soft"
export CXXFLAGS_armv4t_karotz_linux_musleabi="-march=armv4t -marm -mfloat-abi=soft"
cd /mnt/c/Code/Karotz/karotzd
cargo +nightly build --release --target "$SPEC" \
  -Zbuild-std=std,panic_abort -Zjson-target-spec "$@"
BIN=$CARGO_TARGET_DIR/$T/release/karotzd
cp "$BIN" "$OUT/karotzd"
# Unstripped twin for scan-isa.sh (mapping symbols separate code from data).
CARGO_TARGET_DIR=/root/karotzd-target-dbg CARGO_PROFILE_RELEASE_STRIP=false \
  cargo +nightly build -q --release --target "$SPEC" \
  -Zbuild-std=std,panic_abort -Zjson-target-spec "$@"
ls -la "$OUT/karotzd"
file "$OUT/karotzd" 2>/dev/null || true
arm-linux-gnueabi-readelf -A "$OUT/karotzd" | grep -E 'CPU_arch|THUMB|ARM_ISA' || true
