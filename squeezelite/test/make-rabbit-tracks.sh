#!/bin/bash
# Makes the 30 s tracks for the live rabbit test into /mnt/c/Code/Karotz/squeezelite/test/rabbit-tracks
set -euo pipefail
T=/mnt/c/Code/Karotz/squeezelite/test
O=$T/rabbit-tracks
mkdir -p $O
cd /root/sqtest
python3 $T/make_music.py music.wav
flac -s -f -5 music.wav -o $O/music.flac
lame --quiet -b 320 music.wav $O/music320.mp3
lame --quiet -b 128 music.wav $O/music128.mp3
ls -la $O
