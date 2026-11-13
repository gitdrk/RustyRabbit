# karotzd: how it works, how it's built, what we learned

karotzd is a single static Rust binary that replaces the Karotz rabbit's slow
bash CGI web API. It serves the same URLs with the same JSON, a new web UI, and
live rabbit events. This document is for both people and coding agents picking
the project up later. Agents: read [Rules for agents](#rules-for-agents) first.

- **Status (2026-11-13):** feature-complete for the OpenKarotz CGI API and tested on
  the real rabbit, but **not installed**. It has only ever run from `/tmp` (RAM)
  in `--readonly` mode. Installing it is planned in [INSTALL.md](INSTALL.md).
- **Size:** about 620 KB on disk, about 470 KB of RAM (RSS) while serving.
- **Dependencies:** none outside Rust's `std`. There are no crates, and the D-Bus,
  HTTP, MD5 and base64 code are written by hand.

---

## 1. The device

| | |
|---|---|
| Board | Karotz (Violet/Mindscape), running the OpenKarotz community firmware (patch 310) |
| CPU | ARM920T, **ARMv4T**, about 200 MHz, **no FPU**, no vDSO |
| RAM | 60 MB, about 42 MB available |
| Kernel | Linux **2.6.28-pragmatec** (`uname -m` → `armv4tl`) |
| Userland | uClibc 0.9.31, busybox, bash 4.1, dbus-daemon 1.2.24 |
| Storage | `/usr` is writable **flash** (yaffs, about 147 MB free). `/etc` → `/usr/etc` and `/www` → `/usr/www` are on flash too. `/tmp` and `/var` are ramfs. |
| Network | Wi-Fi with a static IP set in `/usr/scripts/waitfornetwork.sh` (ours is 192.168.1.233) |
| Access | telnet, user `karotz`, blank password, which gives a root shell |
| Old API | bash CGI in `/usr/www/cgi-bin` served on port 80; data in `/usr/openkarotz` |

### How the old firmware drives the hardware

- **Daemons ("bricks"):** `led-daemon`, `rfid-daemon`, `webcam-daemon` and
  `button-daemon` are started by `/usr/scripts/startup.sh` under `immortaldog`,
  which restarts them if they die. They talk over the **D-Bus system bus**
  (`/var/run/dbus/system_bus_socket`). Names are `com.mindscape.karotz.*`,
  interface `com.mindscape.karotz.KarotzInterface`.
- **Ears:** driven by the `/karotz/bin/ears` binary.
- **Sound:** played with `madplay` (MP3) or `mplayer` (streams). The old player
  takes commands through a FIFO.
- **Actions on events:** `/usr/scripts/dbus_watcher` greps `dbus-monitor` output
  and acts on button clicks and RFID tags. **A triple click plays `flush.mp3` and
  reboots the rabbit.**
- **Camera:** `webcam-daemon`'s `takeSnapshot("1","file",50)` writes
  `/tmp/picture.jpg` (RAM): 640x480, about 44 KB, about 1.5–1.6 s, and it flashes
  the LED. A live view is `ffmpeg` reading `/dev/camera` as MJPEG at about 1 fps.
- **Speed:** every CGI call starts bash and parses several include files, which costs
  about 0.3 s before any work. `status` takes about 1.3 s.

---

## 2. What karotzd does

```
browser ──HTTP──▶ karotzd :8080 ─┬─ native routes ── D-Bus ──▶ led/webcam daemons
                                 │                 └ spawn ──▶ ears, madplay, mplayer, ffmpeg, wget
                                 ├─ /, /ui ── embedded web UI (ui.html)
                                 ├─ /api/events[/stream] ◀── D-Bus signal listener
                                 ├─ blocked scripts ── 403
                                 └─ everything else ── proxy ──▶ old CGI on 127.0.0.1:80
```

- **Native routes:** about 50 `/cgi-bin/<name>` endpoints. Each keeps the old
  script's URL, parameters, validation messages and JSON, so the old web UI and
  any existing integrations keep working. GET and form POST are both accepted.
- **Blocked scripts (HTTP 403 with a JSON reason):**
  - `cmd`: runs any shell command from the URL as root.
  - `get_file` and `get_list`: arbitrary downloads to arbitrary paths.
  - `save_snapshot`: uploads photos to a hard-coded FTP server.
  - `api_test`, `create_ip_sound`, `test.py`: developer leftovers.
- **Proxied to the old server:** installers, firmware and patch updates, the
  vera/eedomus/zibase/hc2 home-automation hooks, `squeezebox`, and the old UI's
  static files. The old UI stays reachable at `/index.html`, and `/legacy`
  redirects there.
- **Events:** karotzd subscribes to all D-Bus signals and keeps the interesting
  ones (rfid, button, led, ears, webcam, plus its own test signal) in a 200-entry
  ring buffer. `/api/events?since=N&wait=S` long-polls, and `/api/events/stream`
  is Server-Sent Events. karotzd **only observes**; `dbus_watcher` still performs
  the actions.
- **Read-only mode (`--readonly`):** refuses anything that writes to flash, such as
  LED memory, sleep state, snapshots to `Snapshots/`, RFID records and cache
  clears. The reply says why. It was used for every test so far.

### Measured on the rabbit (old CGI → karotzd)

| Route | Old CGI | karotzd |
|---|---|---|
| `status` | 1258 ms | 112 ms (every field identical) |
| `leds` | about 1000 ms | about 137 ms (native D-Bus; a bus round trip is about 8 ms) |
| `sound` | 6028 ms (waited for playback to finish) | 106 ms (plays in the background) |
| `ears` | 611 ms | 48 ms |
| other read-only routes | — | 5–17× faster |
| `/api/info` | — | 20–30 ms |

### HTTP surface

| Path | What |
|---|---|
| `/`, `/ui` | New web UI: vanilla JS, no CDN, responsive, dark mode. Tabs: Control, Sounds, Speak, Camera, Tags, Events, System. |
| `/cgi-bin/<name>` | Ported OpenKarotz API (the full list is in `json_route` and `raw_route` in `src/main.rs`) |
| `/api/info` | Uptime, memory and version as JSON |
| `/api/events`, `/api/events/stream` | Events (long-poll / SSE) |
| `/api/stop` | Stops all sound players |
| `/hello` | Liveness text |
| anything else | Proxied to `--upstream` (default 127.0.0.1:80) |

### Command line

```
karotzd serve [port] [--readonly] [--datadir /usr/openkarotz] [--upstream 127.0.0.1:80]
              [--bus /var/run/dbus/system_bus_socket] [--no-events]
karotzd selftest                 # threads, fs, HTTP, routes, codecs, spawn; never touches hardware
karotzd listen [socket]          # print rabbit events as JSON lines
karotzd dbus-test <socket> [--emit] [--led RRGGBB]
karotzd child                    # helper for the selftest's spawn check
```

When karotzd runs as PID 1 (only inside the emulator's initramfs), it mounts
`/proc`, brings up loopback, runs `selftest` and powers the VM off.

### Configuration

- **TTS:** uses VoiceRSS. **No API key is built in.** karotzd reads `$VOICERSS_KEY`,
  or else the first line of `<datadir>/voicerss.key`. Without a key, `/cgi-bin/tts`
  replies with a `msg` saying so. MP3s are cached in `/tmp` as `md5(voice+text).mp3`
  and played with `madplay -A6`.
- Everything else comes from the OpenKarotz data directory (`--datadir`).

---

## 3. Source layout (`karotzd/src`)

| File | Lines | Role |
|---|---|---|
| `main.rs` | about 530 | Modes, route tables (`json_route`, `raw_route`), `handle()` dispatch order, `/api/*`, `selftest`, PID-1 emulator setup |
| `http.rs` | about 170 | Minimal HTTP/1.0: request parsing (query + form POST up to 64 KB), `respond`, `respond_json` (text/plain + newline, like the old CGI), the reverse `proxy`, `json_str` |
| `karotz.rs` | about 350 | Hardware layer: the global `Ctx` (datadir, readonly), LED (native D-Bus with a `dbus-send` fallback), ears jobs, sound, the state files in `Run/`, process helpers (`run`, `run_bg`, `killall`), `readonly_block` |
| `routes.rs` | about 1040 | The rest of the CGI port: status, sleep/wake, moods, stories, lists, cache, camera/webcam, TTS, RFID, tools, `blocked()` |
| `dbus.rs` | about 400 | Hand-written D-Bus client: EXTERNAL auth, little-endian marshalling of method calls and signals, body decoding (s o g i u b y n q), `call`, `emit`, `listen` |
| `events.rs` | about 140 | Signal filter, ring buffer, long-poll and SSE |
| `util.rs` | about 140 | md5, base64, URL encoding, `sh_quote`, `random_below` (`/dev/urandom`), `df -P` parsing |
| `ui.html` | about 380 | The web UI, compiled in with `include_str!` |

The order of checks in `handle()` matters: blocked scripts first, then JSON
routes, raw routes, UI pages, `/api/*`, and finally the proxy.

Rules the code follows on purpose:
- Nothing runs through a shell. Parameters are validated (hex colours, sound IDs
  with no path characters, `http(s)://` only for URLs).
- RFID `.action` files are base64 curl command lines, built with `sh_quote`,
  because `dbus_watcher` `eval`s them.
- Slow work (sound, ear moves) runs in the background, so the HTTP reply is immediate.
- At most 16 connections at once, each on a thread with a 128 KB stack.
- No temporary files on flash. The old scripts wrote `Tmp/tmplst` on every count.

---

## 4. Building

The build runs in **WSL (Ubuntu 24.04) as root** and needs no Docker. The scripts
assume the repository is at `C:\Code\Karotz` (`/mnt/c/Code/Karotz`). Edit the
paths at the top of each script if yours is elsewhere.

### One-time setup

```bash
bash sim/rust/setup-rust.sh   # ARM cross gcc, nightly Rust + rust-src
bash sim/rust/build-musl.sh   # musl 1.2.5 for ARMv4T at /opt/musl-armv4t, linker wrapper, stub libunwind
bash sim/setup-wsl.sh         # (emulator only) QEMU + an Ubuntu 14.04 chroot at /opt/trusty with gcc 4.7
bash sim/build-kernel.sh      # (emulator only) Linux 2.6.28 for versatilepb -> sim/out/zImage-2.6.28
```

### Build karotzd

```bash
bash sim/rust/build-karotzd.sh   # -> sim/out/karotzd (stripped) + an unstripped twin for the ISA scan
```

Why the toolchain looks the way it does:
- **Custom target** `sim/rust/armv4t-karotz-linux-musleabi.json`: LLVM target
  `armv4t-unknown-linux-musleabi`, soft-float, strict alignment, 32-bit atomics,
  fully static. It is built with `cargo +nightly -Zbuild-std=std,panic_abort -Zjson-target-spec`.
- **musl, not uClibc or glibc:** we link our own static musl, so the binary doesn't
  depend on the rabbit's libraries at all. On pre-ARMv6 cores, musl does its
  atomics and TLS through the kernel's kuser helpers.
- **musl is built without vDSO.** ARM Linux 2.6.28 has no vDSO, and qemu-arm's
  Thumb vDSO breaks ARMv4T (its `pop {pc}` doesn't switch back to ARM state).
  `build-musl.sh` deletes the `VDSO_` defines.
- **Linker wrapper** `armv4t-musl-cc`: links with musl's own start files and never
  pulls in Ubuntu's ARM libgcc or glibc, which are built for ARMv5T.
- **Stub libunwind** (`unwind-stub.c`): the build uses `panic=abort`, so unwinding is never needed.
- **Release profile:** `opt-level="s"`, LTO, `codegen-units=1`, `panic="abort"`, `strip`.
- **Kernel compatibility:** ARM 2.6.28 is missing `accept4`, `epoll_pwait`,
  `getrandom`, `statx` and `prlimit64`. The rabbit's vendor kernel does have
  `inotify_init1`. `sim/rabbit-syscalls.txt` has the probe results. Rust std plus
  musl fall back correctly; the self-test exercises the paths that matter.

---

## 5. Testing

```bash
bash sim/rust/test-karotzd.sh   # 3 layers, see below
bash sim/rust/test-dbus.sh      # D-Bus client + events against a private dbus-daemon in WSL
```

1. **Static ISA scan** (`scan-isa.sh`): disassembles the unstripped binary and fails
   on any instruction newer than ARMv4T. Allowlisted: musl's `__a_cas_v6/v7` and
   `__a_barrier_v7`, which are only chosen at runtime on ARMv6+.
2. **`qemu-arm -cpu ti925t`** (an ARMv4T core): runs `selftest`. **"spawn child:
   Exec format error" is expected here**, because user-mode QEMU can't exec an ARM child.
3. **Linux 2.6.28 on `qemu-system-arm -M versatilepb`:** karotzd runs as `/init`,
   all tests must PASS (23 at last count), and the log is written to
   `sim/out/karotzd-system-run.log`.

`sim/v5-check.sh` is a negative control. It proves that the ti925t CPU model
really traps ARMv5 instructions.

### Tests on the real rabbit (`tools/`)

These Python helpers drive the rabbit over telnet and HTTP. Each one logs in, runs
`unset HISTFILE` first, works only in `/tmp`, and cleans up after itself.

| Script | Does |
|---|---|
| `run_live.py start` / `collect` | Uploads karotzd to `/tmp` (gzip + uuencode over telnet, about 3 min), runs `serve 8080 --readonly` next to a raw `dbus-monitor`, and later collects events, stops everything, deletes it and **compares a flash fingerprint** from before and after |
| `test_routes.py` | karotzd (:8080) vs the old CGI (:80): timings, the `status` field diff, guards, POST, hardware, webcam stream |
| `cam_test.py` | Camera timing via D-Bus `takeSnapshot` (RAM only) |
| `record_events.py [minutes]` | Saves the SSE event stream to a file on the PC |
| `fix_dns.py` | The one approved flash change (see below), kept as a record |

---

## 6. History and findings

- **2026-11-05:** read-only survey of the rabbit; built the emulator (Linux 2.6.28
  on versatilepb) and a no-libc syscall probe. The probe matched the rabbit
  except for `inotify_init1`.
- **2026-11-08:** Rust toolchain for ARMv4T and musl; hello server passed all
  self-tests on the rabbit. Ported leds, ears and sound; added the native D-Bus client.
- **2026-11-09:** events (SSE and long-poll); camera tests.
- **2026-11-10:** found that telnet sessions were writing bash history to flash, so
  every session now runs `unset HISTFILE`. The rabbit rebooted during a button
  test, probably from a triple click.
- **2026-11-12:** captured a real double click live (single presses send no
  signal). Ported the rest of the CGI API and the new UI; `status` is identical to
  the old CGI. **DNS fix**, the only change ever made to the rabbit's flash and
  approved by the owner: `waitfornetwork.sh` pointed at a dead DNS server
  (192.168.1.200), and now uses the router (192.168.1.1). `resolv.conf` was
  updated too, and both files have `*.bak-dns` backups next to them.
- **2026-11-13:** removed the built-in VoiceRSS key, which is now read from config.

### Known security problems in the stock firmware (not fixed on the device)
- `/cgi-bin/cmd` on port 80 is an **unauthenticated root shell** for anyone on the LAN.
- `waitfornetwork.sh` stores the Wi-Fi password in plain text.
- telnet as root with a blank password.
- `save_snapshot` contains hard-coded FTP credentials.

### Gotchas
- **busybox:**
  - `ls` has no `--full-time` together with `-R`; use `ls -lae`.
  - `diff` prints unified format.
  - `nslookup` ignores its server argument.
  - `netcat -u -w` doesn't time out.
- **Clock:** reads 1970 until `ntpd` runs, so `find -newer` is useless.
- **ls colours:** differ between telnet sessions. Strip ANSI codes before comparing output.
- **Background jobs:** a `nohup`'d watchdog did not survive telnet logout. Always
  clean up explicitly.
- **Snapshot timing:** a snapshot takes about 1.6 s, so the D-Bus read timeout is 5 s.
- **LED after a snapshot:** the LED daemon broadcasts `event(uuid, 4)` after each
  command, and the webcam daemon flashes the LED, so restore the LED afterwards.

---

## Rules for agents

These rules come from the rabbit's owner and are not negotiable:

1. **Never write to the rabbit's flash without explicit approval for that specific
   change.** Flash covers `/usr` and everything linked into it (`/etc`, `/www`,
   `/usr/karotz`). The owner can't un-brick the rabbit. Installing software counts
   as a flash write.
2. **RAM-only tests are allowed:** `/tmp`, `/var`, D-Bus calls, and LED, ear and
   sound tests that can be reversed. Clean up afterwards and restore the LED.
3. **Run `unset HISTFILE` immediately after every telnet login.** Otherwise bash
   writes `.bash_history` to flash when you log out.
4. **Never fake button or RFID signals on the real bus.** `dbus_watcher` acts on
   them, and a triple click reboots the rabbit. To test the bus, use
   `karotzd dbus-test <sock> --emit` (a harmless `com.karotzd.Test.ping`).
5. **Don't use the snapshot CGI for tests**, because it writes to flash. Call the
   webcam daemon over D-Bus.
6. Before and after any live test, compare a flash fingerprint:
   `ls -laeR /usr | md5sum`, with ANSI codes stripped.
7. **Never commit secrets.** That includes the Wi-Fi password, API keys, and the
   files the owner copied off the rabbit, which stay in the git-ignored `reference/`.
