#!/usr/bin/env python3
# Compares squeezelite's 16-bit stdout PCM with a reference 16-bit WAV decode.
# Aligns by cross-correlation on a 1 s window, reports SNR in dB per channel.
import sys, wave, array, math
ref = wave.open(sys.argv[1]); r = array.array('h', ref.readframes(ref.getnframes()))
o = array.array('h', open(sys.argv[2], 'rb').read()[: (len(open(sys.argv[2], 'rb').read()) // 4) * 4])
def drop_silence(x, minrun=32):
    # stdout output emits silence whenever its buffer runs dry (it isn't paced
    # by a sound card). Remove runs of >= minrun all-zero frames; keep the
    # signal's own isolated zero crossings so the timing stays intact.
    y = array.array('h'); run = array.array('h')
    for i in range(0, len(x) - 1, 2):
        if x[i] or x[i + 1]:
            if 0 < len(run) // 2 < minrun: y.extend(run)
            run = array.array('h'); y.append(x[i]); y.append(x[i + 1])
        else:
            run.append(0); run.append(0)
    return y
print('frames: reference %d, squeezelite %d (incl. idle silence)' % (len(r) // 2, len(o) // 2))
o = drop_silence(o)
print('non-silent frames: reference %d, squeezelite %d' % (len(r) // 2, len(o) // 2))
def best_lag(a, b, maxlag=3000, n=4410, start=88200):
    best = (None, -1)
    for lag in range(-maxlag, maxlag + 1, 1):
        s = 0
        for i in range(start, start + n, 3):
            j = i + lag
            if 0 <= j < len(b) // 2: s += a[2 * i] * b[2 * j] + a[2 * i + 1] * b[2 * j + 1]
        if s > best[1]: best = (lag, s)
    return best[0]
lag = best_lag(r, o) if len(o) > 200000 else 0
for ch in (0, 1):
    sig = err = 0.0; n = 0
    for i in range(44100, len(r) // 2 - 44100):
        j = i + lag
        if not 0 <= j < len(o) // 2: continue
        a, b = r[2 * i + ch], o[2 * j + ch]
        sig += a * a; err += (a - b) ** 2; n += 1
    snr = 99.0 if err == 0 else 10 * math.log10(sig / err)
    print('channel %d: lag %d frames, SNR %.1f dB over %d frames%s' % (ch, lag, snr, n, ' (bit-exact)' if err == 0 else ''))
