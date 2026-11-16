# RustyRabbit

New software for a **Karotz** rabbit that already runs the
[OpenKarotz](http://www.openkarotz.org/) community firmware. It adds to OpenKarotz
and doesn't replace it.

| Part | What it is | Where |
|---|---|---|
| **karotzd** | A fast, safe replacement for the OpenKarotz web API, plus a new web UI. One static Rust binary. | [`karotzd/`](karotzd/), [docs/KAROTZD.md](docs/KAROTZD.md) |
| **squeezelite** | A music player for Lyrion Music Server (LMS, formerly Logitech Media Server), built for the rabbit. The rabbit becomes a network speaker. | [`squeezelite/`](squeezelite/), [squeezelite/README.md](squeezelite/README.md) |
| **deploy.py** | A script that installs either or both on top of OpenKarotz, one reversible phase at a time | [`tools/deploy.py`](tools/deploy.py), [docs/DEPLOY.md](docs/DEPLOY.md) |

Both programs are fully static, so they don't depend on the rabbit's own libraries.
They're built for its 200 MHz ARMv4T CPU (no FPU) and Linux 2.6.28.

> **Status (2026-11-16):** both programs work and have been tested in emulators and
> on a real rabbit, but **from RAM only**. Nothing is installed on the rabbit yet.
> The deploy script is ready, but none of its steps that write to flash have been
> run.

## karotzd

- **Same API, much faster.** It serves the same `/cgi-bin/*` URLs and JSON as the
  stock bash CGI, **5–50× faster**. For example, `status` drops from 1258 ms to
  112 ms, and `sound` from 6 s to 0.1 s. Existing apps and the old UI keep working.
- **New web UI** at `http://<rabbit>:8080/`, with no external dependencies. It
  shows live button, RFID and LED events (long-poll and SSE).
- **Safer.** It blocks the stock firmware's unauthenticated root shell
  (`/cgi-bin/cmd`) and other dangerous scripts. Anything it doesn't handle itself is
  passed on to the old server on port 80.
- **Small.** It has no crate dependencies: HTTP, D-Bus, MD5 and base64 are written
  by hand. It's about 620 KB on disk and uses about 470 KB of RAM.
- **Read-only mode** (`--readonly`) refuses anything that would write to flash.
  It's the default when installed, until you run `deploy.py allow-writes`.

## squeezelite

- **Formats:** FLAC, MP3 and Ogg Vorbis (all integer-only decoders), plus PCM.
- **Sound output:** OSS (`/dev/dsp`) through PortAudio. It's about 650 KB.
- **Tested on the rabbit:**
  - FLAC took about 11% CPU, and 320 kbit/s MP3 about 21%;
  - memory use peaked at 6 MB;
  - there were no audible glitches.
- **Installed by deploy.py**, it runs with `-C 5`: it lets go of the speaker after
  5 s of silence, so the rabbit's own sounds still play.

## Deploying to the rabbit

[`tools/deploy.py`](tools/deploy.py) runs on the PC (Python 3) and talks to the
rabbit over telnet. The rabbit downloads the files from the PC with `wget`. The
full guide is [docs/DEPLOY.md](docs/DEPLOY.md).

```bash
python tools/deploy.py check                                     # read-only: is OpenKarotz there? what's installed?
python tools/deploy.py stage                                     # RAM only: download, md5, self-test, delete
python tools/deploy.py install --lms <lms-ip>                    # dry run: shows exactly what it would do
python tools/deploy.py install --lms <lms-ip> --write-flash      # phase 1: /usr/karotzd, /usr/squeezelite
python tools/deploy.py autostart --write-flash                   # phase 2: one cron line each
python tools/deploy.py status | disable | enable | uninstall
```

What keeps it safe:

- **It only runs on top of OpenKarotz.** Every command first checks for the right
  kernel and CPU, the OpenKarotz files, and the OpenKarotz web API answering on port
  80. If anything is missing, it stops before changing anything.
- **Dry run by default.** Nothing is written to flash without `--write-flash`.
  Each phase is a separate command with its own rollback.
- **It only adds files.** It creates new folders and adds lines to the root crontab,
  after taking a backup. Stock files, boot scripts, port 80 and telnet are left
  alone. If both programs vanished, the rabbit would boot and work as before.
- **Each step is checked.** It runs a RAM test before installing, checks md5 sums
  and the crontab diff, lists `/usr` before and after to catch any other change,
  and has a kill switch (`/usr/<name>/disabled`). Logs go to RAM.

[docs/INSTALL.md](docs/INSTALL.md) explains why each phase is done the way it is,
and gives the manual commands.

## Building and testing

Builds run in **WSL (Ubuntu 24.04) as root**. The scripts expect the repository at
`C:\Code\Karotz` (`/mnt/c/Code/Karotz`).

```bash
# once
bash sim/rust/setup-rust.sh && bash sim/rust/build-musl.sh   # cross toolchain + musl for ARMv4T
bash sim/setup-wsl.sh && bash sim/build-kernel.sh            # emulator: QEMU + Linux 2.6.28

# karotzd
bash sim/rust/build-karotzd.sh        # -> sim/out/karotzd
bash sim/rust/test-karotzd.sh         # ISA scan + qemu-arm ARMv4T + real 2.6.28 kernel; must all pass

# squeezelite
bash squeezelite/scripts/build.sh     # fetches pinned, checksummed sources -> squeezelite/out/squeezelite-karotz
bash squeezelite/test/test-qemu.sh    # decodes against the PC's decoders (FLAC bit-exact)
bash squeezelite/test/test-vm.sh      # same on a real 2.6.28 kernel
```

Every binary must pass the **ISA scan**, which fails on any instruction newer than
ARMv4T, and the **Linux 2.6.28 test** before it goes near the rabbit. Live tests
on the real rabbit (`tools/run_live.py`, `tools/test_routes.py`,
`squeezelite/test/rabbit_live.py`) work only in `/tmp`. They compare a flash
fingerprint before and after.

## Layout

```
karotzd/        Rust source (src/*.rs, src/ui.html)
squeezelite/    squeezelite build (scripts/), runtime library (rtlib/), tests (test/)
sim/            Cross toolchain, emulator and karotzd test scripts (run in WSL)
  rust/         Target spec, musl build, linker wrapper, build/test scripts
tools/          deploy.py, plus Python helpers that test against the real rabbit
docs/           Documentation (below)
reference/      Copies of the rabbit's own files: git-ignored, contains secrets, never commit
```

## Docs

| Doc | For |
|---|---|
| [docs/DEPLOY.md](docs/DEPLOY.md) | What `deploy.py` does, command by command: checks, files, rollback, troubleshooting |
| [docs/INSTALL.md](docs/INSTALL.md) | The phased, reversible install plan behind it, and why it can't brick the rabbit |
| [docs/KAROTZD.md](docs/KAROTZD.md) | How karotzd works, building and testing it, hardware findings, and the **rules for agents** |
| [squeezelite/README.md](squeezelite/README.md) | squeezelite build, tests, live results and LMS use |
| [docs/artifacts/karotz-teardown.html](docs/artifacts/karotz-teardown.html) | The original read-only survey of the stock firmware (open it in a browser) |
| [CLAUDE.md](CLAUDE.md) | Short instructions for coding agents |

## Disclaimer

Not affiliated with Karotz, Mindscape or OpenKarotz. Use at your own risk. The
rabbit's flash is small and hard to recover, which is why everything here is
tested from RAM first, and nothing is written to flash without a deliberate,
per-phase `--write-flash`.
