#!/bin/bash
# Downloads the pinned sources into squeezelite/sources (git-ignored). Tarballs are
# checked against SHA-256, git sources are fetched at a fixed commit. Nothing in
# there is executed by this script; build.sh unpacks and builds it.
set -euo pipefail
D=/mnt/c/Code/Karotz/squeezelite/sources
mkdir -p $D; cd $D

get() {  # url file sha256
  if [ ! -s "$2" ]; then curl -sSfL --retry 3 -o "$2.part" "$1"; mv "$2.part" "$2"; fi
  echo "$3  $2" | sha256sum -c --quiet - || { echo "SHA-256 mismatch for $2"; rm -f "$2"; exit 1; }
}
get https://github.com/PortAudio/portaudio/archive/refs/tags/v19.7.0.tar.gz portaudio-19.7.0.tar.gz \
    5af29ba58bbdbb7bbcefaaecc77ec8fc413f0db6f4c4e286c40c3e1b83174fa0
get https://downloads.sourceforge.net/project/mad/libmad/0.15.1b/libmad-0.15.1b.tar.gz libmad-0.15.1b.tar.gz \
    bbfac3ed6bfbc2823d3775ebb931087371e142bb0e9bb1bee51a76a6e0078690
get https://downloads.xiph.org/releases/flac/flac-1.4.3.tar.xz flac-1.4.3.tar.xz \
    6c58e69cd22348f441b861092b825e591d0b822e106de6eb0ee4d05d27205b70
get https://downloads.xiph.org/releases/ogg/libogg-1.3.5.tar.gz libogg-1.3.5.tar.gz \
    0eb4b4b9420a0f51db142ba3f9c64b333f826532dc0f48c6410ae51f4799b664

g() { git -c safe.directory='*' -c advice.detachedHead=false "$@"; }  # /mnt/c is owned by another uid
gitpin() {  # url dir commit
  if [ "$(g -C "$2" rev-parse HEAD 2>/dev/null)" != "$3" ]; then
    rm -rf "$2"; g init -q "$2"
    g -C "$2" fetch -q --depth 1 "$1" "$3"
    g -C "$2" checkout -q FETCH_HEAD
  fi
  echo "$2 at $(g -C "$2" rev-parse --short HEAD)"
}
gitpin https://gitlab.xiph.org/xiph/tremor.git tremor 820fb3237ea81af44c9cc468c8b4e20128e3e5ad
gitpin https://github.com/ralph-irving/squeezelite.git squeezelite cc7ee7c0d00bd2aa691a574a7d429d5cd13c858b
echo "sources OK"
