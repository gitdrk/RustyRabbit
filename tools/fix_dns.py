import socket, time, sys
# Fixes the rabbit's DNS (user-approved flash change), carefully:
#  1. back up /usr/scripts/waitfornetwork.sh and /usr/etc/resolv.conf (next to them, *.bak-dns)
#  2. try each candidate server in /etc/resolv.conf until a real lookup works
#     (if none works: restore resolv.conf from the backup and stop)
#  3. change ONLY the DNS="..." line in the boot script (via a copy in /tmp),
#     check the diff is exactly that one line and the script still parses, then install it.
CANDIDATES = ['192.168.1.1', '192.168.1.205']
SCRIPT, RESOLV = '/usr/scripts/waitfornetwork.sh', '/usr/etc/resolv.conf'

s = socket.create_connection(('192.168.1.233', 23), 8)
time.sleep(1); s.recv(8192)
s.send(b'karotz\r\n'); time.sleep(1); s.recv(8192)
s.send(b'\r\n'); time.sleep(1.5); s.recv(8192); s.send(b'unset HISTFILE\n'); time.sleep(0.3); s.recv(8192)
s.settimeout(60)
s.send(b'stty -echo\n'); time.sleep(0.5); s.recv(65536)

def run(cmd):
    s.send((cmd + '; echo __E""ND__\n').encode())
    buf = b''
    while b'__END__' not in buf:
        d = s.recv(65536)
        if not d: break
        buf += d
    return buf.decode('latin1', 'replace').replace('__END__', '').replace('-bash-4.1# ', '').strip()

def done():
    s.send(b'stty echo\nexit\n'); s.close()

print('== 1. backups')
print(run(f'cp -p {SCRIPT} {SCRIPT}.bak-dns && cp -p {RESOLV} {RESOLV}.bak-dns && '
          f'md5sum {SCRIPT} {SCRIPT}.bak-dns {RESOLV} {RESOLV}.bak-dns && cat {RESOLV}'))

print('== 2. try servers')
good = None
for d in CANDIDATES:
    out = run(f"echo 'nameserver {d}' > {RESOLV}; nslookup api.voicerss.org 2>&1 | tail -3; "
              f"nslookup example.com 2>&1 | grep -c Address")
    print(f'-- {d}:', ' | '.join(l for l in out.splitlines() if l.strip()))
    if "can't resolve" not in out and 'Address' in out:
        good = d
        break
if not good:
    print(run(f'cp -p {RESOLV}.bak-dns {RESOLV}; cat {RESOLV}; md5sum {RESOLV}'))
    print('No server worked; resolv.conf restored, boot script untouched.')
    done(); sys.exit(1)

print(f'== 3. boot script: DNS -> {good}')
out = run(f"sed 's|^DNS=\"192\\.168\\.1\\.200\"$|DNS=\"{good}\"|' {SCRIPT} > /tmp/wfn.new && "
          f"diff {SCRIPT} /tmp/wfn.new; echo diff-exit=$?; bash -n /tmp/wfn.new && echo SYNTAX-OK; "
          f"wc -l < {SCRIPT}; wc -l < /tmp/wfn.new")
print(out)
lines = out.splitlines()
# busybox diff prints a unified diff: count only the -/+ content lines.
changed = [l for l in lines if l[:1] in '-+' and not l.startswith(('---', '+++'))]
expected = ['-DNS="192.168.1.200"', f'+DNS="{good}"']
if changed != expected or 'SYNTAX-OK' not in out:
    print('Unexpected diff or syntax error; NOT installing. Restoring resolv.conf.')
    print(run(f'rm -f /tmp/wfn.new; cp -p {RESOLV}.bak-dns {RESOLV}'))
    done(); sys.exit(1)
print(run(f'cat /tmp/wfn.new > {SCRIPT} && rm -f /tmp/wfn.new && sync && '
          f'ls -la {SCRIPT} && md5sum {SCRIPT} && grep -n "^DNS=" {SCRIPT} && bash -n {SCRIPT} && echo INSTALLED-SYNTAX-OK; '
          f'cat {RESOLV}; ls -la /usr/scripts/*.bak-dns /usr/etc/*.bak-dns'))
print('== 4. final check')
print(run('nslookup api.voicerss.org 2>&1 | tail -2; wget -q -T 10 -O /dev/null http://example.com/ && echo HTTP-BY-NAME-OK'))
done()
