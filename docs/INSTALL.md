# Installing karotzd on the rabbit: the plan

> **Status: plan only. Nothing here has been done.** Each phase that writes to flash
> needs the owner's explicit go-ahead, phase by phase. Approving one phase doesn't
> approve the next. See [KAROTZD.md](KAROTZD.md) for background and the rules for agents.
>
> The phases are scripted in [`tools/deploy.py`](../tools/deploy.py), which also
> installs squeezelite. See [DEPLOY.md](DEPLOY.md).

## Goals and principles

- **Never on the boot-critical path.** If karotzd is missing, broken or crashes,
  the rabbit boots and behaves exactly as it does today. The old web server on
  port 80, telnet and the hardware daemons stay untouched.
- **Additive only.** New files go in one new directory, `/usr/karotzd/`. The only
  existing file to change is one cron table, by adding a line. No existing file is
  replaced.
- **One step back at every stage.** Each phase has a rollback that takes a single
  command, and phase 1 is undone by deleting the directory.
- **Kill switch.** If `/usr/karotzd/disabled` exists, karotzd is never started.
- **Logs go to RAM** (`/tmp/karotzd.log`), never to flash.
- **Small flash footprint.** About 620 KB once, against about 147 MB free on `/usr`
  (yaffs). There are no repeated writes beyond what the old CGI already does.

## Why the rabbit can't be bricked by this

- **telnet doesn't depend on karotzd.** It's started by the stock system, so a shell
  is always available to delete `/usr/karotzd`.
- **Boot doesn't change.** karotzd is started by cron *after* boot has finished, by
  an entry that only starts a program. `startup.sh`, `waitfornetwork.sh`, inittab and
  the rootfs aren't edited.
- **Worst case is a crash loop**, i.e. karotzd crashing and being restarted every
  minute. That costs a little CPU, and `touch /usr/karotzd/disabled` or a power cycle
  plus that command stops it.

---

## Phase 0: read-only checks (no flash writes)

Before asking for approval, confirm these facts with read-only commands:

| Check | Command | Why |
|---|---|---|
| Free space | `df -P /usr` | Need well over 1 MB |
| Cron is running and where it reads from | `ps \| grep [c]rond`; `ls -la /usr/spool/cron/crontabs/`; `cat /usr/spool/cron/crontabs/root` | `startup.sh` starts `crond -c /usr/spool/cron/crontabs`, but only on the "internet connected" path |
| Does busybox crond support `@reboot`? | `crond --help 2>&1`; `busybox \| head -1` | Usually not, so we use a once-a-minute "start if not running" line |
| Nothing already on :8080 | `netstat -ltn` | Port choice |
| Existing state | `ls -la /usr/karotzd 2>&1` | Must not exist yet |
| Fresh RAM test of the build we'll install | `tools/run_live.py start` then `collect` | Same binary, same flags, flash fingerprint unchanged |

If the root crontab doesn't exist or crond isn't running, use **Plan B** below
instead of cron.

---

## Phase 1: copy the binary to flash, with no autostart

*Flash writes: one new directory and two files. Approval needed.*

1. **Transfer to RAM first.** The old uuencode-over-telnet method took about 3 min
   with gzip. A faster way is to serve `sim/out/` from the PC
   (`python -m http.server 8000`) and pull it from the rabbit:
   ```bash
   unset HISTFILE
   wget -q -O /tmp/karotzd http://<pc-ip>:8000/karotzd && md5sum /tmp/karotzd   # compare with the PC's md5
   chmod 755 /tmp/karotzd && /tmp/karotzd selftest                              # must be all PASS
   ```
2. **Copy into place** only once the md5 matches and the self-test passes:
   ```bash
   mkdir /usr/karotzd
   cp /tmp/karotzd /usr/karotzd/karotzd && sync
   md5sum /usr/karotzd/karotzd     # must equal the PC's md5
   ```
3. **Add the start wrapper** `/usr/karotzd/run.sh`, mode 755:
   ```sh
   #!/bin/sh
   # Starts karotzd if it isn't running. Called by cron every minute.
   [ -e /usr/karotzd/disabled ] && exit 0
   pidof karotzd >/dev/null && exit 0
   cd /tmp
   /usr/karotzd/karotzd serve 8080 --readonly >/tmp/karotzd.log 2>&1 &
   ```
   Check it with `sh -n /usr/karotzd/run.sh`.
4. **Run it by hand** (`/usr/karotzd/run.sh`) and test `http://192.168.1.233:8080/`
   from the PC with `tools/test_routes.py`.

**Rollback:** `killall karotzd; rm -rf /usr/karotzd; sync`

---

## Phase 2: start automatically

*Flash write: append one line to the root crontab. Approval needed.*

1. Back up the crontab first: `cp -p /usr/spool/cron/crontabs/root /usr/spool/cron/crontabs/root.bak-karotzd`
2. Append the line through a copy in `/tmp`, check that the diff is exactly one
   added line, then install it with `cat > file` so its permissions are kept. This is
   the same careful pattern `tools/fix_dns.py` used.
   ```
   * * * * * /usr/karotzd/run.sh
   ```
3. If busybox crond needs a reload, run `killall -HUP crond`, or let the next
   reboot pick it up.
4. **Reboot test:** power-cycle the rabbit and confirm:
   - the boot is normal (cyan, then green, then the "ready" sound);
   - karotzd answers on :8080 within about 1 min of "ready";
   - port 80 still works;
   - telnet works.

**Rollback:** copy the `.bak-karotzd` crontab back, or simply
`touch /usr/karotzd/disabled; killall karotzd`.

### Plan B (if cron isn't usable): one line in `startup.sh`

Add a single backgrounded line at the very end of `/usr/scripts/startup.sh`, after
`start_bricks`, so it can never delay or block anything before it:

```sh
[ -x /usr/karotzd/run.sh ] && /usr/karotzd/run.sh &
```

Use the same backup, diff check and `bash -n` check as the DNS fix, keeping a
`startup.sh.bak-karotzd`. Without cron there's no restart if it crashes; that's
acceptable, or `run.sh` can loop with a `sleep 60`. This touches a boot script, so
it gets its own approval.

---

## Phase 3: soak in read-only mode

Leave it running for a few days on :8080 with `--readonly`, and use the new UI
day to day. Watch these:

- `/api/info` for RSS growth. It is about 470 KB at start and should stay flat.
- `/tmp/karotzd.log` for errors.
- That the rabbit's behaviour is unchanged: button, tags, and the morning sounds.

## Phase 4: allow writes (drop `--readonly`)

*Changes `run.sh`. Approval needed.*

Without `--readonly`, karotzd writes the same OpenKarotz state files the old CGI
writes, for example `Run/led.color` and the sleep flags, RFID records and snapshots.
These are not new kinds of flash writes. `tts` also needs a key in
`/usr/openkarotz/voicerss.key` (see KAROTZD.md, Configuration).

**Rollback:** put `--readonly` back in `run.sh`.

## Phase 5 (optional, later): close the root-shell hole

The stock `/cgi-bin/cmd` on port 80 is an unauthenticated root shell. Two options,
each a separate approval:

- **Minimal:** `mv /usr/www/cgi-bin/cmd /usr/www/cgi-bin/cmd.disabled`. Undo with `mv` back.
- **Bigger:** make karotzd the server on port 80 and move the old server elsewhere.
  This needs a look at how the stock httpd is started (probably the rootfs init,
  which we won't touch), so it's not planned yet.

---

## Upgrading later

Copy the new binary to `/tmp`, run `selftest`, compare the md5, then
`cp /tmp/karotzd /usr/karotzd/karotzd.new && mv /usr/karotzd/karotzd.new /usr/karotzd/karotzd && sync && killall karotzd`.
Cron restarts it within a minute. Keep the previous version as `karotzd.prev` so
you can go back with one `mv`.

## Uninstall (everything)

```bash
unset HISTFILE
touch /usr/karotzd/disabled; killall karotzd
cp -p /usr/spool/cron/crontabs/root.bak-karotzd /usr/spool/cron/crontabs/root   # if phase 2 was done
rm -rf /usr/karotzd; sync
```

---

## The deploy script

[`tools/deploy.py`](../tools/deploy.py) runs these phases for karotzd and squeezelite:

- `check` is phase 0;
- `install` is phase 1;
- `autostart` is phase 2;
- `allow-writes` is phase 4;
- `uninstall` is the rollback.

It checks that OpenKarotz is installed before every command, and it's a dry run
unless given `--write-flash`. squeezelite follows the same pattern as karotzd: its
own `/usr/squeezelite/` folder, a `run.sh` with a kill switch, and a cron line. See
[DEPLOY.md](DEPLOY.md) for what each command does.
