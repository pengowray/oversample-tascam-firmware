# Portacapture X8 firmware and the bat patch

Reference for TASCAM's stock firmware for the Portacapture X8 and for how this project patches it:
the update file format, the start-up and update path, where the patch lives, how it hooks the stock
code, how builds are made and published, how to install and recover, and the known limits. For the
chips, inputs and rates, see [HARDWARE.md](HARDWARE.md). The full address tables and the log of
every test build are in [HOMEBREW.md](HOMEBREW.md); this page explains each mechanism once and links
there for the details.

Written on 10 November 2026. The current build is B256 (built 9 November 2026, not yet installed).
The last build installed on the owner's unit is B254.

## Stock firmware

| Item | Details |
|---|---|
| Releases in this repository | V2.01, file `#PortaX8_0215.201` (build 215, header date 2024-10-02). V2.03, file `#PortaX8_0217.203` (build 217, header date 2025-08-19, released 8 September 2025). V2.02 (July 2025) is not kept. TASCAM's files are in the local `Tascam_X8_official/` folder, which git ignores. |
| Version the patch supports | V2.03 build 217 only. Every hook address is that build's. |
| Internal project name | `DR-TURBO`, the magic in the update file header and the name in TASCAM's source paths (`C:\Perforce\projects\SSR\DR-TURBO\firmware\work\...`). |
| Compiler | IAR Embedded Workbench for Arm, for both the bootloader and the main application. |
| Drivers | NXP MCUXpresso SDK (`fsl_*.c`): SAI, eDMA, LPI2C, LPSPI, LPUART, USDHC, USB, eLCDIF, PXP, FlexSPI and others. |
| RTOS | A μITRON-style kernel with a 2 ms tick. The patch calls its `cre_tsk`, `dly_tsk` and `snd_mbx`. Which kernel it is has not been identified. |
| User interface | Qt for MCUs (Qt Quick Ultralite), compiled to C++. Screens, the LAUNCHER tiles and their texts are compiled code and bindings, not data files. |
| Other libraries | Fraunhofer MP3 codec; TASCAM's "TCS" Bluetooth stack for the AK-BT1; Lattice's MachXO3 configuration routines for the FPGA (the board's MachXO2 takes the same commands). |

Tasks that matter to the patch (numbers are the RTOS's task ids):

| Task | Priority | What it does |
|---|---|---|
| 1 | 15 | Handles UI messages: buttons, the wheel, pop-ups. Calls the current screen's key handler. |
| 4 | not checked | Runs recorder commands one at a time, including closing a take's files and reading or setting the clock. 17 KB of stack. |
| 9 | not checked | Polls the buttons, the wheel and the touch controller. |
| 21 | 17 | Probably Qt (1 MB of stack). |

## Update file format

An update file is a 32-byte header and then the flash image, scrambled. All header fields are
big-endian.

| Offset | Size | Field | V2.03 value |
|---|---|---|---|
| 0 | 8 | Magic | `DR-TURBO` |
| 8 | 2 | Version | 203 |
| 10 | 2 | Build | 217 |
| 12 | 4 | Year (2 bytes), month, day | 2025, 8, 19 |
| 16 | 4 | sum32: the sum of the flash image bytes, except the last 4, modulo 2^32 | 0x7B86741B |
| 20 | 4 | Length of the whole file | 25,493,536 |
| 24 | 8 | Zero | |
| 32 | | The flash image, scrambled | 25,493,504 bytes |

- **Scrambling.** Each byte is replaced through a fixed 256-entry table, with no key. The DR-05
  homebrew project found the table in 2016 for an older TASCAM recorder, and the qubero hex editor
  project found that the X8 uses the same one.
- **Checksum.** The last 4 bytes of the flash image repeat sum32. There is no other checksum.
- **No signature.** The boot header has no HAB signature (its CSF pointer is 0), and the updater
  has no signature error. Every repacked test build that has been installed, from B218 on, was
  accepted.
- **Version check.** The function at 0x80E23DD0 refuses a file whose version is lower than the
  version compiled into the running image. It never compares the build number, which is only shown
  on the update screen. So any file with version 203 installs over V2.03, patched or not, and the
  official V2.03 file installs over a patched one. The patch keeps the image's own version (203 and
  217 at flash offset 0x20400), so the Information screen still shows V2.03 Build 217.

Updater errors: 93 Product Error, 94 Old Version, 95 Checksum Error, 96 to 99 flash erase, blank,
write and verify errors, 100 Update File Error. Errors 94, 95 and 100 come before anything is
written.

Details: [Update file format](HOMEBREW.md#update-file-format) in HOMEBREW.md.

## Flash layout

Flash image offsets equal offsets from the start of the 32 MiB flash.

| Flash offset | Address when running | Contents |
|---|---|---|
| 0x0000000 | 0x60000000 | FlexSPI configuration block (tag `FCFB`) |
| 0x0001000 | 0x60001000 | Boot header (IVT). No DCD, no signature. |
| 0x0002000 to 0x000A83D | 0x60002000 | Bootloader, about 34 KB, run from flash |
| 0x0020000 to 0x184FFFB | 0x80000000 (SDRAM) | Main application, about 25.4 MB |
| 0x184FFFC | | sum32 trailer of the stock image. In a patched image the trailer moves to the last 4 bytes, after the patch. |
| 0x1850000 onwards | 0x81830000 | The bat patch (not in the stock image) |
| 0x1FA0000 to 0x1FFFFFF | | Saved settings, in sectors 0x1FA, 0x1FB, 0x1FE and 0x1FF. Never written by the updater or the patch. |

The boot header and bootloader are identical in V2.01 and V2.03.

## Start-up and update mode

1. **Boot ROM.** Reads the FlexSPI configuration block and the boot header, and starts the
   bootloader from flash.
2. **Bootloader.** Sets up the clocks, the SDRAM and the display, then copies flash 0x20000 to
   0x1FDFFFF into SDRAM at 0x80000000 and jumps to the main application. It checks nothing, and has
   no SD card driver. Because it copies more than the stock image, anything stored after the stock
   image lands in SDRAM too. The patch relies on this.
3. **Main application start-up.** The reset handler sets the ITCM and DTCM split and runs IAR's
   start-up: it zeroes seven ranges, unpacks three LZ77-packed runs into SDRAM, ITCM and DTCM, and
   runs the C++ constructors. Then `main()` (0x80DF0F78) checks the boot button combinations and
   stores the **system mode** in the byte at 0x81969DD9: 0 or 1 for a normal start, 3 for firmware
   update mode (HOME and REC held at power-on), 8 for factory test mode, 5 and 6 for modes not yet
   identified (6 is probably the FPGA update).
4. **Update mode.** The updater reads the whole file from `UTILITY` on the card into SDRAM,
   descrambles it, checks sum32, and then erases and writes 64 KiB sectors in order from sector 0,
   which holds the boot header and bootloader. It writes as many sectors as the file needs. A
   normal update never touches the FPGA.

`main()` chooses update mode before it starts the RTOS, the screens or the audio. So a bug in the
patch's audio or screen code cannot stop update mode from starting. The patch's hooks also check the
system mode first (see [Safety](#safety-rules-and-the-checks-that-enforce-them)).

Details: [Start-up and update mode](HOMEBREW.md#start-up-and-update-mode-v203) in HOMEBREW.md.

## How the patch fits into the firmware

The patch is the `x8-patch` crate: Rust, `no_std`, built for `thumbv7em-none-eabihf` with
`target-cpu=cortex-m7`, with no memory allocator and `panic` as an endless loop. It uses the
`bat-app`, `bat-dsp`, `guano` and `chime` crates. `tools/x8patch.py` turns its ELF file into a
patched update file.

### Where the patch lives

| Section | Runs at | Stored in the update file | Notes |
|---|---|---|---|
| Header, code, read-only data, data | SDRAM 0x81830000 onwards | Yes, at flash 0x1850000, right after the stock image | No stock code refers to SDRAM 0x81830000 to 0x818FFFFF. IAR's start-up starts filling SDRAM at 0x81900000. The MPU maps this range as cached and executable. |
| `.bss` | Patch region | Yes, as zeros | |
| `.noinit` | Patch region | No | Buffers the patch writes before it reads them: the app's large buffers and the off-screen frame. |
| `.itcm` | ITCM from 0x31B00 | Yes, at its load address in the patch region | Copied into ITCM by `load_itcm` the first time a bat screen opens. |
| `.dtcm` | DTCM from 0x20011D00 | No | App state, the input ring, the DSP task's stack (32 KB) and the card reader task's stack (10 KB). |
| Fixed addresses (`spare.rs`) | SDRAM 0x865E2000 and 0x87EB0000 onwards | No | Output ring (cached), the display buffer, TE storage, task 1's patch stack and BAT PLAYBACK's read ring (uncached). |

`crates/x8-patch/x8patch.ld` is the linker script; its comments give the reasons for each range. At
B256 the patched update file is 25,852,932 bytes, 359,396 bytes longer than the official one.
Free space at B256: ITCM 76.4 of 89.2 KB used, DTCM 149.1 of 152.8 KB used, and the patch region
ends at 0x818FDA70 of 0x81900000. DTCM is nearly full.

The memory ranges and their sizes are in [HARDWARE.md](HARDWARE.md#memory-free-for-the-patch).

### The patch header

The patch starts with a header, `X8_PATCH_HEADER` in `crates/x8-patch/src/patch.rs`: the magic
`BATX`, a header version (18 at B256), the addresses of 31 entry points, and a build number.
`tools/x8patch.py` reads the entry points from the header, so the stock firmware's hook sites
always point at the right functions after a rebuild. The build field is 0 in the ELF file;
`x8patch.py` writes the update file's build number into it, and the CPU test shows it.

### Kinds of hook

Every change to the stock image is listed in the `BUILDS` table in `tools/x8patch.py`, keyed by
version and build. Before it changes a site, `x8patch.py` checks that the site holds the stock
value it expects, and it refuses the build if any site differs.

| Kind | What changes | Example |
|---|---|---|
| Pointer word | A literal word holding a stock function's address is replaced with a patch function's address. | 0x80DA9A20 holds the key handler that the LAUNCHER and app screens share. The patch's `key_hook` sees every button and wheel event, then calls the stock handler. |
| Tail call (`b.w`) | A branch at the end of a stock function is retargeted to a patch function. | The USER SETTINGS 2 and 3 tiles' handlers end with `b.w` to the function that loads a preset. The patch's `tile_hook` opens MANUAL instead. |
| Call (`bl`) | One call instruction is retargeted. The patch function usually calls the original and then adds to its result. | The launcher's title binding is called from one place. The patch's `text_hook` returns its own title for the bat tiles. |
| Function entry | The first 4 bytes of a stock function become a `b.w` to a patch function that does the whole job. | The bridge's USER SETTINGS slot getter (0x80D6A238). The patch's `slot_hook` reports slots 2 and 3 as holding a MANUAL preset. |
| Fact | A stock word the patch's constants depend on is checked and left alone. | The layout word for 8-bit alpha images, which the bat tiles' icons use. |

At B256 there are 33 hook sites: 2 pointer words, 3 tail calls, 26 calls and 2 function entries,
plus 1 fact that is only checked.
The patch also changes two things at run time, from inside its hooks:

- The audio callback pointer in the eDMA channel 9 handle (RAM, 0x2001024C), replaced with
  `audio_hook` once the stock audio setup has filled it in.
- The literal that Qt's present function uses as the display controller's register address
  (0x80E2B71C), pointed at a dummy block in RAM while a bat screen is open.

### Running code: tasks, stacks and timing

| Context | What runs there |
|---|---|
| Audio interrupt (48,000 times a second at 192 kHz) | `audio_hook`: calls the stock callback, copies the inputs the bat app listens to into the input ring, and writes the bat app's output from the output ring into the output block. It must stay short: a block at 192 kHz is about 12,500 cycles, and the stock callback uses 32% of the processor. |
| Task 1 (stock) | The key hooks. They queue actions for the DSP task and return quickly. Opening the bat screen the first time builds the app on a separate 40 KB patch stack, because task 1's own stack is 6 KB. |
| Task 9 (stock) | The touch hook. |
| Qt's task (stock) | The launcher hooks: the tile, text, icon, slot and delete-mode hooks. |
| Task 4 (stock) | The GUANO hook and the location log, at the end of a take. |
| DSP task (patch, priority 16) | Processes the input ring through the bat app, draws the screen about 10 times a second (5 while recording), and runs the clock, the phone remote and the BAT PLAYBACK player. Each pass stops after 30 ms and the task then sleeps 2 ms, so a mode that cannot keep up loses input instead of freezing the unit. 32 KB stack in DTCM. |
| Card reader task (patch, priority 12) | BAT PLAYBACK's card access: lists folders and WAV files and streams the chosen file into a read ring. Created with the DSP task; it touches the card only while BAT PLAYBACK is open and sleeps otherwise. 10 KB stack in DTCM. |

The patch creates its tasks with `cre_tsk`, trying ids that the stock firmware leaves unused at
start-up (23, 14, 13, 8 and 3). Priority 16 is below task 1 and every audio and recording task and
above Qt.

### Fast memory: ITCM and DTCM

Code running from SDRAM misses the instruction cache often enough to make the DSP too slow. In
HET, B222 used 73% of wall time; B224, with the hot code in ITCM and the hot state in DTCM, used
53%. So the linker script puts
the app's processing, the DSP library, the FFT and libm in `.itcm`, and the hot state in `.dtcm`.

Two rules keep this safe:

- **Nothing in ITCM may run before `load_itcm`.** ITCM is empty until the first bat screen opens,
  but the stock firmware calls the patch's hooks from start-up. B241 to B250 put `memcpy` and
  `memset` in ITCM; the key hooks and `load_itcm` itself called them, and B250 hung as soon as a
  bat screen opened (B241 to B249 were never installed). `load_itcm` now copies one word at a time with volatile reads and writes. And
  `x8patch.py` follows every call into ITCM back to where its chain of callers starts, and refuses
  the patch unless each chain starts at the bat screen's start-up, the DSP task, the tick function
  (`tick_work`, which returns at once until the bat screen is open) or code already in ITCM. Calls through function pointers are not followed.
- **The ITCM copy is checked once a second.** If its checksum changes, the patch stops running it.

The audio hook has two copies: the SDRAM one installed at start-up, and an ITCM one that the bat
screen switches to after loading ITCM.

### Display

While a bat screen is open, the patch:

1. Saves the display controller's current buffer and points the literal that Qt's present function
   uses (0x80E2B71C) at a dummy block in RAM, so Qt keeps drawing MANUAL but its presents go
   nowhere.
2. Draws into an off-screen frame and copies it in one pass to its own display buffer at 0x87EB0000
   (320 × 480, RGB565, uncached).
3. Writes that buffer's address to the display controller's next-buffer register.

On closing, it puts the literal back and shows the last buffer Qt presented. While task 1's state
byte (0x8196A3E0) says a pop-up is open, the DSP task gives the display and touch back to Qt, so
stock pop-ups such as "No Card" are visible and can be answered.

Drawing uses `embedded-graphics` and `u8g2-fonts`.

### Buttons, wheel and touch

- `key_hook` wraps the shared key handler of the LAUNCHER and app screens. `rec_key_hook` wraps the
  recording screen's handler, which the firmware installs from record standby to the end of a take
  (literal 0x80DD3A24). While the bat screen is up, the hooks give the bat app the buttons it uses
  (PLAY, MARK, the wheel, and a short HOME press in BAT PLAYBACK) and swallow those events by
  rewriting the message. During a take, MARK also goes on to the recorder. In BAT DETECTOR, REC
  goes on to the stock handler, so MANUAL records; in BAT PLAYBACK, REC is swallowed. Holding HOME
  goes on to the stock handler in both, which returns to the LAUNCHER. The full rules are in
  `crates/x8-patch/src/patch/keys.rs`.
- `touch_hook` replaces the touch poll's only call that passes a touch to Qt (`bl` at 0x80DF501A,
  task 9). While the bat screen is up, touches go to the bat app.
- The phone remote presses buttons by posting the same message that a real button press posts, into
  task 1's mailbox (a message block from 0x80D70027, sent with `snd_mbx`).

### The bat tiles: BAT DETECTOR and BAT PLAYBACK

The LAUNCHER's app tiles are compiled Qt code, so a new app tile would need new compiled code. The
three USER SETTINGS tiles already exist and are shown only while their slot holds a saved preset.
The patch takes over the second and third:

| What | Hook |
|---|---|
| Show the tiles | The slot getter (function entry, 0x80D6A238) reports slots 2 and 3 as holding a MANUAL preset, so the launcher always shows both tiles. |
| Titles and descriptions | The launcher's title binding, its six description-line bindings and the delete-mode label binding are wrapped (calls). For the bat tiles they return the patch's texts. The title's y binding moves the title up by 75 pixels, to where an app tile's title is: the stock firmware places USER SETTINGS titles lower, since those tiles have no description. |
| Icons | The icon image, height and width bindings are replaced for these two tiles (calls). The icons are 8-bit alpha masks made by `tools/make_glyph.py`: the Oversample bat and a play button. A hand-made image descriptor must set its colour and layout words, because the stock icons get theirs from static constructors; leaving them 0 hangs the unit on an assert. |
| Opening | The tiles' tail calls to "load preset" go to `tile_hook`, which opens MANUAL through Qt's bridge (property 6 = 1), as the MANUAL tile does. When the app byte (0x81E8CA90) changes to MANUAL, task 1 opens the bat screen over it. No preset is loaded. |
| Closing | The bat screen closes when the app byte changes, so holding HOME returns to the LAUNCHER as in the stock apps. |
| Presets | The first-free-slot function (function entry, 0x80D6A252) offers only slot 1, so a save cannot go into a slot hidden behind a bat tile. In delete mode the bat tiles show no badge, a tap opens them, and the delete listener ignores slots 2 and 3. USER SETTINGS 1 works as before. |
| Start-up app | At power-off, if a bat tile opened the current app, the settings are saved with the LAUNCHER as the open app, so the next start shows the LAUNCHER (the power-off sequence's `bl` to the settings save, called with interrupts off). |

Because the bat screen is drawn over MANUAL, TASCAM's own recorder makes every take: REC, the
files, the file names and the WAV format are MANUAL's. BAT DETECTOR listens to two of the inputs
turned on in MANUAL. BAT PLAYBACK reads WAV files from the card itself (from B254) and leaves
MANUAL's player alone.

Details: [Step 11](HOMEBREW.md#step-11-test-file-bat-detector-and-bat-playback-tiles-tuner-back-built-2026-11-05-never-installed-superseded-by-b229)
and "Launcher icons" and "USER SETTINGS tiles" in the [firmware map](HOMEBREW.md#firmware-map-v203-in-progress).

### Recorder, clock and card hooks

| Feature | How |
|---|---|
| MIX file switch | `open_take_hook` replaces the only call that opens a take's files; `file_mask_hook` runs right after the file mask is rebuilt. For a take from BAT DETECTOR with the switch off, the master (MIX) file's bit is removed from the mask. See [Turning off the MIX file](HOMEBREW.md#turning-off-the-mix-file-while-recording-2026-11-07). |
| GUANO metadata | `guano_hook` replaces a call made right after the stock code rewrites a WAV header at the end of a take or a split. For a take from BAT DETECTOR with the GUANO switch on (the default), it appends a GUANO chunk after the audio, on task 4, if at least 2 KB of stack is free. See [GUANO metadata](HOMEBREW.md#guano-metadata-2026-11-08). |
| Location log | The same hook appends the phone's location to `BAT_LOCATIONS.CSV` beside the takes, if 8 KB of stack is free. |
| Pauses | REC is kept from pausing a bat take unless the app's switch allows it; pauses that do happen are counted for GUANO. |
| Clock | The DSP task gives task 4 the stock commands that read and set the clock (75 and 76), only while the recorder is stopped, and reads the clock back to check. See [Setting the clock](HOMEBREW.md#setting-the-clock-locations-and-pauses-2026-11-08). |
| Card reads | BAT PLAYBACK's reader uses the file system's core calls with absolute UTF-16 paths, never changes folder, and keeps at most one folder and two files open. See [Reading files from the card](HOMEBREW.md#reading-files-from-the-card-2026-11-09). |

The phone remote's chime format is in [PHONE_REMOTE.md](PHONE_REMOTE.md).

## Safety rules and the checks that enforce them

The rules come from [Keeping the bricking risk low](HOMEBREW.md#keeping-the-bricking-risk-low).

| Rule | Status at B256 |
|---|---|
| Leave the boot header, bootloader and start-up and update-mode code unchanged | Enforced by `x8fw.py pack --base`, which refuses changes to flash 0x0 to 0x1FFFF and to the known start-up and update-mode functions. The list is incomplete: it does not cover the SD card and file system code that update mode also uses. |
| Never reach the settings sectors | Enforced by `pack`, which refuses an image long enough to reach sector 0x1FA. |
| Do nothing in update mode | Every hook reads the system mode byte first and goes straight to the stock code unless the mode is 0 or 1. The two function-entry hooks give the stock result instead. The power-off hook does not read the mode; it changes nothing unless a bat tile opened the current app, which cannot happen in update mode. |
| Never write to flash from patch code | Followed. The power-off hook calls the stock settings save, which writes flash the same way as before. |
| Check every hook site | `x8patch.py` checks the stock value at every site, that each target is a Thumb address inside the patch, that the base file is an unmodified official file, that the header version matches, and that every section is inside its region. |
| Nothing in ITCM before `load_itcm` | Enforced by `x8patch.py` (see [Fast memory](#fast-memory-itcm-and-dtcm)). |
| Turn the patch on only when a file is on the card | Not built. The hooks are installed in every normal start. To get rid of a misbehaving patch, install the official file. |

## Building

You need Python 3, rustup, and TASCAM's V2.03 update file. `rust-toolchain.toml` makes rustup use
Rust 1.99.1 with the `thumbv7em-none-eabihf` target.

```bash
tools/build.sh
```

```bash
python3 tools/x8patch.py "#PortaX8_0217.203" target/thumbv7em-none-eabihf/release/x8-patch "#PortaX8_0256.203" --build 256
```

The tests run on the computer: `cargo test --workspace`. The parts of `x8-patch` that need no
firmware addresses (`card`, `clock_record`, `file_mask`, `handoff`, `take_guano`, `wide`) have host
tests.

### Repeatable builds

The same source and Rust version give an update file with the same SHA-256 on any computer:

- `rust-toolchain.toml` pins Rust 1.99.1.
- `tools/build.sh` builds with `--remap-path-prefix`, so Cargo's home folder is not stored in the
  firmware's panic and error paths.
- The `chime` crate comes from the Oversample repository at the commit named in `Cargo.toml`.
- `.cargo/config.toml` sets `target-cpu=cortex-m7`, and the release profile uses LTO and one codegen
  unit.

A local `[patch]` for `chime` in a `.cargo/config.toml` outside the repository gives working code but
a different SHA-256, so remove it before building a file to publish.

### Publishing

A patched update file contains TASCAM's code and licensed third-party code, so it is not published.
The patcher page ([site/](../site/index.html), published to GitHub Pages by
`.github/workflows/pages.yml`) applies the patch to the user's own copy of the official file in the
browser:

1. `tools/make_patch_data.py` writes `manifest.json` (both files' names, sizes and SHA-256) and
   `changes.bin`: only the runs of bytes that differ, plus the bytes after the end of the original.
   Because the scrambling replaces each byte on its own, a changed byte holds only the patch's data.
2. `site/patches/latest.json` names the build the page offers.
3. The page refuses a file whose SHA-256 differs from TASCAM's, and checks the result's SHA-256
   against the published one before offering it.

## Tools

| Tool | What it does |
|---|---|
| `tools/x8fw.py` | `info`, `unpack`, `diff`, `pack` (with `--base` safety checks and `--allow-boot-path`), `replace` (same-length text) and `selftest` for update files. Standard library only. |
| `tools/x8patch.py` | Builds a patched update file from the `x8-patch` ELF file: hook sites, safety checks, build number, then `x8fw.py pack --base`. |
| `tools/build.sh` | Builds the patch for the X8 with stored paths made repeatable. |
| `tools/make_patch_data.py` | Makes the patcher page's data for a build. |
| `tools/make_glyph.py` | Makes the bat tiles' icons as alpha masks. Needs Pillow and numpy. |
| `tools/x8ram.py` | Unpacks what IAR's start-up copies into SDRAM, ITCM and DTCM, so a disassembler can load each run at its address. |
| `tools/ghidra/` | `x8_setup.py` builds a headless Ghidra 10.4 project of V2.03 (auto-analysis takes about 20 minutes); `q.sh` and `x8_query.py` decompile functions and list references from the command line. |
| `tools/fwdis.py` | Quick capstone helpers: disassemble, find callers, find what loads a string. Needs capstone and numpy. |

Notes from the reverse engineering that are too long for HOMEBREW.md are in the author's local
`work/notes/` folder (audio, input, launcher, phantom, recording), which git ignores.

## Installing and going back to stock

TASCAM's update procedure, with any update file:

1. Power the X8 from USB or the AC adapter, not batteries.
2. Turn on SD CARD READER (GENERAL SETTINGS > OTHER SETTINGS) and copy one update file into the
   card's `UTILITY` folder.
3. Eject the card on the computer, press DISCONNECT and turn the X8 off.
4. Hold HOME and REC while turning it on, until UPDATE MODE shows.
5. Press MARK. Check the file's version and build on screen. If ERROR 94, 95 or 100 shows, turn the
   X8 off: nothing has been written.
6. Press MARK again to install. Don't touch the power or the card until the X8 turns itself off.
7. Delete the file from `UTILITY`.

**Going back to stock** is the same procedure with TASCAM's `#PortaX8_0217.203`. The version check
should accept it over any patched V2.03, but this has not been tried on the unit yet.

**If a patched build misbehaves:** turn the X8 off (take out the batteries and USB power if POWER does
not respond, since a hung task 1 can stop POWER from working), then install the official file or a
fixed build from update mode. Update mode does not run the patch's code. This was needed once: B250
hung when a bat screen opened, and B251 was installed over it from update mode.

**What still bricks a unit:** losing power or removing the card while the updater writes (the
same as for an official update), or a change to the start-up and update-mode code. Recovery then
needs hardware access (SWD or the processor's USB serial download mode), which has not been found
on this board. See [HARDWARE.md](HARDWARE.md#opening-the-unit-and-debug-access).

## Limits and risks

- **One firmware build.** The patch works only on V2.03 build 217. A new TASCAM release needs every
  hook address found again and a new `BUILDS` entry in `x8patch.py`. If a unit has a newer version,
  a patch built on V2.03 also needs a version number in its update file header at least as high as
  the installed one.
- **192 kHz only for listening.** BAT DETECTOR needs the X8 set to 192 kHz and shows a notice at
  other rates. BAT PLAYBACK converts other rates to 192 kHz.
- **PITCH is too slow** on the X8 and is hidden there (see
  [HARDWARE.md](HARDWARE.md#processing-headroom-measured)).
- **Memory is nearly full.** DTCM has about 3.7 KB left at B256.
- **Not tested on a unit:** B255 and B256, going back to stock, a take started or stopped by the
  phone remote, and the AK-BT1 adapter. HOMEBREW.md's status line lists what was last checked.
- **Bat app settings are not saved in the X8's settings record,** since the patch never writes
  flash itself.
- **Card use is limited** to task 4 at the end of a take and to BAT PLAYBACK's reader. Reads during
  a take would compete with the recorder's writes, so the reader runs only while BAT PLAYBACK is
  open.
- **The update-path protection list is incomplete** (see [Safety](#safety-rules-and-the-checks-that-enforce-them)).
- **Warranty and licensing.** Changing the firmware voids the warranty. The project publishes only
  its own bytes; users patch their own copy of TASCAM's file.

## Terms

| Term | Meaning |
|---|---|
| B218, B256 and so on | Build numbers of this project's test builds. The packer writes the number into the update file header, and the update screen shows it. HOMEBREW.md has a section for each step. |
| Bat screen | The patch's own screen, drawn over MANUAL when BAT DETECTOR or BAT PLAYBACK is opened from the LAUNCHER. |
| HET | Heterodyne: the input is mixed with a tone so that calls near the tone's frequency become audible. |
| COMB | Several heterodyne tones at once, mixed together. |
| FD | Frequency division: a zero-crossing divider, as in older detectors. |
| TE | Time expansion: a captured call, or in BAT PLAYBACK the whole file, played slower. |
| 1:1 | The input played unchanged. |
| PITCH | Pitch shift by phase vocoder. Hidden on the X8 because it is too slow there. |
| Auto volume | The bat app's automatic headphone level. Its Bats only choice is silent between calls. |
| MIX file | The stereo mix file MANUAL records beside its track files (TASCAM calls it the master). |
| System mode | The byte at 0x81969DD9 that `main()` sets from the boot buttons: 0 or 1 for a normal start, 3 for update mode. |
| App byte | The byte at 0x81E8CA90 where the firmware's C code keeps the open app: 0 for the LAUNCHER, 2 for MANUAL. |
| Qt bridge | The firmware's link between the Qt screens and its C code. Setting bridge property 6 to 1 opens MANUAL. |
| Input frame | The 12 words the processor receives from the converters for each sample period (see [HARDWARE.md](HARDWARE.md#audio-path-in-the-processor)). |
| ITCM, DTCM | The Cortex-M7's tightly coupled memories for code and data: single-cycle, no cache. |

Boot terms (FCFB, IVT, DCD, HAB, XIP, SEMC, SAI, eDMA) are explained in [HOMEBREW.md](HOMEBREW.md#terms).
