import time, urllib.request, urllib.parse, json, socket, os
# HTTP tests against karotzd (:8080, --readonly) vs the old CGI (:80) on the rabbit.
H = '192.168.1.233'

def get(path, port=8080, data=None, timeout=30, raw=False):
    t = time.time()
    try:
        req = urllib.request.Request('http://%s:%d%s' % (H, port, path), data=data)
        with urllib.request.urlopen(req, timeout=timeout) as r:
            body, code, ctype = r.read(), r.status, r.headers.get('Content-Type', '')
    except urllib.error.HTTPError as e:
        body, code, ctype = e.read(), e.code, e.headers.get('Content-Type', '')
    except Exception as e:
        body, code, ctype = str(e).encode(), 0, ''
    ms = (time.time() - t) * 1000
    return ms, code, ctype, (body if raw else body.decode('latin1').strip())

def line(label, ms, code, body, old=None):
    o = '' if old is None else ' (old %6.0f ms)' % old
    print('  %-30s %4d %6.0f ms%s  %s' % (label, code, ms, o, body[:110].replace('\n', ' | ')))

print('=== read-only routes: karotzd vs old CGI')
for p in ['/cgi-bin/status', '/cgi-bin/get_version', '/cgi-bin/get_free_space', '/cgi-bin/sound_list',
          '/cgi-bin/snapshot_list', '/cgi-bin/rfid_list_ext', '/cgi-bin/display_cache', '/cgi-bin/radios_list',
          '/cgi-bin/moods_list', '/cgi-bin/voice_list', '/cgi-bin/tools_log', '/cgi-bin/tools_ps', '/cgi-bin/status?format=xml']:
    ms, code, _, body = get(p)
    oms, ocode, _, obody = get(p, 80)
    line(p.replace('/cgi-bin/', ''), ms, code, body, oms)
    if p.endswith('status'):
        try:
            n, o = json.loads(body), json.loads(obody)
            diff = {k: (o.get(k), n.get(k)) for k in set(o) | set(n) if o.get(k) != n.get(k)}
            print('    status fields that differ old->new:', diff or 'none')
        except Exception as e:
            print('    status compare failed:', e, obody[:80])

print('=== guards (karotzd is --readonly) and blocked scripts')
for p in ['/cgi-bin/sleep', '/cgi-bin/snapshot', '/cgi-bin/clear_cache', '/cgi-bin/rfid_start_record',
          '/cgi-bin/cmd?cmd=id', '/cgi-bin/get_file?src=x&dest=y', '/cgi-bin/sound_control?cmd=run%20id']:
    ms, code, _, body = get(p)
    line(p.replace('/cgi-bin/', ''), ms, code, body)

print('=== POST form body + validation')
ms, code, _, body = get('/cgi-bin/rfid_info', data=b'tag=nosuchtag')
line('rfid_info (POST)', ms, code, body)
ms, code, _, body = get('/cgi-bin/rfid_test_url?url=http://127.0.0.1/cgi-bin/get_version')
line('rfid_test_url (local)', ms, code, body)

print('=== hardware')
ms, code, _, body = get('/cgi-bin/ears_random?noreset=1'); line('ears_random', ms, code, body)
time.sleep(3)
ms, code, _, body = get('/cgi-bin/apps/moods?id=1'); line('apps/moods id=1', ms, code, body)
time.sleep(4)
ms, code, ctype, jpg = get('/cgi-bin/snapshot_view?silent=1', raw=True)
print('  %-30s %4d %6.0f ms  %s, %d bytes, JPEG=%s' % ('snapshot_view', code, ms, ctype, len(jpg), jpg[:2] == b'\xff\xd8'))
if jpg[:2] == b'\xff\xd8':
    open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'karotzd-photo.jpg'), 'wb').write(jpg)

print('=== live webcam stream (6 s)')
try:
    s = socket.create_connection((H, 8080), 10)
    s.send(b'GET /cgi-bin/webcam HTTP/1.0\r\n\r\n')
    s.settimeout(15)
    t0, buf = time.time(), b''
    while time.time() - t0 < 6:
        try:
            d = s.recv(65536)
        except socket.timeout:
            break
        if not d:
            break
        buf += d
    s.close()
    head = buf[:buf.find(b'\r\n\r\n')].decode('latin1')
    frames = buf.count(b'\xff\xd8\xff')
    print('  header:', head.split('\r\n')[0], '|', [h for h in head.split('\r\n') if 'Content-Type' in h])
    print('  %d bytes, %d JPEG frames in 6 s' % (len(buf), frames))
except Exception as e:
    print('  stream failed:', e)
time.sleep(2)
ms, code, ctype, jpg = get('/cgi-bin/snapshot_view?silent=1', raw=True)
print('  %-30s %4d %6.0f ms  %d bytes, JPEG=%s' % ('snapshot after stream', code, ms, len(jpg), jpg[:2] == b'\xff\xd8'))

print('=== pages')
ms, code, ctype, body = get('/'); line('new UI /', ms, code, '%s, %d bytes, title ok=%s' % (ctype, len(body), '<title>Karotz</title>' in body))
ms, code, ctype, body = get('/index.html'); line('old UI /index.html (proxied)', ms, code, '%s, %d bytes' % (ctype, len(body)))
ms, code, _, body = get('/cgi-bin/leds?color=000000&nomemory=1'); line('LED back to saved (off)', ms, code, body)
