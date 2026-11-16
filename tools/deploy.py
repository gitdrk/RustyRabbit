#!/usr/bin/env python3
"""Deploys karotzd and/or squeezelite on top of an existing OpenKarotz install.

Follows docs/INSTALL.md phase by phase. Run it on the PC (Windows or WSL), on the
same network as the rabbit. The PC serves the files over HTTP and the rabbit
pulls them with wget.

    python tools/deploy.py check                 # read-only: is this OpenKarotz? phase 0 checks
    python tools/deploy.py stage                 # RAM only: download to /tmp, md5, self-test, delete
    python tools/deploy.py install               # phase 1: copy to /usr/<name>/, start by hand, no autostart
    python tools/deploy.py autostart             # phase 2: one cron line per component
    python tools/deploy.py allow-writes          # phase 4 (karotzd only): drop --readonly
    python tools/deploy.py disable | enable      # kill switch: /usr/<name>/disabled
    python tools/deploy.py status                # read-only: what is installed and running
    python tools/deploy.py uninstall             # remove cron lines and /usr/<name>/

Safety:
  * Every command refuses to go on unless the rabbit looks like OpenKarotz on the
    hardware it was built for (Linux 2.6.28, armv4tl, OpenKarotz files and its
    web API answering on port 80).
  * Commands that write to flash (install, autostart, allow-writes, disable,
    enable, uninstall) are a DRY RUN by default: they print what they would do.
    Pass --write-flash to really do it, and only with the owner's go-ahead for
    that phase.
  * Only new files in /usr/karotzd and /usr/squeezelite, plus our own lines in
    the root crontab (with a backup), are ever written. A listing of all of /usr
    is compared before and after, and anything else that changed is reported.
  * Logs go to /tmp (RAM). `unset HISTFILE` runs right after login.

Options: --host IP (default 192.168.1.233), --only karotzd|squeezelite,
--lms IP and --name NAME (squeezelite), --upgrade (replace an installed binary,
keeping the old one as .prev), --http-port N (default 8099).
"""
import argparse, hashlib, http.server, os, re, shutil, socket, socketserver, sys, tempfile, threading, time, urllib.request

ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), '..'))
CRONDIR = '/usr/spool/cron/crontabs'
CRONTAB = CRONDIR + '/root'
CRONBAK = CRONTAB + '.bak-deploy'
OK_FILES = ['/usr/openkarotz', '/usr/www/cgi-bin/setup.inc', '/usr/www/cgi-bin/get_version',
            '/usr/scripts/startup.sh', '/usr/scripts/dbus_watcher', '/usr/karotz/bin/immortaldog']
PATH = 'PATH=/bin:/sbin:/usr/bin:/usr/sbin'


class Comp:
    def __init__(self, name, local, extra=''):
        self.name, self.local = name, local
        self.dir = '/usr/' + name
        self.bin = self.dir + '/' + name
        self.run = self.dir + '/run.sh'
        self.cron = '* * * * * %s' % self.run
        self.extra = extra

    def run_sh(self, args):
        if self.name == 'karotzd':
            start = ('RO=--readonly\n[ -e %s/allow-writes ] && RO=\n'
                     '%s serve 8080 $RO </dev/null >/tmp/karotzd.log 2>&1 &\n') % (self.dir, self.bin)
        else:
            srv = (' -s %s' % args.lms) if args.lms else ''
            # -C 5 closes /dev/dsp after 5 s idle, so the rabbit's own sounds can play.
            start = "%s -n '%s'%s -r 44100,48000 -C 5 </dev/null >/tmp/squeezelite.log 2>&1 &\n" % (self.bin, args.name, srv)
        return ('#!/bin/sh\n'
                '# Starts %s if it isn\'t running. Called by cron every minute.\n'
                '# Installed by tools/deploy.py (RustyRabbit). Kill switch: touch %s/disabled\n'
                '%s\n'
                '[ -e %s/disabled ] && exit 0\n'
                'pidof %s >/dev/null && exit 0\n'
                'cd /tmp\n%s') % (self.name, self.dir, PATH, self.dir, self.name, start)


COMPS = {'karotzd': Comp('karotzd', os.path.join(ROOT, 'sim', 'out', 'karotzd')),
         'squeezelite': Comp('squeezelite', os.path.join(ROOT, 'squeezelite', 'out', 'squeezelite-karotz'))}


# ---------------- telnet ----------------
class Rabbit:
    def __init__(self, host):
        self.host = host
        s = self.s = socket.create_connection((host, 23), 8)
        time.sleep(1); s.recv(8192)
        s.send(b'karotz\r\n'); time.sleep(1); s.recv(8192)
        s.send(b'\r\n'); time.sleep(1.5); s.recv(8192)
        s.send(b'unset HISTFILE; set +H\n'); time.sleep(0.3); s.recv(8192)   # no history on flash; no ! expansion
        s.settimeout(180)
        s.send(b'stty -echo\n'); time.sleep(0.5); s.recv(65536)
        self.pc_ip = s.getsockname()[0]

    def run(self, cmd):
        self.s.send((cmd + '; echo __E""ND__\n').encode()); buf = b''
        while b'__END__' not in buf:
            d = self.s.recv(65536)
            if not d: break
            buf += d
        t = buf.decode('latin1', 'replace').replace('__END__', '').replace('-bash-4.1# ', '')
        return re.sub(r'\x1b\[[0-9;]*m', '', t).replace('\r', '').strip()

    def close(self):
        try: self.s.send(b'stty echo\nexit\n'); self.s.close()
        except OSError: pass


def vals(out, key):
    return [l.split('=', 1)[1] for l in out.splitlines() if l.startswith(key + '=')]


# ---------------- file server on the PC ----------------
class Served:
    """Serves a temp dir over HTTP; the rabbit pulls files from it with wget."""
    def __init__(self, port):
        self.dir = tempfile.mkdtemp(prefix='karotz-deploy-')
        d, hits = self.dir, []
        self.hits = hits
        class H(http.server.SimpleHTTPRequestHandler):
            def __init__(self, *a, **k): super().__init__(*a, directory=d, **k)
            def log_message(self, fmt, *a): hits.append(fmt % a)
        class TS(socketserver.ThreadingMixIn, http.server.HTTPServer):
            daemon_threads = True; allow_reuse_address = True
        self.srv = TS(('0.0.0.0', port), H); self.port = port
        threading.Thread(target=self.srv.serve_forever, daemon=True).start()

    def put(self, name, data):
        with open(os.path.join(self.dir, name), 'wb') as f: f.write(data)
        return hashlib.md5(data).hexdigest()

    def close(self):
        self.srv.shutdown(); shutil.rmtree(self.dir, ignore_errors=True)


def fetch(r, srv, name, dest, md5):
    """wget a served file into /tmp on the rabbit and check its md5. Returns True if it matches."""
    out = r.run('rm -f %s; wget -q -O %s http://%s:%d/%s; md5sum %s 2>&1' % (dest, dest, r.pc_ip, srv.port, name, dest))
    return md5 in out


# ---------------- checks ----------------
def openkarotz_gate(r, verbose=True):
    """Refuses to go on unless this is OpenKarotz on the expected hardware."""
    out = r.run('echo K_KERNEL=$(uname -r); echo K_ARCH=$(uname -m); '
                "echo K_CNF=$(grep '^CNF_VERSION=' /usr/www/cgi-bin/setup.inc 2>/dev/null | cut -d= -f2 | tr -d '\"'); "
                'echo K_OKVER=$(cat /usr/www/ok.version 2>/dev/null); echo K_PATCH=$(cat /www/ok_patch 2>/dev/null); '
                'echo K_ROOTFS=$(cat /karotz/etc/rootfs_version 2>/dev/null); '
                'for f in %s; do [ -e $f ] && echo K_HAS=$f || echo K_MISSING=$f; done' % ' '.join(OK_FILES))
    problems = []
    first = lambda k: (vals(out, k) or [''])[0]
    kernel, arch, cnf = first('K_KERNEL'), first('K_ARCH'), first('K_CNF')
    if not kernel.startswith('2.6.28'): problems.append('kernel is %r, expected 2.6.28' % kernel)
    if arch != 'armv4tl': problems.append('arch is %r, expected armv4tl' % arch)
    if not cnf: problems.append('no CNF_VERSION in /usr/www/cgi-bin/setup.inc')
    for m in vals(out, 'K_MISSING'): problems.append('missing ' + m)
    try:
        api = urllib.request.urlopen('http://%s/cgi-bin/get_version' % r.host, timeout=20).read().decode('latin1')
    except Exception as e:
        api = 'ERROR %s' % e
    if '"version"' not in api: problems.append('OpenKarotz API on port 80 did not answer get_version (%s)' % api[:80])
    if verbose:
        print('OpenKarotz: CNF_VERSION=%s ok.version=%s patch=%s rootfs=%s kernel=%s %s' % (
            cnf or '?', first('K_OKVER') or '?', first('K_PATCH') or '?', first('K_ROOTFS') or '?', kernel, arch))
        print('get_version on :80 ->', api.strip()[:160])
    if problems:
        print('\nNOT OpenKarotz (or not the hardware this was built for). Nothing was changed.')
        for p in problems: print('  -', p)
        sys.exit(2)
    print('OpenKarotz detected: OK')


def facts(r):
    out = r.run('echo F_DF=$(df -P /usr | tail -1); '
                "ps | grep -q '[c]rond' && echo F_CROND=yes || echo F_CROND=no; "
                '[ -f %s ] && echo F_CRONTAB=yes || echo F_CRONTAB=no; '
                '[ -e %s ] && echo F_CRONBAK=yes; '
                'for t in pidof wget md5sum killall sync diff; do which $t >/dev/null 2>&1 && echo F_TOOL=$t || echo F_NOTOOL=$t; done; '
                "netstat -ltn 2>/dev/null | grep -q ':8080 ' && echo F_8080=busy || echo F_8080=free; "
                'for c in karotzd squeezelite; do [ -e /usr/$c ] && echo F_EXISTS=$c; '
                'pidof $c >/dev/null && echo F_RUNNING=$c; [ -e /usr/$c/disabled ] && echo F_DISABLED=$c; done; '
                '[ -e /usr/karotzd/allow-writes ] && echo F_WRITES=yes; '
                'echo F_BUSYBOX=$(busybox 2>&1 | head -1); '
                "echo F_SPOOL=$(cd %s 2>/dev/null && pwd -P); echo F_USRMNT=$(mount | grep ' /usr ' | head -1); "
                'echo F_PCM=$(cat /proc/asound/card0/pcm0p/sub0/status 2>/dev/null | head -1)' % (CRONTAB, CRONBAK, CRONDIR))
    f = {}
    for l in out.splitlines():
        if l.startswith('F_') and '=' in l:
            k, v = l.split('=', 1); f.setdefault(k, []).append(v)
    one = lambda k: (f.get(k) or [''])[0]
    f['free_kb'] = int(one('F_DF').split()[3]) if len(one('F_DF').split()) > 3 else 0
    f['one'] = one
    f['crontab'] = r.run('cat %s 2>/dev/null' % CRONTAB) if one('F_CRONTAB') == 'yes' else None
    return f


def print_facts(f):
    one = f['one']
    print('free on /usr: %d KB   (%s)' % (f['free_kb'], one('F_USRMNT')))
    print('crond running: %s   root crontab: %s   spool dir: %s   our crontab backup: %s' % (
        one('F_CROND'), one('F_CRONTAB'), one('F_SPOOL') or '?', 'yes' if f.get('F_CRONBAK') else 'no'))
    if f['crontab'] is not None:
        print('root crontab:\n  ' + ('\n  '.join(f['crontab'].splitlines()) or '(empty)'))
    print('busybox:', one('F_BUSYBOX'))
    print('tools missing:', ', '.join(f.get('F_NOTOOL', [])) or 'none')
    print('port 8080:', one('F_8080'), '  audio device:', one('F_PCM') or '?')
    for c in COMPS:
        print('%-12s installed: %-3s running: %-3s disabled: %s' % (
            c, 'yes' if c in f.get('F_EXISTS', []) else 'no', 'yes' if c in f.get('F_RUNNING', []) else 'no',
            'yes' if c in f.get('F_DISABLED', []) else 'no'))


# ---------------- flash fingerprint ----------------
def listing(r):
    out = r.run('ls -laeR /usr 2>/dev/null')
    secs, cur = {}, None
    for l in out.splitlines():
        if l.endswith(':') and l.startswith('/usr'):
            cur = l[:-1]; secs[cur] = set()
        elif cur is not None and l.strip():
            secs[cur].add(l)
    return secs


def report_changes(before, after, comps, strict=False):
    """Prints any change in /usr outside our own dirs and the crontab dir (strict: any change at all)."""
    if strict: comps = []
    ours = [] if strict else [c.dir for c in comps] + [CRONDIR]
    def allowed(sec, line):
        if any(sec == d or sec.startswith(d + '/') for d in ours): return True
        if sec == '/usr' and (line.endswith(' .') or line.startswith('total ') or
                              any(line.endswith(' ' + c.name) for c in comps)): return True
        if ours and sec == '/usr/spool/cron' and (line.endswith(' crontabs') or line.endswith(' .') or line.startswith('total ')): return True
        return False
    other = []
    for sec in sorted(set(before) | set(after)):
        b, a = before.get(sec, set()), after.get(sec, set())
        for l in sorted(b - a): other += [] if allowed(sec, l) else ['- %s: %s' % (sec, l)]
        for l in sorted(a - b): other += [] if allowed(sec, l) else ['+ %s: %s' % (sec, l)]
    if other:
        print('WARNING: other changes in /usr during this run (may be the rabbit\'s own activity, e.g. Run/ state files):')
        for l in other[:40]: print('  ' + l)
    else:
        print('flash check: nothing changed in /usr' + (' outside %s' % ', '.join(ours) if ours else ''))


# ---------------- phases ----------------
def stage(r, srv, comps, args, keep=False):
    """RAM only: pull each binary and run.sh into /tmp, check md5, test the binary."""
    ok = True
    for c in comps:
        if not os.path.exists(c.local):
            print('%s: local build missing: %s' % (c.name, c.local)); ok = False; continue
        data = open(c.local, 'rb').read()
        c.md5 = srv.put(c.name, data); c.run_md5 = srv.put(c.name + '.run.sh', c.run_sh(args).encode())
        t = time.time()
        if not fetch(r, srv, c.name, '/tmp/deploy-' + c.name, c.md5) or \
           not fetch(r, srv, c.name + '.run.sh', '/tmp/deploy-%s.run.sh' % c.name, c.run_md5):
            print('%s: download or md5 failed' % c.name); ok = False; continue
        r.run('chmod 755 /tmp/deploy-%s /tmp/deploy-%s.run.sh' % (c.name, c.name))
        syn = r.run('sh -n /tmp/deploy-%s.run.sh && echo SH-OK' % c.name)
        if c.name == 'karotzd':
            out = r.run('cd /tmp && /tmp/deploy-karotzd selftest 2>&1; cd /')
            good = 'PASS' in out and 'FAIL' not in out
        else:
            out = r.run('/tmp/deploy-squeezelite -? 2>&1 | head -1; /tmp/deploy-squeezelite -l 2>&1 | head -8')
            good = out.startswith('Squeezelite ') and '/dev/dsp' in out
        good = good and 'SH-OK' in syn
        print('%s: md5 %s ok, downloaded in %.0f s, run.sh syntax %s, test %s' % (
            c.name, c.md5, time.time() - t, 'ok' if 'SH-OK' in syn else 'BAD', 'PASS' if good else 'FAIL'))
        if not good: print('  ' + out.replace('\n', '\n  ')); ok = False
    if not keep or not ok:
        r.run('rm -f /tmp/deploy-*')
    return ok


def dry_or_do(r, args, title, cmds):
    print('\n%s %s:' % ('DOING' if args.write_flash else 'DRY RUN, would run', title))
    for c in cmds: print('  ' + c)
    if not args.write_flash: return None
    out = r.run(' && '.join(cmds) + ' && echo STEP-OK')
    if 'STEP-OK' not in out:
        print('FAILED:\n  ' + out.replace('\n', '\n  ')); sys.exit(1)
    return out


def cmd_install(r, srv, comps, args, f):
    for c in comps:
        exists = c.name in f.get('F_EXISTS', [])
        if exists and not args.upgrade:
            sys.exit('%s is already installed in %s. Use --upgrade to replace the binary.' % (c.name, c.dir))
        if c.name == 'karotzd' and f['one']('F_8080') == 'busy' and 'karotzd' not in f.get('F_RUNNING', []):
            sys.exit('port 8080 is in use by something else; not installing karotzd')
    need = sum(os.path.getsize(c.local) for c in comps) // 1024 + 1024
    if f['free_kb'] < need * 4:
        sys.exit('only %d KB free on /usr, want at least %d KB' % (f['free_kb'], need * 4))
    if f.get('F_NOTOOL'): sys.exit('missing tools on the rabbit: %s' % ', '.join(f['F_NOTOOL']))
    if not args.write_flash:
        for c in comps:
            print('\n--- %s/run.sh would be:\n%s' % (c.dir, c.run_sh(args)), end='')
    if args.write_flash:
        print('\n== RAM test of the exact files to install')
        if not stage(r, srv, comps, args, keep=True): sys.exit('RAM test failed; nothing written to flash')
    for c in comps:
        exists = c.name in f.get('F_EXISTS', [])
        cmds = []
        if exists:
            cmds += ['cp -p %s %s.prev' % (c.bin, c.bin)]
        else:
            cmds += ['mkdir %s' % c.dir]
        cmds += ['cp /tmp/deploy-%s %s.new' % (c.name, c.bin), 'mv %s.new %s' % (c.bin, c.bin),
                 'cat /tmp/deploy-%s.run.sh > %s' % (c.name, c.run), 'chmod 755 %s %s' % (c.bin, c.run),
                 'sync', 'rm -f /tmp/deploy-%s /tmp/deploy-%s.run.sh' % (c.name, c.name)]
        if exists: cmds += ['(killall %s 2>/dev/null; sleep 2; true)' % c.name]
        cmds += [c.run, 'sleep 3', 'pidof %s' % c.name]
        dry_or_do(r, args, '%s %s (phase 1)' % ('upgrade' if exists else 'install', c.name), cmds)
        if args.write_flash:
            print(r.run('md5sum %s; ls -la %s; tail -5 /tmp/%s.log' % (c.bin, c.dir, c.name)))
            if getattr(c, 'md5', '') not in r.run('md5sum %s' % c.bin): sys.exit('MD5 on flash does not match!')
            if c.name == 'karotzd':
                try: print('http :8080 /api/info ->', urllib.request.urlopen('http://%s:8080/api/info' % r.host, timeout=15).read()[:200])
                except Exception as e: print('http :8080 /api/info -> ERROR', e)
    if args.write_flash:
        print('\nInstalled. Nothing starts it on boot yet; that is `autostart`.')
        print('Rollback: python tools/deploy.py uninstall --write-flash' + (' --only ' + args.only if args.only else ''))


def crontab_update(r, srv, args, f, add=(), remove=()):
    if f['crontab'] is None:
        sys.exit('%s does not exist (or crond is not set up). Use Plan B in docs/INSTALL.md (separate approval).' % CRONTAB)
    old = [l for l in f['crontab'].splitlines()]
    new = [l for l in old if l.strip() not in remove] + [a for a in add if a not in [l.strip() for l in old]]
    if new == old:
        print('crontab already as wanted; nothing to do'); return False
    added = [l for l in new if l not in old]; removed = [l for l in old if l not in new]
    print('crontab change: ' + ' '.join(['+ %r' % a for a in added] + ['- %r' % x for x in removed]))
    if not args.write_flash:
        print('DRY RUN: would back up %s to %s (if no backup yet), write the new crontab, and touch %s/cron.update'
              % (CRONTAB, CRONBAK, CRONDIR))
        return False
    md5 = srv.put('crontab.new', ('\n'.join(new) + '\n').encode())
    if not fetch(r, srv, 'crontab.new', '/tmp/deploy-crontab.new', md5): sys.exit('crontab download failed')
    out = r.run('diff %s /tmp/deploy-crontab.new' % CRONTAB)
    plus = [l[1:] for l in out.splitlines() if l.startswith('+') and not l.startswith('+++')]
    minus = [l[1:] for l in out.splitlines() if l.startswith('-') and not l.startswith('---')]
    if sorted(set(plus) - set(minus)) != sorted(added) or sorted(set(minus) - set(plus)) != sorted(removed):
        r.run('rm -f /tmp/deploy-crontab.new')
        sys.exit('unexpected crontab diff, NOT installing:\n' + out)
    dry_or_do(r, args, 'update root crontab', [
        '([ -e %s ] || cp -p %s %s)' % (CRONBAK, CRONTAB, CRONBAK),
        'cat /tmp/deploy-crontab.new > %s' % CRONTAB, 'echo root > %s/cron.update' % CRONDIR,
        'sync', 'rm -f /tmp/deploy-crontab.new'])
    print(r.run('cat %s; ls -la %s' % (CRONTAB, CRONDIR)))
    return True


def cmd_autostart(r, srv, comps, args, f):
    if f['one']('F_CROND') != 'yes': sys.exit('crond is not running. Use Plan B in docs/INSTALL.md (separate approval).')
    if f['one']('F_SPOOL') and not f['one']('F_SPOOL').startswith('/usr'):
        sys.exit('crontab dir resolves to %s, which is not on flash; a cron line would not survive a reboot' % f['one']('F_SPOOL'))
    for c in comps:
        if c.name not in f.get('F_EXISTS', []):
            if args.write_flash: sys.exit('%s is not installed; run install first' % c.name)
            print('(%s is not installed yet; install comes first)' % c.name)
    if crontab_update(r, srv, args, f, add=[c.cron for c in comps]):
        print('\nTesting cron: stopping each program and waiting for cron to start it again (up to 2.5 min)...')
        for c in comps: r.run('killall %s 2>/dev/null' % c.name)
        t = time.time(); left = list(comps)
        while left and time.time() - t < 150:
            time.sleep(10)
            left = [c for c in left if not r.run('pidof %s' % c.name).strip()]
        for c in comps:
            print('%s restarted by cron: %s' % (c.name, 'no' if c in left else 'yes'))
        if left:
            print('Cron did not pick up the change yet. It will after a reboot; starting them by hand for now.')
            for c in left: r.run(c.run)
        print('Next: reboot test (normal boot, then karotzd on :8080 within about a minute of "ready").')


def cmd_allow_writes(r, srv, comps, args, f):
    if 'karotzd' not in f.get('F_EXISTS', []): sys.exit('karotzd is not installed')
    dry_or_do(r, args, 'karotzd: allow writes (phase 4)', [
        'touch /usr/karotzd/allow-writes', 'sync', '(killall karotzd 2>/dev/null; sleep 2; true)', '/usr/karotzd/run.sh'])
    print('Rollback: rm /usr/karotzd/allow-writes; killall karotzd  (cron restarts it read-only)')


def cmd_switch(r, srv, comps, args, f, on):
    for c in comps:
        if c.name not in f.get('F_EXISTS', []): print('%s is not installed; skipping' % c.name); continue
        if on: dry_or_do(r, args, 'enable ' + c.name, ['rm -f %s/disabled' % c.dir, 'sync', c.run])
        else: dry_or_do(r, args, 'disable ' + c.name, ['touch %s/disabled' % c.dir, 'sync', '(killall %s 2>/dev/null; true)' % c.name])


def cmd_uninstall(r, srv, comps, args, f):
    crontab_update(r, srv, args, f, remove=[c.cron for c in comps]) if f['crontab'] is not None else None
    for c in comps:
        if c.name not in f.get('F_EXISTS', []): print('%s is not installed' % c.name); continue
        dry_or_do(r, args, 'uninstall ' + c.name, [
            '(touch %s/disabled; killall %s 2>/dev/null; sleep 1; killall -9 %s 2>/dev/null; true)' % (c.dir, c.name, c.name),
            'rm -rf %s' % c.dir, 'rm -f /tmp/%s.log' % c.name, 'sync'])
    if args.write_flash:
        out = r.run('[ -e {b} ] && cmp -s {t} {b} && rm -f {b} && sync && echo BAK-REMOVED; true'.format(t=CRONTAB, b=CRONBAK))
        if 'BAK-REMOVED' in out: print('crontab is back to its original content; removed %s' % CRONBAK)


def cmd_status(r, srv, comps, args, f):
    print_facts(f)
    for c in comps:
        if c.name in f.get('F_EXISTS', []):
            md5 = hashlib.md5(open(c.local, 'rb').read()).hexdigest() if os.path.exists(c.local) else None
            out = r.run('md5sum %s 2>&1; ls -la %s; echo --- log; tail -5 /tmp/%s.log 2>&1' % (c.bin, c.dir, c.name))
            print('\n%s: %s\n  %s' % (c.name, 'same as local build' if md5 and md5 in out else 'differs from local build',
                                       out.replace('\n', '\n  ')))
        print('%s cron line: %s' % (c.name, 'yes' if f['crontab'] and c.cron in f['crontab'] else 'no'))
    if 'karotzd' in f.get('F_RUNNING', []):
        try: print('karotzd /api/info ->', urllib.request.urlopen('http://%s:8080/api/info' % r.host, timeout=15).read()[:300])
        except Exception as e: print('karotzd /api/info -> ERROR', e)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('command', choices=['check', 'stage', 'install', 'autostart', 'allow-writes', 'disable', 'enable', 'status', 'uninstall'])
    ap.add_argument('--host', default='192.168.1.233')
    ap.add_argument('--only', choices=list(COMPS))
    ap.add_argument('--write-flash', action='store_true', help='really write to flash (otherwise a dry run)')
    ap.add_argument('--upgrade', action='store_true')
    ap.add_argument('--lms', help='LMS server IP for squeezelite (default: discover)')
    ap.add_argument('--name', default='Karotz', help='squeezelite player name')
    ap.add_argument('--http-port', type=int, default=8099)
    args = ap.parse_args()
    if args.lms and not re.match(r'^\d+\.\d+\.\d+\.\d+$', args.lms): sys.exit('--lms must be an IP address')
    if not re.match(r'^[A-Za-z0-9 _-]{1,32}$', args.name): sys.exit('--name: letters, digits, space, _ and - only')
    comps = [COMPS[args.only]] if args.only else list(COMPS.values())
    if args.command == 'allow-writes': comps = [COMPS['karotzd']]

    r = Rabbit(args.host)
    srv = None
    try:
        print('rabbit %s, PC %s' % (args.host, r.pc_ip))
        openkarotz_gate(r)
        f = facts(r)
        if args.command == 'check':
            print_facts(f)
            for c in comps:
                print('local %s build: %s' % (c.name, c.local if os.path.exists(c.local) else 'MISSING (' + c.local + ')'))
            return
        if args.command == 'status':
            return cmd_status(r, srv, comps, args, f)
        flash = args.command != 'stage'
        if flash and args.write_flash:
            print('\n*** --write-flash: this WRITES TO THE RABBIT\'S FLASH (%s) ***' % args.command)
        srv = Served(args.http_port)
        if args.command in ('stage', 'install', 'autostart', 'uninstall') and (args.write_flash or args.command == 'stage'):
            probe = r.run('wget -q -O /dev/null http://%s:%d/ && echo REACH-OK' % (r.pc_ip, srv.port))
            if 'REACH-OK' not in probe: sys.exit('the rabbit cannot reach this PC on port %d (firewall?)' % srv.port)
        before = listing(r) if args.write_flash or args.command == 'stage' else None
        if args.command == 'stage':
            ok = stage(r, srv, comps, args)
            report_changes(before, listing(r), comps, strict=True)
            sys.exit(0 if ok else 1)
        {'install': cmd_install, 'autostart': cmd_autostart, 'allow-writes': cmd_allow_writes,
         'disable': lambda *a: cmd_switch(*a, on=False), 'enable': lambda *a: cmd_switch(*a, on=True),
         'uninstall': cmd_uninstall}[args.command](r, srv, comps, args, f)
        if before is not None:
            report_changes(before, listing(r), comps)
        if not args.write_flash:
            print('\nDry run: nothing was written. Add --write-flash to do it (owner approval needed for this phase).')
    finally:
        try: r.run('rm -f /tmp/deploy-*')
        except Exception: pass
        r.close()
        if srv: srv.close()


if __name__ == '__main__':
    main()
