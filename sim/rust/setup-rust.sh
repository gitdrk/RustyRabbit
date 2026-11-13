#!/bin/bash
# One-time setup in WSL (as root): ARM cross gcc for building musl and linking,
# plus nightly Rust with rust-src (needed for -Zbuild-std on a custom target).
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq gcc-arm-linux-gnueabi binutils-arm-linux-gnueabi make curl ca-certificates >/dev/null
arm-linux-gnueabi-gcc --version | head -1

if [ ! -x "$HOME/.cargo/bin/rustup" ]; then
  curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none
fi
. "$HOME/.cargo/env"
rustup toolchain install nightly --profile minimal --component rust-src
rustc +nightly --version
