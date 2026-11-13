import socket, sys, time, binascii, os, urllib.request
# Times the Karotz camera without touching flash: calls the webcam daemon's
# takeSnapshot directly (it writes /tmp/picture.jpg, RAM), times the thumbnail
# step into /tmp, copies one picture back, then deletes everything.
HOST = '192.168.1.233'
HERE = os.path.dirname(os.path.abspath(__file__))
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

SNAPS = 'ls -la --full-time /usr/openkarotz/Snapshots/ | md5sum; ls /usr/openkarotz/Snapshots | wc -l'
before = run(SNAPS)
print('Snapshots/ files before:', before.split()[-1])
print(run('ls -la /tmp/picture.jpg 2>&1; cut -d" " -f1 /proc/uptime'))

SNAP = ('dbus-send --system --type=method_call --print-reply --dest=com.mindscape.karotz.Webcam '
        '/com/mindscape/karotz/Webcam com.mindscape.karotz.KarotzInterface.takeSnapshot string:1 string:"file" int32:%d')
print('=== takeSnapshot via dbus-send (what the snapshot CGI does), picture goes to /tmp')
print(run('TIMEFORMAT=%R'))
for i, q in enumerate([50, 50, 50, 50, 100, 10]):
    out = run('rm -f /tmp/picture.jpg; { time ' + (SNAP % q) + ' >/tmp/cam.reply 2>&1 ; } 2>&1; '
              'ls -l /tmp/picture.jpg 2>/dev/null | awk \'{print $5}\'; tr -s " \\n" " " < /tmp/cam.reply | cut -c1-120')
    print('  #%d int32:%-3d  %s' % (i + 1, q, ' | '.join(out.splitlines())))
    if i == 3:
        # dimensions + thumbnail step (CGI writes this to flash; here /tmp)
        print('  djpeg info :', run('/usr/bin/arm-linux-djpeg -verbose -outfile /dev/null /tmp/picture.jpg 2>&1 | grep -iE "start of frame|image" | head -2'))
        print('  thumbnail  :', run('{ time /usr/bin/arm-linux-djpeg -gif -scale 1/4 -outfile /tmp/cam.thumb.gif /tmp/picture.jpg ; } 2>&1; ls -l /tmp/cam.thumb.gif | awk \'{print $5" bytes"}\''))
        # copy this picture back to the PC
        uu = run('uuencode /tmp/picture.jpg pic.jpg')
        lines = [l for l in uu.splitlines() if l and not l.startswith('begin') and l not in ('`', 'end')]
        data = b''.join(binascii.a2b_uu(l) for l in lines)
        open(os.path.join(HERE, 'karotz-snapshot.jpg'), 'wb').write(data)
        print('  copied back: %d bytes, JPEG header ok: %s' % (len(data), data[:2] == b'\xff\xd8'))
    time.sleep(1)

print('=== same call from a back-to-back burst (3 in a row, no pause)')
print(run('for i in 1 2 3; do { time ' + (SNAP % 50) + ' >/dev/null 2>&1 ; } 2>&1; done | tr "\\n" " "'))

print('=== webcam events seen by the karotzd listener still running from earlier')
try:
    ev = urllib.request.urlopen('http://%s:8080/api/events?since=0' % HOST, timeout=10).read().decode()
    print(ev.replace('},{', '},\n {')[:3000])
except Exception as e:
    print('  karotzd not reachable:', e)

print('=== cleanup: camera files, and the karotzd listener + its broken watchdog')
print(run('rm -f /tmp/picture.jpg /tmp/cam.reply /tmp/cam.thumb.gif; '
          'kill $(cat /tmp/karotzd.pid) 2>/dev/null; kill $(cat /tmp/karotzd.watchdog) 2>/dev/null; sleep 1; '
          'rm -f /tmp/karotzd /tmp/karotzd.pid /tmp/karotzd.watchdog /tmp/karotzd.log; '
          'ls /tmp/karotzd* /tmp/picture.jpg /tmp/cam.* 2>&1; ps | grep -E "[k]arotzd|[s]leep 1200" || echo no-karotzd-processes'))
after = run(SNAPS)
print('Snapshots/ unchanged:', before == after)
print('Run/ led.color:', run('md5sum /usr/openkarotz/Run/led.color'))
s.send(b'stty echo\nexit\n'); s.close()
