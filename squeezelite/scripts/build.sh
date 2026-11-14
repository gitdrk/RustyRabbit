#!/bin/bash
# Builds a static squeezelite for the Karotz (ARMv4T, soft-float, musl) in WSL as root.
# Codecs: FLAC, MP3 (libmad, fixed-point), Ogg Vorbis (Tremor, integer), PCM/WAV/AIFF.
# Output: PortAudio v19 with only the OSS backend (/dev/dsp).
# Needs sim/rust/setup-rust.sh and sim/rust/build-musl.sh run once (the musl prefix and
# libunwind stub in /opt/musl-armv4t). Runs setup-cc.sh and fetch.sh itself.
# Result: squeezelite/out/squeezelite-karotz (stripped) + .unstripped, ISA-scanned.
set -euo pipefail
S=/mnt/c/Code/Karotz/squeezelite/scripts
bash $S/setup-cc.sh
bash $S/fetch.sh
export DEBIAN_FRONTEND=noninteractive
command -v autoreconf >/dev/null && command -v libtoolize >/dev/null && [ -x /usr/bin/pkg-config ] || \
  apt-get install -y -qq autoconf automake libtool pkgconf >/dev/null
SRC=/mnt/c/Code/Karotz/squeezelite/sources
B=/root/sqbuild
P=$B/prefix
OUT=/mnt/c/Code/Karotz/squeezelite/out
export PATH=/opt/musl-armv4t/bin:/usr/bin:/bin
export CC=armv4t-musl-gcc AR=armv4t-musl-ar RANLIB=armv4t-musl-ranlib STRIP=armv4t-musl-strip
export CFLAGS="-O2 -I$P/include" LDFLAGS="-L$P/lib"
export PKG_CONFIG=/usr/bin/pkg-config PKG_CONFIG_LIBDIR=$P/lib/pkgconfig PKG_CONFIG_PATH=
HOSTOPT="--host=arm-linux-gnueabi --prefix=$P --disable-shared --enable-static"
mkdir -p $B $P/include $OUT
log() { echo "== $*"; }
step() { local n=$1; shift; "$@" >$B/$n.log 2>&1 || { echo "FAILED: $n"; tail -40 $B/$n.log; exit 1; }; }

# Kernel headers for OSS only (musl ships no sys/soundcard.h). Copy just what it needs.
K=/usr/arm-linux-gnueabi/include
mkdir -p $P/include/sys $P/include/linux $P/include/asm $P/include/asm-generic
cp $K/linux/soundcard.h $K/linux/patchkey.h $K/linux/ioctl.h $P/include/linux/
cp $K/asm/ioctl.h $P/include/asm/; cp $K/asm-generic/ioctl.h $P/include/asm-generic/
echo '#include <linux/soundcard.h>' > $P/include/sys/soundcard.h

unpack() { rm -rf $B/$2; mkdir -p $B/$2; tar -xf $SRC/$1 -C $B/$2 --strip-components=1; }

log libogg
unpack libogg-1.3.5.tar.gz ogg; cd $B/ogg
step ogg-conf ./configure $HOSTOPT; step ogg-make make -j16 install

log tremor
rm -rf $B/tremor; cp -r $SRC/tremor $B/tremor; rm -rf $B/tremor/.git; cd $B/tremor
step tremor-autogen autoreconf -fi -I $P/share/aclocal
step tremor-conf ./configure $HOSTOPT --with-ogg=$P
step tremor-make make -j16 install

log libmad
unpack libmad-0.15.1b.tar.gz mad; cd $B/mad
sed -i 's/-fforce-mem//g; s/-fthread-jumps//g; s/-fcse-follow-jumps//g; s/-fcse-skip-blocks//g; s/-fregmove//g; s/-fschedule-insns2//g; s/-fstrength-reduce//g' configure
step mad-conf ./configure $HOSTOPT --enable-fpm=arm --disable-debugging
step mad-make make -j16 install

log flac
unpack flac-1.4.3.tar.xz flac; cd $B/flac
step flac-conf ./configure $HOSTOPT --disable-ogg --disable-cpplibs --disable-programs --disable-examples \
  --disable-doxygen-docs --disable-xmms-plugin --disable-asm-optimizations --disable-stack-smash-protection \
  --disable-oggtest --disable-thorough-tests --disable-version-from-git
step flac-make make -j16 -C src/libFLAC install
step flac-hdr make -C include install

log portaudio
unpack portaudio-19.7.0.tar.gz pa; cd $B/pa
step pa-conf ./configure $HOSTOPT --with-oss --without-alsa --without-jack --without-asihpi --without-winapi
grep -E '^(OTHER_OBJS|DLL_LIBS|CFLAGS) *=' Makefile | sed 's/^/   /'
step pa-make make -j16 lib/libportaudio.la
step pa-inst make install

log squeezelite
rm -rf $B/sl; cp -r $SRC/squeezelite $B/sl; rm -rf $B/sl/.git; cd $B/sl
SLSRC="main.c slimproto.c buffer.c stream.c utils.c output.c output_alsa.c output_pa.c output_stdout.c output_pack.c decode.c flac.c pcm.c vorbis.c mad.c"
OPTS="-DLINKALL -DPORTAUDIO -DTREMOR_ONLY -DNO_FAAD -DNO_MPG123"
step sl-build $CC -O2 -fcommon -std=gnu99 -Wall -I$P/include $OPTS $SLSRC -o squeezelite-karotz \
  -L$P/lib -lportaudio -lFLAC -lvorbisidec -logg -lmad -lpthread -lm -lrt
$STRIP -o $OUT/squeezelite-karotz squeezelite-karotz
cp squeezelite-karotz $OUT/squeezelite-karotz.unstripped
ls -la $OUT
file $OUT/squeezelite-karotz
arm-linux-gnueabi-readelf -A $OUT/squeezelite-karotz | grep -E 'CPU_arch|THUMB|ARM_ISA|FP' || true
log ISA scan
bash <(sed 's/\r$//' /mnt/c/Code/Karotz/sim/rust/scan-isa.sh) $OUT/squeezelite-karotz.unstripped
