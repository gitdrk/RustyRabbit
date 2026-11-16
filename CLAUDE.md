# Agent instructions

Read `docs/KAROTZD.md` (the whole thing, and especially "Rules for agents") before
doing anything that touches the rabbit. `docs/INSTALL.md` is the install plan.

Hard rules (from the owner):
- **No writes to the rabbit's flash** (`/usr`, `/etc`, `/www`, `/usr/karotz`)
  without explicit approval for that specific change. Installing counts as a write.
  RAM-only tests in `/tmp` are fine.
- **Run `unset HISTFILE` immediately after every telnet login.**
- **Never fake button or RFID D-Bus signals on the real rabbit**: a triple click
  reboots it. Use `karotzd dbus-test <sock> --emit`.
- **Never commit secrets.** `reference/` (copies of the rabbit's files, including the
  Wi-Fi password) is git-ignored and must stay that way.

Deploying: `tools/deploy.py`, documented in `docs/DEPLOY.md`. It's a dry run unless
given `--write-flash`, which writes to flash and needs the owner's approval for that
phase. `check`, `stage` and `status` are read-only.

Build and test: `sim/rust/build-karotzd.sh`, then `sim/rust/test-karotzd.sh`
(WSL, root). Both the ISA scan and the 2.6.28 self-test must pass before anything
goes to the rabbit. squeezelite: `squeezelite/scripts/build.sh`, then
`squeezelite/test/test-qemu.sh` and `test-vm.sh` (see `squeezelite/README.md`). In the qemu-arm layer, "spawn child: Exec format error" is expected.
