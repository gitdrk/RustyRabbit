#!/bin/bash
# Layer 1: squeezelite under qemu-arm -cpu ti925t (ARMv4T core), decoding MP3, FLAC,
# Ogg Vorbis and WAV from a fake LMS + HTTP server, PCM to stdout, compared with
# reference decodes made on the PC.
set -uo pipefail
export DEBIAN_FRONTEND=noninteractive
command -v flac >/dev/null && command -v lame >/dev/null && command -v oggenc >/dev/null || \
  apt-get install -y -qq flac lame vorbis-tools >/dev/null
T=/mnt/c/Code/Karotz/squeezelite/test
BIN=/mnt/c/Code/Karotz/squeezelite/out/squeezelite-karotz
W=/root/sqtest; mkdir -p $W; cd $W
python3 $T/make_tracks.py tone.wav
flac -s -f -o tone.flac tone.wav
lame --quiet -b 128 tone.wav tone.mp3
oggenc -Q -q 3 -o tone.ogg tone.wav
# Reference decodes (PC-side decoders) to compare with
cp tone.wav ref-flac.wav; lame --quiet --decode tone.mp3 ref-mp3.wav; oggdec -Q -o ref-ogg.wav tone.ogg
ls -la tone.*
python3 -m http.server 8099 --bind 127.0.0.1 >http.log 2>&1 & HTTP=$!
sleep 1
echo "== version/codecs"; qemu-arm -cpu ti925t $BIN -? 2>&1 | sed -n '1,3p;/Build options/,+1p;/known codecs/p'
for c in flac:f mp3:m ogg:o; do
  ext=${c%%:*}; f=${c##*:}
  echo "######## $ext ########"
  python3 $T/fake_lms.py $f 8099 /tone.$ext 120 & LMS=$!
  sleep 0.5
  ( cd $W; exec timeout 150 qemu-arm -cpu ti925t $BIN -s 127.0.0.1 -o - -a 16 -n test-$ext -d all=warn >out-$ext.raw 2>err-$ext.log ) & SL=$!
  wait $LMS                      # fake server returns once the track has played out
  sleep 0.5; kill $SL 2>/dev/null; wait $SL 2>/dev/null
  grep -iE 'illegal|segment|error|warn' err-$ext.log | head -5
  python3 $T/compare.py ref-$ext.wav out-$ext.raw
done
kill $HTTP
python3 - <<'PY'
import wave
for n in ('ref-mp3', 'ref-ogg'):
    w = wave.open(n + '.wav'); open(n + '.raw', 'wb').write(w.readframes(w.getnframes()))
PY
for e in mp3 ogg; do echo "## for scale: PC reference $e decoder vs original"; python3 $T/compare.py tone.wav ref-$e.raw | tail -2
echo "## squeezelite $e vs original"; python3 $T/compare.py tone.wav out-$e.raw | tail -2; done
