#!/usr/bin/env python3
"""Minimal stand-in for Lyrion Music Server, for testing squeezelite only.
Accepts one player on TCP 3483, enables output at unity gain, tells it to
stream one file over HTTP, and waits until the player reports the track has
fully played (STMu/STMd), then disconnects so squeezelite exits.
Usage: fake_lms.py <format f|m|o|p> <http_port> <path> [timeout_s]"""
import socket, struct, sys, time

fmt, http_port, path = sys.argv[1], int(sys.argv[2]), sys.argv[3]
timeout = float(sys.argv[4]) if len(sys.argv) > 4 else 120

def send(c, op, payload=b''):
    data = op + payload
    c.sendall(struct.pack('>H', len(data)) + data)

def read_msg(c):
    hdr = b''
    while len(hdr) < 8:
        d = c.recv(8 - len(hdr))
        if not d: return None, None
        hdr += d
    op, n = hdr[:4], struct.unpack('>I', hdr[4:])[0]
    body = b''
    while len(body) < n:
        d = c.recv(n - len(body))
        if not d: return None, None
        body += d
    return op, body

srv = socket.socket(); srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind(('0.0.0.0', 3483)); srv.listen(1); srv.settimeout(timeout)
c, addr = srv.accept(); c.settimeout(timeout)
t0 = time.time()
op, body = read_msg(c)
print('player says', op, body[36:].decode('latin1', 'replace')[:120], flush=True)
send(c, b'aude', bytes([1, 1]))
send(c, b'audg', struct.pack('>IIBBII', 0, 0, 1, 255, 0x10000, 0x10000))
req = ('GET %s HTTP/1.0\r\n\r\n' % path).encode()
strm = struct.pack('>cBBBBBBBBBBBBBIHI', b's', ord('1'), ord(fmt), ord('?'), ord('?'), ord('?'), ord('?'),
                   32, 0, 0, ord('0'), 0, 0, 0, 0, http_port, 0)   # server_ip 0 = same host as us
send(c, b'strm', strm + req)
seen = []
while time.time() - t0 < timeout:
    op, body = read_msg(c)
    if op is None: break
    if op == b'STAT':
        ev = body[:4].decode()
        if not seen or seen[-1] != ev: seen.append(ev)
        if ev == 'STMu':  # output underrun after decoder finished = track played to the end
            break
print('events', ' '.join(seen), 'in %.1f s' % (time.time() - t0), flush=True)
c.close()
