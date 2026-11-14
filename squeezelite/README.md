# squeezelite for the Karotz

A static [squeezelite](https://github.com/ralph-irving/squeezelite) player for the
rabbit (ARMv4T at about 200 MHz, no FPU, Linux 2.6.28), so it can be a Lyrion Music
Server (LMS) player. It is about 650 KB and needs no libraries on the rabbit.

- **Formats:** FLAC, MP3 (libmad, fixed-point), Ogg Vorbis (Tremor, integer-only),
  and PCM/WAV/AIFF. AAC, ALAC, Opus, DSD and resampling are left out.
- **Sound output:** PortAudio v19 with only the OSS backend, writing to `/dev/dsp`.
  The rabbit has no `/dev/snd`.
- **Not installed on the rabbit.** It has only been run from `/tmp` (RAM).

## Build (WSL, Ubuntu 24.04, as root)

These steps are needed once, and are shared with karotzd:

```bash
bash sim/setup-wsl.sh && bash sim/rust/setup-rust.sh && bash sim/rust/build-musl.sh
```

Then build squeezelite:

```bash
bash squeezelite/scripts/build.sh      # -> squeezelite/out/squeezelite-karotz
```

`build.sh` does the following:

1. Runs `setup-cc.sh`, which installs the C toolchain pieces into `/opt/musl-armv4t`:
   - `armv4t-musl-gcc`, a wrapper that passes `-march=armv4t -marm -mfloat-abi=soft`
     and uses `armv4t-gcc.specs`.
   - `librt_armv4t.a`, which is Rust's `compiler_builtins` built from `rtlib/` for the
     ARMv4T target.

   Ubuntu's `libgcc.a` can't be used because it is ARMv5T and uses `clz`, which
   crashes on the rabbit. `setup-cc.sh` checks the replacement under
   `qemu-arm -cpu ti925t`.
2. Runs `fetch.sh`, which downloads the pinned sources into `sources/`, which is
   git-ignored. The tarballs are checked against SHA-256 and the git sources are
   fetched at fixed commits.
3. Builds libogg, Tremor, libmad, FLAC (decoder only), PortAudio (OSS only) and
   squeezelite with `-DLINKALL -DPORTAUDIO -DTREMOR_ONLY -DNO_FAAD -DNO_MPG123`.
   Build logs are in `/root/sqbuild/*.log`.
4. Strips the binary and runs the ISA scan (`sim/rust/scan-isa.sh`) on it. The scan
   must say "nothing newer than ARMv4T".

## Test

| Script | What it checks |
|---|---|
| `test/test-qemu.sh` | Runs on an emulated ARMv4T CPU, decoding FLAC, MP3 and Ogg that a stand-in server (`fake_lms.py`) sends. The PCM is compared with the PC's own decoders: FLAC must be bit-exact, and MP3 and Ogg should be about 84 dB SNR. |
| `test/test-vm.sh` | Runs on a real Linux 2.6.28 kernel (`qemu-system-arm`). `vmharness.c` is `/init`; it serves the files, acts as the server, and hashes the PCM. The hashes must match the `test-qemu.sh` outputs. Run `test-qemu.sh` first. |
| `test/rabbit_live.py` | Live test on the rabbit, RAM only (Windows Python). See below. |

### Live test on the rabbit

`rabbit_live.py` makes the PC the music server, with slimproto on port 3483 and HTTP
on 8099. Before running it, create the test tracks with `test/make-rabbit-tracks.sh`
in WSL.

The script then does the following:

1. Logs in over telnet, then runs `unset HISTFILE; set +H`. `set +H` turns off bash
   history expansion, which would otherwise break `$!`.
2. Has the rabbit download the binary into `/tmp` with `wget`.
3. Plays three 30-second tracks through `/dev/dsp`, logging CPU, memory and
   underruns.
4. Deletes everything it put in `/tmp` and checks that a fingerprint of the flash is
   unchanged.

It writes nothing to flash.

Results on 2026-11-13:

| Track | squeezelite CPU, average | Whole system idle |
|---|---|---|
| FLAC | 11% | 79% |
| MP3 at 320 kbps | 21% | 69% |
| MP3 at 128 kbps | 14% | 78% |

- Memory use (VmRSS) peaked at 6 MB.
- There were no audible glitches.

## Using it with a real LMS

```bash
/tmp/squeezelite-karotz -s <lms-ip> -n Karotz -r 44100,48000
```

`-r` limits the sample rates squeezelite reports, so that LMS resamples hi-res files.
Without it, squeezelite claims up to 192 kHz, and the OSS device may not accept that.

Starting it on boot would mean writing to flash. That needs the owner's approval: see
`docs/INSTALL.md` and the rules in `docs/KAROTZD.md`.
