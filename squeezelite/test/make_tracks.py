#!/usr/bin/env python3
# 10 s stereo 44.1 kHz test signal that never repeats (so alignment is unambiguous):
# left sweeps 200 Hz -> 4 kHz, right sweeps 3 kHz -> 300 Hz, about -6 dBFS.
import math, struct, wave, sys
w = wave.open(sys.argv[1], 'wb'); w.setnchannels(2); w.setsampwidth(2); w.setframerate(44100)
N, T = 441000, 10.0
def sweep(f0, f1, t): return 2 * math.pi * (f0 * t + (f1 - f0) * t * t / (2 * T))
fr = bytearray()
for i in range(N):
    t = i / 44100.0
    fr += struct.pack('<hh', int(16000 * math.sin(sweep(200, 4000, t))), int(16000 * math.sin(sweep(3000, 300, t))))
w.writeframes(bytes(fr)); w.close()
