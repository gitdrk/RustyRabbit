#!/usr/bin/env python3
"""RAM-only live test of squeezelite on the rabbit. Everything on the rabbit lives in /tmp
(ramfs) and is deleted at the end; a flash fingerprint is compared before/after.
The PC (this script) is the music server: slimproto on 3483 and HTTP on 8099.
Usage: python rabbit_live.py [gain_hex]   (default gain 0x8000 = -6 dB in squeezelite)"""
import binascii, gzip, hashlib, http.server, os, re, socket, socketserver, struct, sys, threading, time

RABBIT, PC = '192.168.1.233', '192.168.1.72'
HERE = os.path.dirname(os.path.abspath(__file__))
TRACKS = os.path.join(HERE, 'rabbit-tracks')
BIN = os.path.join(HERE, '..', 'out', 'squeezelite-karotz')
GAIN = int(sys.argv[1], 16) if len(sys.argv) > 1 else 0x8000
PLAYLIST = [('f', '/music.flac'), ('m', '/music320.mp3'), ('m', '/music128.mp3')]
FLASH = 'ls -laeR /usr | md5sum; md5sum /usr/openkarotz/Run/* 2>/dev/null'
http_hits = []

# ---------- PC side: HTTP + slimproto ----------
class H(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *a, **k): super().__init__(*a, directory=TRACKS, **k)
    def log_message(self, fmt, *a): http_hits.append('%s %s' % (self.client_address[0], fmt % a))
class TS(socketserver.ThreadingMixIn, http.server.HTTPServer): daemon_threads = True; allow_reuse_address = True

def send(c, op, payload=b''):
    d = op + payload
    c.sendall(struct.pack('>H', len(d)) + d)

def read_msg(c):
    def rd(n):
        b = b''
        while len(b) < n:
            x = c.recv(n - len(b))
            if not x: return None
            b += x
        return b
    h = rd(8)
    if not h: return None, None
    return h[:4], rd(struct.unpack('>I', h[4:])[0]) or b''

lms_log = []        # (time, text)
lms_done = threading.Event()
track_times = []    # (name, start, end, events)

def lms():
    srv = socket.socket(); srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(('0.0.0.0', 3483)); srv.listen(1); srv.settimeout(300)
    try:
        c, addr = srv.accept()
    except socket.timeout:
        lms_log.append((time.time(), 'no player connected')); lms_done.set(); return
    c.settimeout(120)
    op, body = read_msg(c)
    lms_log.append((time.time(), 'player %s says %s' % (addr[0], op)))
    send(c, b'aude', bytes([1, 1]))
    send(c, b'audg', struct.pack('>IIBBII', 0, 0, 1, 255, GAIN, GAIN))
    for fmt, path in PLAYLIST:
        req = ('GET %s HTTP/1.0\r\n\r\n' % path).encode()
        strm = struct.pack('>cBBBBBBBBBBBBBIHI', b's', ord('1'), ord(fmt), ord('?'), ord('?'), ord('?'), ord('?'),
                           32, 0, 0, ord('0'), 0, 0, 0, 0, 8099, 0)
        t0 = time.time(); seen = []
        send(c, b'strm', strm + req)
        try:
            while time.time() - t0 < 100:
                op, body = read_msg(c)
                if op is None: break
                if op == b'STAT':
                    ev = body[:4].decode('latin1')
                    if ev == 'STMt': continue
                    seen.append('%s@%.1f' % (ev, time.time() - t0))
                    if ev == 'STMu': break
        except socket.timeout:
            seen.append('TIMEOUT')
        track_times.append((path, t0, time.time(), seen))
        time.sleep(2)
    c.close(); srv.close(); lms_done.set()

# ---------- rabbit side: telnet ----------
s = socket.create_connection((RABBIT, 23), 8)
time.sleep(1); s.recv(8192)
s.send(b'karotz\r\n'); time.sleep(1); s.recv(8192)
s.send(b'\r\n'); time.sleep(1.5); s.recv(8192); s.send(b'unset HISTFILE; set +H\n'); time.sleep(0.3); s.recv(8192)
s.settimeout(120)
s.send(b'stty -echo\n'); time.sleep(0.5); s.recv(65536)

def run(cmd):
    s.send((cmd + '; echo __E""ND__\n').encode()); buf = b''
    while b'__END__' not in buf:
        d = s.recv(65536)
        if not d: break
        buf += d
    return re.sub(r'\x1b\[[0-9;]*m', '', buf.decode('latin1', 'replace').replace('__END__', '').replace('-bash-4.1# ', '')).strip()

norm = lambda t: [l for l in t.splitlines() if re.match(r'^[0-9a-f]{32}  ', l)]
before = run(FLASH)
print('flash fingerprint lines:', len(norm(before)))
try:
    pre = run('ls /tmp/sl* 2>&1; cat /proc/asound/card0/pcm0p/sub0/status; fuser /dev/dsp 2>&1; echo dsp-check-done')
    print(pre)
    if 'closed' not in pre:
        raise SystemExit('audio device busy, not starting')
    hs = TS(('0.0.0.0', 8099), H); threading.Thread(target=hs.serve_forever, daemon=True).start()
    print('reach PC from rabbit:', run('echo -e "HEAD /music.flac HTTP/1.0\\r\\n\\r\\n" | nc %s 8099 | head -1' % PC))
    if not http_hits:
        raise SystemExit('rabbit cannot reach the PC on 8099 (Windows firewall?)')

    import shutil
    shutil.copyfile(BIN, os.path.join(TRACKS, 'sl-squeezelite'))
    md5 = hashlib.md5(open(BIN, 'rb').read()).hexdigest()
    t = time.time()
    print(run('wget -q -O /tmp/sl-squeezelite http://%s:8099/sl-squeezelite; chmod 755 /tmp/sl-squeezelite' % PC))
    print('download %.0f s' % (time.time() - t))
    if md5 not in run('md5sum /tmp/sl-squeezelite'):
        raise SystemExit('MD5 MISMATCH')
    print('--- squeezelite -?  (first lines)')
    print(run('/tmp/sl-squeezelite -? 2>&1 | head -3; /tmp/sl-squeezelite -t 2>&1 | head -2'))
    print('--- PortAudio device list')
    print(run('/tmp/sl-squeezelite -l 2>&1 | head -20'))

    threading.Thread(target=lms, daemon=True).start()
    time.sleep(0.5)
    pid = run('/tmp/sl-squeezelite -s %s -n karotz-test -m 00:11:22:33:44:56 -d all=info </dev/null >/tmp/sl.log 2>&1 & echo PID=$!' % PC); pid = re.search(r'PID=(\d+)', pid).group(1)
    print('squeezelite pid', pid)
    samples = []
    def sample():
        out = run('cat /proc/stat | head -1; cat /proc/%s/stat 2>/dev/null; grep -E "VmRSS" /proc/%s/status 2>/dev/null' % (pid, pid))
        cpu = [int(x) for x in re.search(r'^cpu\s+(.*)$', out, re.M).group(1).split()]
        m = re.search(r'^%s \(.*?\) \S+ (.*)$' % pid, out, re.M)
        if not m: return None
        f = m.group(1).split()
        rss = re.search(r'VmRSS:\s+(\d+)', out)
        return time.time(), sum(cpu), cpu[3], int(f[10]) + int(f[11]), rss and int(rss.group(1))
    top_snap = None
    while not lms_done.is_set():
        x = sample()
        if x is None: print('squeezelite process gone!'); break
        samples.append(x)
        if top_snap is None and len(samples) == 6:
            top_snap = run('top -b -n 1 | head -12')
        lms_done.wait(3)
    print('--- top during playback (about 15 s in)')
    print(top_snap)
    print('--- per-track results (slimproto events, seconds from track start)')
    for path, a, b, ev in track_times:
        win = [x for x in samples if a + 1 <= x[0] <= b - 1]
        if len(win) >= 2:
            dt = win[-1][1] - win[0][1]; sl = (win[-1][3] - win[0][3]) / dt * 100; idle = (win[-1][2] - win[0][2]) / dt * 100
            peak = max(((q[3] - p[3]) / max(1, q[1] - p[1]) * 100) for p, q in zip(win, win[1:]))
            print('%-14s %.1f s  squeezelite avg %.0f%% CPU (peak 3s window %.0f%%), system idle %.0f%%  events %s'
                  % (path, b - a, sl, peak, idle, ' '.join(ev)))
        else:
            print(path, ev)
    print('max VmRSS KB:', max((x[4] or 0) for x in samples) if samples else '?')
    print('--- squeezelite log (warnings/underruns)')
    print(run("grep -iE 'underrun|overrun|error|warn|fail|rate|device|open' /tmp/sl.log | head -30; wc -l < /tmp/sl.log"))
    print('LMS:', lms_log)
finally:
    print('=== cleanup')
    print(run('killall sl-squeezelite 2>/dev/null; sleep 1; killall -9 sl-squeezelite 2>/dev/null; '
              'rm -f /tmp/sl-squeezelite /tmp/sl.log /tmp/sl.uu /tmp/sl.gz /tmp/sl; ls /tmp/sl* 2>&1; '
              'ps | grep "[s]l-squeezelite" || echo no-test-process; cat /proc/asound/card0/pcm0p/sub0/status'))
    after = run(FLASH)
    print('flash fingerprint unchanged:', norm(before) == norm(after), '(%d lines)' % len(norm(after)))
    s.send(b'stty echo\nexit\n'); s.close()
    print('HTTP requests seen:', http_hits[:8])
    try: os.remove(os.path.join(TRACKS, 'sl-squeezelite'))
    except OSError: pass
