# Oversample bat detector for the TASCAM Portacapture X8

An unofficial firmware patch that adds a bat detector to the TASCAM Portacapture X8 recorder. It runs
on top of TASCAM's own V2.03 firmware: the X8 records 192 kHz WAV files as usual, and the bat screen
lets you listen to bats live through headphones while it records.

**Make the update file:** [pengowray.github.io/oversample-tascam-firmware](https://pengowray.github.io/oversample-tascam-firmware/)
patches TASCAM's V2.03 firmware in your browser.

This project is by the author of [Oversample](https://github.com/pengowray/oversample), an app for
looking at and listening to bat recordings. It is not affiliated with or endorsed by TASCAM. TASCAM's
firmware is not included: the patcher changes your own copy of the official update file.

## Status

Experimental. Earlier builds have run on one X8 (F/W Version 2.03 Build 217, H/W Version 1.00
Build 010, as GENERAL SETTINGS > SYSTEM > Information shows them). Other hardware versions are
untested. The current build is B258. The optional AK-BT1 Bluetooth adapter
has not been tried with the patch. [docs/HOMEBREW.md](docs/HOMEBREW.md) lists what each build changed
and what to check.

Installing TASCAM's official V2.03 update file the same way should put the X8 back to stock: the
updater checks only the version number, so it accepts the official file over a patched one. This
has not been tried yet. As with any firmware update, losing power or removing the card during the
update can leave the unit needing repair.

## What it adds

- **BAT DETECTOR and BAT PLAYBACK** tiles on the LAUNCHER, in place of USER SETTINGS 2 and 3. BAT
  DETECTOR opens MANUAL with the bat screen on top, so REC records as MANUAL does. BAT PLAYBACK lists
  the WAV files in the card's SOUND folder and its subfolders, newest first, and plays them through
  the listening modes, converting other sample rates to 192 kHz (from B254). From B255 it can list
  another folder at the top of the card instead, so files the X8 itself can't play can be kept out
  of SOUND. From B256, TE in BAT PLAYBACK plays the whole file slowed, 5 to 32 times (not yet tried
  on a unit).
- **Listening modes:** heterodyne (HET), comb, frequency division (FD), time expansion (TE) and 1:1,
  in stereo for HET, COMB and 1:1.
- **A waterfall** from 0 to about 96 kHz, with bat calls, steady tones, audible sounds and clicks
  marked as they are detected.
- **Auto volume** for the headphones, with a Bats only choice that is silent between calls.
- **GUANO metadata** in each WAV file: start time, length, recorder, and the phone's location when
  known.
- **Phone remote** (experimental): chimes played by a phone set the X8's clock, start and stop
  recording, and change the listening mode. The phone page is
  [remote.oversample.com](https://remote.oversample.com/#x8); the X8's SETTINGS, Phone tab shows a
  QR code for it. The format is in [docs/PHONE_REMOTE.md](docs/PHONE_REMOTE.md).
- **Settings** in four tabs: Listen, Record, Phone and Tests, with a CPU test.

Screens from the emulator are in [docs/screens/](docs/screens/).

## Building from source

You need Python 3, TASCAM's V2.03 update file (`#PortaX8_0217.203`, from
`portacapture_x8_fw_v203.zip` on TASCAM's download page), and rustup. `rust-toolchain.toml` makes
rustup use Rust 1.99.1 with the `thumbv7em-none-eabihf` target.

```bash
tools/build.sh
python3 tools/x8patch.py "#PortaX8_0217.203" target/thumbv7em-none-eabihf/release/x8-patch "#PortaX8_0254.203" --build 254
```

`tools/build.sh` runs `cargo build` for the X8 and removes Cargo's home folder from the source
paths stored in the firmware. With the same source and Rust version, the update file should have
the same SHA-256 as the one the patcher page makes.

`tools/x8patch.py` checks the stock code at every place it changes, then packs and checks the
update file. To publish a build on the patcher page, make its data with
`tools/make_patch_data.py` (see the file's header) and point `site/patches/latest.json` at it.

The AudioMoth chime decoder is the `chime` crate in the Oversample repository, which Cargo fetches
from GitHub at the commit named in `Cargo.toml`. To try changes to it before committing them in
Oversample, add a `.cargo/config.toml` outside this repository, for example in its parent folder:

```toml
[patch."https://github.com/pengowray/oversample"]
chime = { path = "/path/to/oversample/chime" }
```

Remove it before building a file to publish. Cargo treats a local dependency differently from one
fetched from GitHub, so the update file comes out different even when the code is the same.

The tests run on the computer: `cargo test --workspace`.

## Repository layout

- [crates/bat-dsp](crates/bat-dsp/README.md): the real-time bat DSP library (`no_std`): heterodyne,
  comb, frequency division, time expansion, pitch shift, spectrum, call detection.
- [crates/bat-app](crates/bat-app/src/lib.rs): the bat app (`no_std`): listening modes, call
  detection and the 320 × 480 screen.
- [crates/x8-patch](crates/x8-patch/src/main.rs): the code patched into the X8's firmware, and its
  hooks into the stock code.
- [crates/guano](crates/guano/src/lib.rs): GUANO metadata for WAV files (`no_std`).
- [crates/bat-app-web](web/README.md) and [web/](web/README.md): an emulator page that runs the bat
  app in a browser on WAV files or a microphone.
- [site/](site/index.html): the patcher page.
- [tools/](tools/): `x8fw.py` unpacks, checks and repacks update files; `x8patch.py` builds a
  patched update file; `make_patch_data.py` makes the patcher page's data; `fwdis.py` and
  `tools/ghidra/` help read the firmware.
- [docs/HARDWARE.md](docs/HARDWARE.md): the X8's chips, inputs, sample rates, display and controls,
  with the source of each fact and what is still unknown.
- [docs/FIRMWARE.md](docs/FIRMWARE.md): the update file format, the flash layout, how the patch
  hooks into the stock firmware, and how to build, install and go back to stock.
- [docs/HOMEBREW.md](docs/HOMEBREW.md): the firmware map, with addresses, and a log of every build.
- [docs/PHONE_REMOTE.md](docs/PHONE_REMOTE.md): the phone remote's format.
- [docs/FEASIBILITY.md](docs/FEASIBILITY.md): the original feasibility study.

`Tascam_X8_official/` (TASCAM's files) and `work/` (unpacked images and built update files) are
local folders, not part of the repository.

## Licence

MIT or Apache-2.0, at your option: see [LICENSE-MIT](LICENSE-MIT) and
[LICENSE-APACHE](LICENSE-APACHE). TASCAM and Portacapture are trademarks of TEAC Corporation.
