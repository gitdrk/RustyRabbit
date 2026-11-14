#!/usr/bin/env python3
# Same rule as vmharness.c: drop leading all-zero frames and any run of >= 32
# all-zero frames, FNV-1a hash the rest. Accepts .wav or raw 16-bit stereo PCM.
import sys, wave

for p in sys.argv[1:]:
    if p.endswith('.wav'):
        w = wave.open(p); d = w.readframes(w.getnframes())
    else:
        d = open(p, 'rb').read()
    h, kept, zrun, started = 2166136261, 0, 0, False
    for i in range(0, len(d) - 3, 4):
        fr = d[i:i + 4]
        if fr == b'\0\0\0\0':
            zrun += 1
            continue
        if started and 0 < zrun < 32:
            for _ in range(zrun * 4):
                h = (h * 16777619) & 0xffffffff
            kept += zrun
        zrun = 0
        started = True
        for b in fr:
            h = ((h ^ b) * 16777619) & 0xffffffff
        kept += 1
    print('%-28s frames=%d fnv=%08x' % (p, kept, h))
