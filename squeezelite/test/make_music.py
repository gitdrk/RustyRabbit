#!/usr/bin/env python3
# 30 s stereo 44.1 kHz "music-like" test signal for the rabbit's speaker: soft
# arpeggiated chords with a few harmonics and a little noise, so decoders get
# realistic work and the speaker test isn't a harsh sweep. Peak about -12 dBFS.
import math, random, struct, wave, sys
w = wave.open(sys.argv[1], 'wb'); w.setnchannels(2); w.setsampwidth(2); w.setframerate(44100)
SR, SECS = 44100, 30
chords = [[261.6, 329.6, 392.0], [220.0, 261.6, 329.6], [174.6, 220.0, 261.6], [196.0, 246.9, 293.7]]
random.seed(1)
fr = bytearray()
for i in range(SR * SECS):
    t = i / SR
    beat = int(t * 4)                 # four notes a second
    ch = chords[(beat // 8) % 4]
    f = ch[beat % 3] * (2 if beat % 6 >= 3 else 1)
    tn = t - beat / 4.0               # time since note start
    env = math.exp(-tn * 6)
    note = env * (math.sin(2 * math.pi * f * t) + 0.3 * math.sin(4 * math.pi * f * t) + 0.1 * math.sin(6 * math.pi * f * t))
    pad = 0.25 * sum(math.sin(2 * math.pi * x * t / 2) for x in ch) / 3
    n = random.uniform(-1, 1) * 0.02
    l = 5500 * (note * 0.8 + pad) + 300 * n
    r = 5500 * (note * 0.6 + pad * 1.1) + 300 * n
    fr += struct.pack('<hh', int(l), int(r))
w.writeframes(bytes(fr)); w.close()
