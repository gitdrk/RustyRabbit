import socket, sys, time, binascii, hashlib, gzip, urllib.request, os
# Live event capture on the rabbit, in two phases so the user can act in between.
#   python run_live.py start    upload karotzd to /tmp (RAM), start `serve 8080 --readonly`,
#                               plus the stock dbus-monitor logging raw button/RFID signals
#   python run_live.py collect  print captured events, stop both, delete, verify flash unchanged
# Nothing stops these automatically (a nohup'd watchdog didn't survive logout), so
# `collect` must be run; a reboot would also clear them, since it's all in RAM.
HOST = '192.168.1.233'
HERE = os.path.dirname(os.path.abspath(__file__))
STATE = os.path.join(HERE, 'run_live.before.txt')
FLASH = 'ls -laeR /usr | md5sum; ls -lae /usr/openkarotz/Run/; md5sum /usr/openkarotz/Run/*'

s = socket.create_connection((HOST, 23), 8)
time.sleep(1); s.recv(8192)
s.send(b'karotz\r\n'); time.sleep(1); s.recv(8192)
s.send(b'\r\n'); time.sleep(1.5); s.recv(8192); s.send(b'unset HISTFILE\n'); time.sleep(0.3); s.recv(8192)  # no history file on flash
s.settimeout(120)
s.send(b'stty -echo\n'); time.sleep(0.5); s.recv(65536)

def run(cmd):
    s.send((cmd + '; echo __E""ND__\n').encode())
    buf = b''
    while b'__END__' not in buf:
        d = s.recv(65536)
        if not d: break
        buf += d
    return buf.decode('latin1', 'replace').replace('__END__', '').replace('-bash-4.1# ', '').strip()

def get(path):
    try:
        return urllib.request.urlopen('http://%s:8080%s' % (HOST, path), timeout=15).read().decode('latin1').strip()
    except Exception as e:
        return 'ERROR %s' % e

if sys.argv[1] == 'start':
    pre = run('mount | grep " /var "; (netstat -ltn 2>/dev/null | grep -q :8080) && echo port-8080-busy || echo port-8080-free; '
              'which nohup gunzip; ls /tmp/karotzd* 2>&1')
    print(pre)
    if 'ramfs on /var' not in pre or 'port-8080-free' not in pre or 'gunzip' not in pre:
        print('precheck failed'); sys.exit(1)
    open(STATE, 'w').write(run(FLASH))
    data = open(os.path.join(HERE, '..', 'sim', 'out', 'karotzd'), 'rb').read()
    md5 = hashlib.md5(data).hexdigest()
    payload = gzip.compress(data, 9)
    t = time.time()
    s.send(b"cat > /tmp/karotzd.uu <<'UUEOF'\nbegin 644 x\n")
    chunk = []
    for i in range(0, len(payload), 45):
        chunk.append(binascii.b2a_uu(payload[i:i+45]))
        if len(chunk) == 40:
            s.sendall(b''.join(chunk)); chunk = []; time.sleep(0.005)
    s.sendall(b''.join(chunk) + b'`\nend\nUUEOF\n')
    run('uudecode -o /tmp/karotzd.gz /tmp/karotzd.uu; rm -f /tmp/karotzd.uu; gunzip -f /tmp/karotzd.gz; chmod 755 /tmp/karotzd')
    print('upload %.0f s' % (time.time() - t))
    if md5 not in run('md5sum /tmp/karotzd'):
        print('MD5 MISMATCH'); run('rm -f /tmp/karotzd*'); sys.exit(1)
    print(run("nohup /tmp/karotzd serve 8080 --readonly > /tmp/karotzd.log 2>&1 & echo $! > /tmp/karotzd.pid; "
              "nohup dbus-monitor --system \"type='signal',path='/com/mindscape/karotz/Button'\" "
              "\"type='signal',path='/com/mindscape/karotz/Rfid'\" > /tmp/karotzd.monitor 2>&1 & echo $! > /tmp/karotzd.monpid; "
              "sleep 2; cat /tmp/karotzd.log; ps | grep -E '[k]arotzd|[d]bus-monitor --system .type'"))
    s.send(b'stty echo\nexit\n'); s.close()
    time.sleep(2)
    print('after logout, events endpoint answers:', get('/api/events'))
else:
    print('=== karotzd events captured')
    print(get('/api/events?since=0').replace('},{', '},\n {'))
    print('=== karotzd log')
    print(run('cat /tmp/karotzd.log; grep VmRSS /proc/$(cat /tmp/karotzd.pid)/status'))
    print('=== raw dbus-monitor log (button/rfid paths)')
    print(run('cat /tmp/karotzd.monitor'))
    print('=== cleanup')
    print(run('kill $(cat /tmp/karotzd.pid) $(cat /tmp/karotzd.monpid) 2>/dev/null; sleep 1; '
              'rm -f /tmp/karotzd /tmp/karotzd.pid /tmp/karotzd.monpid /tmp/karotzd.monitor /tmp/karotzd.log /tmp/karotzd.uu /tmp/karotzd.gz; '
              "ls /tmp/karotzd* 2>&1; ps | grep -E '[k]arotzd|[d]bus-monitor --system .type' || echo no-test-processes; "
              'ps | grep -c "[d]bus_watcher"; cut -d" " -f1 /proc/uptime'))
    before = open(STATE).read() if os.path.exists(STATE) else ''
    import re
    norm = lambda t: [l for l in re.sub(r'\[[0-9;]*m', '', t).splitlines() if re.match(r'^[0-9a-f]{32}  |^[-dl]', l)]
    after = run(FLASH)
    print('flash (/usr listing + Run/) unchanged:', norm(before) == norm(after), '(compared %d fingerprint lines)' % len(norm(after)))
    s.send(b'stty echo\nexit\n'); s.close()
