# RustyRabbit

A fast, safe replacement for the web API on a **Karotz** rabbit running the
[OpenKarotz](http://www.openkarotz.org/) firmware: one static Rust binary,
**karotzd**, built for the rabbit's 200 MHz ARMv4T CPU and Linux 2.6.28.

- Serves the same `/cgi-bin/*` URLs and JSON as the stock bash CGI, **5–50× faster**.
  For example, `status` drops from 1258 ms to 112 ms, and `sound` from 6 s to 0.1 s.
- Has a new built-in web UI with no external dependencies, plus live button, RFID
  and LED events (SSE).
- Blocks the stock firmware's unauthenticated root shell (`/cgi-bin/cmd`) and
  other dangerous scripts.
- Has no crate dependencies: hand-written HTTP, D-Bus, MD5 and base64. It's about
  620 KB on disk and about 470 KB of RAM.
- Tested in an emulator (QEMU with a real Linux 2.6.28) and on a real rabbit.

| Doc | For |
|---|---|
| [docs/KAROTZD.md](docs/KAROTZD.md) | How it works, how to build and test it, what we learned about the hardware, and the rules for agents |
| [docs/INSTALL.md](docs/INSTALL.md) | The phased, reversible plan for installing it on the rabbit (not done yet) |
| [docs/artifacts/karotz-teardown.html](docs/artifacts/karotz-teardown.html) | The original read-only survey of the stock firmware: request path, timings, and what could be faster (open in a browser) |
| [CLAUDE.md](CLAUDE.md) | Short instructions for coding agents |

## Layout

```
karotzd/      Rust source (src/*.rs, src/ui.html)
sim/          Cross toolchain, emulator and test scripts (run in WSL)
  rust/       Target spec, musl build, linker wrapper, build/test scripts
tools/        Python helpers that test against a real rabbit over telnet/HTTP
squeezelite/  Build and tests for a static squeezelite (LMS player) for the rabbit
docs/         Documentation
```

## Quick start (WSL, Ubuntu 24.04, as root)

```bash
bash sim/rust/setup-rust.sh && bash sim/rust/build-musl.sh   # once
bash sim/rust/build-karotzd.sh                                # -> sim/out/karotzd
bash sim/rust/test-karotzd.sh                                 # needs sim/setup-wsl.sh + sim/build-kernel.sh once
bash squeezelite/scripts/build.sh                             # optional: squeezelite -> squeezelite/out
```

See [squeezelite/README.md](squeezelite/README.md) for the squeezelite player.

Status: not affiliated with Karotz or OpenKarotz. Use at your own risk. The
rabbit's flash is small and recovery is hard, which is why everything here is
tested from RAM first.
