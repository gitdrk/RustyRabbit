import time, urllib.request, sys, os
# Records karotzd's event stream to a file on this PC, so events survive a rabbit reboot.
# Stops on its own after `minutes` (default 30) or when the stream drops.
HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, 'button_events.log')
minutes = float(sys.argv[1]) if len(sys.argv) > 1 else 30
end = time.time() + minutes * 60
with open(OUT, 'a', buffering=1) as f:
    f.write('--- recording started %s\n' % time.strftime('%H:%M:%S'))
    try:
        r = urllib.request.urlopen('http://192.168.1.233:8080/api/events/stream', timeout=30)
        while time.time() < end:
            line = r.readline()
            if not line:
                f.write('--- stream closed by rabbit %s\n' % time.strftime('%H:%M:%S'))
                break
            line = line.decode('latin1').rstrip()
            if line.startswith('data:'):
                f.write('%s %s\n' % (time.strftime('%H:%M:%S'), line[5:].strip()))
    except Exception as e:
        f.write('--- stream ended %s: %s\n' % (time.strftime('%H:%M:%S'), e))
    f.write('--- recording stopped %s\n' % time.strftime('%H:%M:%S'))
