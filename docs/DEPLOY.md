# Deploying to the rabbit: `tools/deploy.py`

`tools/deploy.py` installs **karotzd** and **squeezelite** on a Karotz that already
runs OpenKarotz. It follows the phases in [INSTALL.md](INSTALL.md): each phase is a
separate command, can be undone, and is a dry run unless you ask for it.

> **Status (2026-11-16):** written and tested without writing to flash. Nothing has
> been installed on the rabbit yet. Any command that writes to flash needs the
> owner's approval for that phase (see [Rules for agents](KAROTZD.md#rules-for-agents)).

## What you need

- **The rabbit running OpenKarotz,** with telnet open (user `karotz`, blank password).
  This is the stock OpenKarotz setup.
- **Python 3 on the PC** (Windows or WSL), on the same network as the rabbit. The
  script uses only Python's standard library.
- **The built binaries:**
  - `sim/out/karotzd`, from `sim/rust/build-karotzd.sh`;
  - `squeezelite/out/squeezelite-karotz`, from `squeezelite/scripts/build.sh`.
- **The rabbit able to reach the PC on TCP 8099.** The script serves the files from
  a temporary folder on that port, and the rabbit pulls them with `wget`. On Windows,
  allow Python through the firewall the first time.

## Quick use

```bash
python tools/deploy.py check                                     # is this OpenKarotz? what's there?
python tools/deploy.py stage                                     # test both binaries in RAM
python tools/deploy.py install --lms 192.168.1.72                # dry run: prints what it would do
python tools/deploy.py install --lms 192.168.1.72 --write-flash  # phase 1, for real (needs approval)
python tools/deploy.py autostart --write-flash                   # phase 2, for real (needs approval)
python tools/deploy.py status
```

Add `--only karotzd` or `--only squeezelite` to work on one program.

## What every command does first

1. **Logs in over telnet** and runs `unset HISTFILE; set +H` straight away, so no
   shell history is written to flash. Your PC's IP is taken from that connection.
2. **Checks that OpenKarotz is installed.** If anything below is missing, the script
   prints what's wrong, exits with code 2, and changes nothing:

   | Check | Why |
   |---|---|
   | `uname -r` starts with `2.6.28` and `uname -m` is `armv4tl` | The binaries are built for this CPU and kernel only |
   | `CNF_VERSION=` in `/usr/www/cgi-bin/setup.inc` | OpenKarotz's own version marker |
   | `/usr/openkarotz` exists | OpenKarotz's data folder, which karotzd reads and writes |
   | `/usr/www/cgi-bin/get_version` exists | Part of the OpenKarotz web API |
   | `/usr/scripts/startup.sh` and `/usr/scripts/dbus_watcher` exist | The OpenKarotz boot script, which also starts cron, and the button/RFID handler |
   | `/usr/karotz/bin/immortaldog` exists | It runs the hardware daemons that karotzd talks to over D-Bus |
   | `http://<rabbit>/cgi-bin/get_version` answers with a `"version"` | The OpenKarotz web server on port 80 is up, and karotzd passes unknown URLs on to it |

   It prints the versions it found, for example
   `CNF_VERSION=210 ok.version=200 patch=310 rootfs=12.07.19.00`.
3. **Collects facts** (read-only):
   - free space on `/usr`;
   - whether `crond` is running, and where the root crontab is;
   - the busybox version;
   - whether the tools it needs are there (`pidof`, `wget`, `md5sum`, `killall`,
     `sync`, `diff`);
   - whether port 8080 is free;
   - whether each program is installed, running or disabled;
   - whether the speaker (`/dev/dsp`) is in use.

## Commands

### `check` (read-only)

Runs the steps above and prints the results, including the current root crontab and
whether each local build exists. Run this first.

### `stage` (RAM only)

Tests the exact files that would be installed, without touching flash:

1. Serves each binary and its `run.sh` from the PC and downloads them to
   `/tmp/deploy-*` on the rabbit.
2. Checks each file's md5 against the PC's copy.
3. Checks that `run.sh` parses (`sh -n`).
4. Tests each binary:
   - karotzd: `karotzd selftest` must print PASS lines and no FAIL;
   - squeezelite: `-?` must print its version and `-l` must list `/dev/dsp`.
5. Deletes `/tmp/deploy-*`, then lists all of `/usr` again. Nothing may have
   changed.

### `install` (phase 1, writes to flash)

Creates one folder per program and starts each program by hand. Nothing starts it
at boot yet.

| File | Contents |
|---|---|
| `/usr/karotzd/karotzd` | The binary (about 620 KB) |
| `/usr/karotzd/run.sh` | Start script (below) |
| `/usr/squeezelite/squeezelite` | The binary (about 650 KB) |
| `/usr/squeezelite/run.sh` | Start script (below) |

With `--write-flash`, for each program it:

1. Refuses if the folder already exists (unless `--upgrade` is given), if port 8080
   is used by something else, if there's under 4× the needed space, or if a tool is
   missing.
2. Runs the `stage` test and keeps the files in `/tmp`. It stops here if the test
   fails.
3. Runs `mkdir`, copies the binary in as `.new` and renames it (so a half-copied file
   is never used), writes `run.sh`, sets `chmod 755`, runs `sync`, and deletes the
   `/tmp` copies.
4. Starts the program with `run.sh`, checks `pidof`, checks the md5 of the file on
   flash, and shows the start of its log. For karotzd it also fetches
   `http://<rabbit>:8080/api/info`.

**`--upgrade`** keeps the installed binary as `<name>.prev`, puts the new one in
place, and restarts the program. To go back:
`mv /usr/<name>/<name>.prev /usr/<name>/<name>`.

Without `--write-flash` it prints both `run.sh` files and every command it would run.

### The start scripts (`run.sh`)

The script generates these. The karotzd one:

```sh
#!/bin/sh
PATH=/bin:/sbin:/usr/bin:/usr/sbin
[ -e /usr/karotzd/disabled ] && exit 0          # kill switch
pidof karotzd >/dev/null && exit 0              # already running
cd /tmp
RO=--readonly
[ -e /usr/karotzd/allow-writes ] && RO=         # phase 4
/usr/karotzd/karotzd serve 8080 $RO </dev/null >/tmp/karotzd.log 2>&1 &
```

The squeezelite one has the same first lines (with its own kill switch), then:

```sh
/usr/squeezelite/squeezelite -n 'Karotz' -s <lms-ip> -r 44100,48000 -C 5 </dev/null >/tmp/squeezelite.log 2>&1 &
```

- `-s <lms-ip>` comes from `--lms`. Without `--lms`, squeezelite finds the server on
  the network.
- `-n` comes from `--name` (default `Karotz`). Only letters, digits, spaces, `_` and
  `-` are allowed.
- `-r 44100,48000` makes LMS resample hi-res files to a rate the speaker accepts.
- `-C 5` closes `/dev/dsp` after 5 s of silence, so the rabbit's own sounds
  (`madplay`, the "ready" sound, TTS) can still play. While squeezelite is playing,
  those sounds can't get the speaker, and the other way round.

Both logs go to `/tmp` (RAM). A reboot clears them, and they never wear the flash.

### `autostart` (phase 2, writes to flash)

Adds one line per program to the root crontab, `/usr/spool/cron/crontabs/root`:

```
* * * * * /usr/karotzd/run.sh
* * * * * /usr/squeezelite/run.sh
```

Every minute, cron runs `run.sh`. It does nothing if the program is running or
disabled, and starts it otherwise. That covers both start at boot and restart after
a crash. OpenKarotz's `startup.sh` starts `crond` only once the network is up, so the
programs start about a minute after the "ready" sound.

With `--write-flash` it:

1. Refuses if `crond` isn't running, if the crontab doesn't exist, if the crontab
   folder isn't on flash (a change there wouldn't survive a reboot), or if a program
   isn't installed. Plan B in [INSTALL.md](INSTALL.md) is the alternative, and it
   needs its own approval.
2. Works out the new crontab on the PC, downloads it to `/tmp`, and runs the
   rabbit's `diff` against the current one. It goes on only if the diff adds exactly
   the expected lines and removes nothing.
3. Backs up the current crontab to `root.bak-deploy`, unless a backup already
   exists. Then it writes the new content with `cat >`, which keeps the file's
   owner and mode, and writes `cron.update` so `crond` re-reads it. Then it runs
   `sync`.
4. **Tests cron:** it stops each program and waits up to 2.5 minutes for cron to
   start it again, then reports each one. If cron hasn't picked up the change, it
   starts the programs by hand and says so. The next reboot then picks it up.

After this, do the reboot test in INSTALL.md phase 2.

### `allow-writes` (phase 4, karotzd only, writes to flash)

Creates `/usr/karotzd/allow-writes` and restarts karotzd, which then runs without
`--readonly`. It then writes the same OpenKarotz state files the old CGI writes. To
undo it: `rm /usr/karotzd/allow-writes; killall karotzd`, and cron restarts it
read-only.

### `disable` / `enable` (writes a tiny file to flash)

- `disable` creates `/usr/<name>/disabled` and stops the program; `run.sh` then
  never starts it, even after a reboot.
- `enable` removes the file and starts the program.

Without the script, over telnet: `touch /usr/karotzd/disabled; killall karotzd`.

### `status` (read-only)

Shows the facts from `check` and, for each program:
- whether its md5 matches your local build;
- its folder listing and the end of its log;
- whether its cron line is there;
- karotzd's `/api/info` (memory use and uptime).

### `uninstall` (writes to flash)

1. Removes this script's cron lines, using the same diff check as `autostart`.
2. For each program: creates `disabled`, stops the program, deletes `/usr/<name>`
   and its log in `/tmp`, and runs `sync`.
3. If the crontab is now byte-for-byte the original, deletes `root.bak-deploy`, so
   nothing is left behind.

`--only` uninstalls one program and leaves the other alone.

## Safety

- **Dry run by default.** `install`, `autostart`, `allow-writes`, `disable`,
  `enable` and `uninstall` print their commands and stop. `--write-flash` makes
  them run, and the script prints a banner when it does.
- **It only adds files.** It writes:
  - new files in `/usr/karotzd/` and `/usr/squeezelite/`;
  - the root crontab, its `.bak-deploy` backup, and `cron.update` in the crontab
    folder.

  Stock OpenKarotz files, boot scripts, the port 80 web server and telnet are left
  alone. If both programs are missing or broken, the rabbit boots and works as before.
- **It checks for side effects.** With `--write-flash`, and for `stage`, it lists
  `/usr` (`ls -laeR`) before and after. It prints any change outside its own folders
  and the crontab folder. Some changes can come from the rabbit itself, for example
  OpenKarotz `Run/` state files, so check what it reports.
- **Each step checks the last one** (md5 before and after copying, `sh -n`, the
  self-test, the crontab diff), and the script stops at the first failure.
- **No password is involved.** It doesn't read or print Wi-Fi settings or keys.
- **It cleans up.** It deletes `/tmp/deploy-*` on the rabbit and the temporary
  folder on the PC, even when it fails.

## When something goes wrong

| Symptom | What to do |
|---|---|
| `NOT OpenKarotz ...` | That's by design: the script only deploys on top of OpenKarotz on this hardware. The list says what's missing. |
| `the rabbit cannot reach this PC on port 8099` | Allow Python through the Windows firewall, or use `--http-port` with a port that's open |
| `already installed ... Use --upgrade` | That's by design. Use `--upgrade` to replace the binary, or `uninstall` first. |
| `RAM test failed; nothing written to flash` | Rebuild, then run `stage` to see the test output |
| Cron didn't restart a program | Reboot, since `crond` reads the crontab when it starts. Check `status` afterwards. |
| A program keeps crashing | Run `disable`, or over telnet `touch /usr/<name>/disabled`. Its log is in `/tmp/<name>.log`. |
| Want everything gone | `python tools/deploy.py uninstall --write-flash`, or the manual steps under "Uninstall" in INSTALL.md |

## How it has been tested

None of these steps wrote to flash:

- **On the rabbit** (OpenKarotz 210, patch 310, busybox 1.24.0):
  - `check`, `status`, and `stage` (both binaries passed); the `/usr` listing was
    unchanged;
  - dry runs of `install`, `autostart` and `uninstall`;
  - the rabbit's `diff` output for adding and removing cron lines, parsed the way
    the script expects, using copies in `/tmp`.
- **On the PC:**
  - the generated `run.sh` files under `sh`, with stand-in binaries: the kill
    switch, no second start while running, the right arguments, and `allow-writes`
    dropping `--readonly`;
  - the OpenKarotz check against a fake non-OpenKarotz system: it refused, with
    code 2.

**Not tested yet:**
- any `--write-flash` run;
- whether busybox `crond` re-reads the crontab via `cron.update` without a reboot;
- the reboot test.
