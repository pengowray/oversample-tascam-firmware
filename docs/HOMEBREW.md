# Homebrew firmware: findings and plan

Notes from reading the V2.01 and V2.03 update files (`#PortaX8_0215.201`,
`#PortaX8_0217.203`), 2 November 2026. Nothing here has been tested on a unit yet.

Two names are used throughout:

- **Update file:** the file Tascam publishes, which the X8 reads from the SD card.
- **Flash image:** the update file without its 32-byte header, descrambled. It is
  the data the updater writes to the 32 MiB flash, starting at the first byte. It is
  the output of `tools/x8fw.py unpack`.

In the layout tables, addresses and flash image offsets are hexadecimal, and the
offset into the flash image equals the offset from the start of flash. Offsets and
sizes in the header table are decimal bytes. Terms are explained at the end.

For what the hardware can do and which features are feasible, see
[FEASIBILITY.md](FEASIBILITY.md).

## Summary

- **The update file format is understood.** `tools/x8fw.py` unpacks both releases
  and repacks them byte for byte.
- **No signature was found.** The boot header has no HAB signature, and the
  updater's error messages cover the product, the version, the checksum and flash
  writing, with no signature error. This comes from strings and headers, not from
  reading the updater code, so it is strong evidence but not proof.
- **Firmware update mode is in the main application, but it is chosen early.** The
  bootloader has no SD card driver. The main application's `main()` checks the boot
  button combinations and chooses update mode before the RTOS, the user interface or
  audio start. So a bug in audio or screen code cannot stop update mode from
  starting, provided the patch leaves the start-up and update code alone. See
  [Keeping the bricking risk low](#keeping-the-bricking-risk-low).
- **An interrupted update still bricks the unit.** The updater erases and rewrites
  the flash from the first sector, which holds the bootloader. That is equally true
  of official updates. The recovery for that case is SWD or the processor's USB
  serial download mode, which needs the unit opened.
- **The stock firmware uses NXP MCUXpresso SDK drivers, a μITRON-style RTOS, and a
  Qt for MCUs user interface.** Patching the audio code looks practical. Adding a
  screen inside the Qt UI is the hardest part, so the plan draws the bat app's
  screen directly into the framebuffer.

## Update file format

| Offset in update file | Size | Field | V2.03 value |
|---|---|---|---|
| 0 | 8 | Magic: Tascam's internal project name | `DR-TURBO` |
| 8 | 2 | Version | 203 (V2.03) |
| 10 | 2 | Build | 217 |
| 12 | 2 | Year | 2025 |
| 14 | 1 | Month | 8 |
| 15 | 1 | Day | 19 |
| 16 | 4 | sum32: the sum of the flash image bytes, except the last 4, modulo 2^32 | 0x7B86741B |
| 20 | 4 | Total length of the update file, header included | 25,493,536 |
| 24 | 8 | Zero | |
| 32 | 25,493,504 | The flash image, scrambled | |

All header fields are big-endian. The last 4 bytes of the flash image repeat sum32,
big-endian.

The scrambling replaces each byte with another through a fixed 256-entry table,
with no key. The DR-05 homebrew project worked the table out in 2016 for an older
TASCAM recorder (<https://github.com/dr05-homebrew/dr05-homebrew>,
`fwupdate/lut_decode.bin`). Analysis in the qubero hex editor project
(`crates/core/src/codec/tascam.rs` in its repository) found that the X8 uses the
same table. The header sum and the trailer sum both match the descrambled flash
image for both releases.

The updater's error messages, from the main application's strings:

| Code | Message |
|---|---|
| ERROR 93 | Product Error |
| ERROR 94 | Old Version |
| ERROR 95 | Checksum Error |
| ERROR 96 | Flash Erase Error |
| ERROR 97 | Flash Blank Error |
| ERROR 98 | Flash Write Error |
| ERROR 99 | Flash Verify Error |
| ERROR 100 | Update File Error |

**The version check (found in V2.03).** The function at 0x80e23dd0 checks an
update file's 32-byte header. It accepts two names: `DR-TURBO`, the main
firmware, and `TURBOCPL`, another part not yet identified. It checks that the
total length field matches the file's size. For `DR-TURBO` it then compares the
header's version (bytes 8 and 9, big-endian) with the version compiled into the
running firmware (a RAM structure returned by 0x80de7ec4; all 15 uses of it only
read it), and refuses the file if its version is lower (code at 0x80e23e60). The
three refusals return -9 (unknown name), -7 (wrong length) and -8 (lower
version); -8 is almost certainly ERROR 94 Old Version. **The build number is never
compared;** it is only shown on the update screen (`B%03d`, code at
0x80dfc4f8). The checksum is checked elsewhere.

What this means:

- An update file with the same version as the installed firmware is always
  accepted, whatever its build. The official V2.03 file can be installed again
  over any patched V2.03.
- A patched file that keeps the image's own version (203) leaves the official file
  installable afterwards, because the version check reads the running image, not
  the last file's header.
- An older release, such as V2.01 over V2.03, is refused, unless it is repacked
  with a header version of 203 or more. After that, the unit reports V2.01 and
  takes V2.03 again normally. This has not been tried.
- If Tascam releases V2.04 and a unit has it, a patch built on V2.03 needs a header
  version of 204 or more to install. The image would still report V2.03, so V2.04
  would install over it again.

### Tool

```bash
python3 tools/x8fw.py selftest "Tascam_X8_official/extracted/#PortaX8_0217.203"
```

```bash
python3 tools/x8fw.py unpack "Tascam_X8_official/extracted/#PortaX8_0217.203" work/v203.bin
```

```bash
python3 tools/x8fw.py diff "Tascam_X8_official/extracted/#PortaX8_0217.203" work/v203-patched.bin
```

```bash
python3 tools/x8fw.py pack work/v203-patched.bin "work/#PortaX8_0218.203" --base "Tascam_X8_official/extracted/#PortaX8_0217.203" --build 218
```

`replace` changes text at given offsets of a flash image, keeping its length, and
refuses if the old text is not there:

```bash
python3 tools/x8fw.py replace work/v203.bin work/v203-patched.bin --at 0x13814f4:TUNER:SONAR
```

`pack` recomputes sum32 in the header and in the flash image's last 4 bytes. With
`--base`, it lists every changed byte range, refuses changes to the boot header and
bootloader, and refuses changes to the start-up and update-mode code listed in
[Keeping the bricking risk low](#keeping-the-bricking-risk-low) unless
`--allow-boot-path` is given. Files in `work/` are ignored by git.

## Step 2 test files (installed 2026-11-02: the X8 accepts a repacked file)

**Result.** On 2026-11-02 the build 218 test file installed on the user's X8
through the normal update procedure (the update screen showed B218), the unit
started normally, and SONAR showed both as the app's name on the LAUNCHER screen
and as the tuner app's title, in the stock font. So the updater accepts a
repacked file with recomputed sums, and no signature or other check stopped it.
Still to do: install the official `#PortaX8_0217.203` again, to prove going back
to stock from the SD card.

Two update files for step 2 of the [plan](#plan): one to test that the X8 accepts
a repacked update file, and one to put the stock firmware back.

| File | What it is | SHA-256 |
|---|---|---|
| `work/test-update/#PortaX8_0218.203` | V2.03 with "TUNER" changed to "SONAR" in two places, build 218 | `2f8c67fa…ed96ae25` |
| `work/recovery/#PortaX8_0219.203` | V2.03 unchanged, build 219. Not needed: the official file `#PortaX8_0217.203` does the same job, since only the version is compared | `af92b2f9…a65826ba` |

The two changed strings, 5 bytes each, no code:

| Flash image offset | Where the string is | Expected on screen |
|---|---|---|
| 0x13814f4 | The launcher's list of app names, beside MANUAL, FIELD, PODCAST, METRONOME and LAUNCHER | The TUNER app's name on the LAUNCHER screen |
| 0x149bc0c | The tuner app's own strings, beside CALIBRATOR and OSCILLATOR | The tuner app's title |

Either change showing on the unit proves the patched image is running. A label
drawn as an image would not change, which is why there are two. The letters S, O,
N, A and R all appear in other text on both screens, so the fonts probably have
them even if they hold only the characters the stock text uses; a name drawn
blank or partly is still a visible change. A third "TUNER", in the POCKET STUDIO
tuner screen (offset 0x13d259c), is left as it is.

**The INFORMATION screen should still show V2.03.** The image holds its own
version and build as numbers (203 and 217, big-endian, at offset 0x20400), and
the test file leaves them alone; only the update file's header says 218. So
SONAR, not the version shown, is the sign that the test worked. Because the
updater compares only versions, the official V2.03 file installs over the test
firmware afterwards.

Checked before installing: unpacking the test file gives the official flash image
with only those letters and the last byte of the trailer sum changed, and the
header differs only in the build number and sum32. The recovery file's flash image
is the official one byte for byte. `pack --base` found no change to the
bootloader or the known start-up and update-mode code.

To build them again:

```bash
python3 tools/x8fw.py replace work/v203.bin work/v203-sonar.bin --at 0x13814f4:TUNER:SONAR --at 0x149bc0c:TUNER:SONAR
```

```bash
python3 tools/x8fw.py pack work/v203-sonar.bin "work/test-update/#PortaX8_0218.203" --base "Tascam_X8_official/extracted/#PortaX8_0217.203" --build 218
```

```bash
python3 tools/x8fw.py pack work/v203.bin "work/recovery/#PortaX8_0219.203" --base "Tascam_X8_official/extracted/#PortaX8_0217.203" --build 219
```

To install one, following Tascam's update procedure:

1. Power the unit from USB or the AC adapter, not batteries.
2. Turn on SD CARD READER (GENERAL SETTINGS, OTHER SETTINGS) and copy one update
   file into the `UTILITY` folder on the card. Only one update file at a time.
3. Eject the card on the computer, press DISCONNECT, and turn the unit off.
4. Hold HOME and REC while turning the unit on, until UPDATE MODE shows.
5. Press MARK. The screen shows the installed version and build and the file's
   version and build. Check that it shows build 0218 (or 0219 for the recovery
   file) before going on.
   If it shows ERROR 94 (Old Version), ERROR 95 (Checksum Error) or ERROR 100
   (Update File Error) instead, turn the unit off: nothing has been written yet.
   ERROR 94 would mean the version check found in the code is not the whole
   rule; ERROR 95 or 100 would mean the updater checks more than sum32 and the
   length. Either is worth reporting here before trying anything else.
6. Press MARK again to install. Don't touch the power or the card until the unit
   turns itself off. Errors 96 to 99 (flash erase, blank, write and verify) come
   during writing; after one of those the unit may need hardware recovery.
7. Delete the file from the `UTILITY` folder afterwards.

Then install the official file, `#PortaX8_0217.203`, the same way. If the tuner's
name goes back to TUNER, going back to stock from the SD card is proven on that
unit.



| Flash image offset | Address when running | Contents |
|---|---|---|
| 0x0000000 | 0x60000000 | FlexSPI NOR configuration block (tag `FCFB`) |
| 0x0001000 | 0x60001000 | Image vector table (IVT). Entry point 0x60002000. Boot data at 0x60001020: image start 0x60000000, size 32 MiB. No DCD. CSF pointer 0, so no HAB signature. |
| 0x0002000 to 0x000A83D | 0x60002000 | Bootloader ("BOOT" in Tascam's source paths), about 34 KB. Runs directly from flash (XIP). Initial stack pointer 0x20080000, reset handler 0x6000A0D5. |
| 0x000A83E to 0x000FFFF | | Unused (0xFF) |
| 0x0010000 to 0x001FFFF | | Erased (0xFF) |
| 0x0020000 to 0x184FFFB | 0x80000000 to 0x8182FFFB (SDRAM) | Main application, about 25.4 MB. Flash image offset 0x20000 is address 0x80000000. |
| 0x184FFFC to 0x184FFFF | | sum32 trailer |
| 0x1850000 to 0x1F9FFFF | 0x81830000 onwards | Outside the stock flash image, about 7.3 MiB. The bootloader copies it into SDRAM anyway (it copies 0x1fc0000 bytes, flash 0x20000 to 0x1fdffff). No stock code refers to SDRAM 0x81830000 to 0x818fffff (IAR's init table starts filling at 0x81900000), so patch code goes at flash 0x1850000 to 0x191ffff. |
| 0x1FA0000 to 0x1FFFFFF | | Saved settings: records tagged `DR-TURBO` and `X8USET`, each with a checksum, in 64 KiB sectors 0x1FA, 0x1FB, 0x1FE and 0x1FF. A patch must not write here. |

**Bootloader.** It uses SDK drivers for clocks, SEMC (the SDRAM controller), LPSPI,
LPUART, GPIO and eLCDIF. Its only messages are display errors ("Display
initialization failed", "Error: LCD INIT ERR SPI ERR"). It sets up the SDRAM and the
display, then starts the main application. It has no SD card driver. It copies
everything from flash image offset 0x20000 onwards into SDRAM at 0x80000000 and
jumps to it without checking anything (found by the qubero hex editor project,
`crates/core/src/formats/tascamfw.rs` in its repository).

**Main application.** Its vector table is at flash image offset 0x20000: initial
stack pointer 0x20011CC8 (DTCM), reset handler 0x8173206D (flash image offset
0x175206C, which starts with `cpsid i` and sets the stack pointer). Most interrupt
vectors point to two-instruction stubs that jump to low addresses such as
0x0002834F. Those addresses are in ITCM, so the startup code copies time-critical
code into ITCM. The audio interrupt handlers are probably among them.

**SDRAM is large.** The updater loads the whole 25.5 MB update file into SDRAM at
0x822F3AD8, so at least about 60 MB of SDRAM is real. A teardown video shows two
SDRAM chips, so the total is probably 64 MB or 128 MB. The reset handler sets its
first stack pointer to 0x86600000, which suggests 128 MB, but it is not proof: if
the SDRAM controller's address window is larger than the chips, a high address
also works. A 10 s stereo 32-bit float look-back buffer (15.4 MB) should fit if the
stock firmware leaves that much free, which is likely even at 64 MB.

## Start-up and update mode (V2.03)

From disassembly of the main application. Addresses are the running addresses.

1. The reset handler (0x8173206C) sets up the stack, the FlexRAM split between ITCM
   and DTCM, and the vector table, then runs the C library start-up, which calls
   `main()` at 0x80DF0F78. Both programs were built with IAR Embedded Workbench for
   Arm. The application's start-up follows IAR's init table: it zeroes seven
   ranges, unpacks three LZ77-packed runs into SDRAM, the ITCM and the DTCM, then
   runs the C++ constructors. (The init table and both of IAR's packing formats
   were worked out by the qubero project; see `crates/core/src/codec/iar.rs` in
   its repository.)
2. `main()` initialises the hardware, turns the unit off if the battery is too low,
   and sets up DMA. Then it checks four boot button combinations with the function
   at 0x80DAD730 and stores the result as the **system mode** (a byte at
   0x81969DD9):

   | Button combination (table index) | System mode | Meaning |
   |---|---|---|
   | 1 (HOME and REC, per the update instructions) | 3 | Firmware update from the SD card |
   | 2 | 6 | Probably the FPGA update (see below) |
   | 0 (MARK and HOME) | 8 | Factory test mode, with a "Debug Key Scan" screen |
   | 3 | 5 | Not yet identified |
   | none, and the function at 0x80DA9582 returns true (probably external power) | 1 | Normal start |
   | none | 0 | Normal start |

3. `main()` then starts the RTOS. A system task runs a state machine (0x80DC7A3C)
   that calls the update code in mode 3.
4. **The update procedure** (0x80DFC57E, flash writer at 0x80E23ED4) reads the whole
   update file into SDRAM at 0x822f3ad8 (refusing files over 32 MiB),
   descrambles it, checks sum32, turns off the data cache, then erases and writes
   64 KiB sectors in order from sector 0 (the boot header and bootloader): (file
   length >> 16) + 1 sectors, so a longer file is written in full. `x8fw.py pack`
   refuses an image that would reach the settings sectors. It retries a sector up
   to 4 times.
5. **The FPGA is not touched by a normal firmware update.** In mode 3 the update
   procedure calls only the main flash writer. In other modes it descrambles the
   file, checks it, and calls the FPGA programming routine (0x80E24334) up to 3
   times. That path is probably what boot combination 2 (mode 6) is for, with a
   separate FPGA update file that Tascam has not published.

The version check ("Old Version") is described under [Update file
format](#update-file-format).

**Changes from V2.01 to V2.03**, compared in 64 KiB blocks:

| Flash image offset | Contents | Status |
|---|---|---|
| 0x000000 to 0x01FFFF | Boot header, bootloader, erased block | identical |
| 0x020000 to 0x02FFFF | Start of the main application | changed |
| 0x030000 to 0xD2FFFF | Main application | identical |
| 0xD30000 to the end | Main application | changed |

The boot header and bootloader are the same in both releases. A patch must keep
those bytes identical to the official release, because the updater probably
rewrites them.

## Software in the image

| Component | Evidence |
|---|---|
| NXP MCUXpresso SDK drivers | `fsl_*.c` source paths: SAI, eDMA and DMAMUX, LPI2C, LPSPI, LPUART, USDHC and SD, USB, eLCDIF, PXP, FlexSPI, GPT, QTMR, TRNG, ADC. ENET and FlexCAN also appear, probably unused SDK code. |
| RTOS | μITRON-style kernel. The USB stack's OS adapter is `usb_osa_uitron.c`. Which kernel is unknown. |
| UI | Qt for MCUs (Qt Quick Ultralite): about 11,000 assertion strings with `qul/` header paths |
| MP3 | Fraunhofer-licensed MP3 encoder and decoder (Tascam's "CDK" library, from the DR-20 project) |
| Bluetooth | "TCS" control stack for the AK-BT1: `TCSBleManager.c`, `TCSBlePeripheral.c`, `TCSDataTransfer.c` and others |
| FPGA programming | Lattice's MachXO3 configuration flash routines (`MachXO3CFMAccessEnable`, `MachXO3CFMErase`, `MachXO3CFMInitializeAddress`, `MachXO3CFMProgramDone`, `MachXO3CFMBypass`) and the message `Write PLD : 0x%08lX`, at flash image offset 0xE44754. Just before them is a table of programming opcodes (0x26, 0x0E, 0xF0, 0x3C, 0x46, 0x70, 0x5E, 0xFF) and a page count of 2,164. The chip itself is a MachXO2 (LCMXO2-1200ZE, from a teardown video); MachXO2 and MachXO3 use the same programming commands, and 2,164 pages fits the MachXO2-1200. |
| Debug log | `fsl_debug_console.c` and the log format `[%s (%d)] ...`, so a debug UART probably exists |
| Source tree | `C:\Perforce\projects\SSR\DR-TURBO\firmware\work\BOOT\...` and `...\work\main\...` |

The FPGA configuration data has not been located. The standard Lattice bitstream
preamble (0xFFFFBDB3) does not appear in the flash image, so the data is probably
stored as raw configuration flash pages.

## Step 3 test file: patch code runs on a key press (installed 2026-11-02: works)

**Result.** Installed on the user's X8 on 2026-11-02. The bar appeared and
flickered from the end of the boot screen onwards, over the main menu and in
the apps, with no key pressed; everything kept working (apps open, buttons
work) and nothing crashed. Opening the HOLD dialog paused it. So the stock
"key handler" is called by task 1 for every message it handles, not only key
presses, many times a second, and it is the launcher's handler as well as the
apps'; the HOLD dialog installs its own handler while it is up. Patch code at
0x81830000 runs, and the hook is a regular call in task 1 that a patch can use
for its own screen updates.

The first file with patch code in it: `work/patch/#PortaX8_0219.203`, built by
`tools/x8patch.py` from the `crates/x8-patch` crate (SHA-256 `3efd237c…ede5ffec`).

- The patch (716 bytes of Rust, `no_std`) is linked at SDRAM 0x81830000 and
  stored at flash 0x1850000, after the stock image. The file is 720 bytes
  longer than the official one, and the updater writes the same sectors as for
  the official file (0 to 0x185), so installing it is no riskier than step 2.
- The MPU maps the patch region as executable, cached memory (region 5), so
  the code can run there. The framebuffers are uncached (only region 4 covers
  them), so the patch writes pixels without any cache maintenance.
- One stock word changes: 0x80da9a20, which held the app screens' key handler
  (0x80dac0d5), now holds the patch's `key_hook` (0x81830011). That address
  occurs nowhere else in the firmware, and only 0x80da98e0 reads the word.
  Update mode installs its own key handler, so the patch never runs there.
- `key_hook` checks the system mode, then, in a normal start, draws a 16-pixel
  bar across the middle of both framebuffers in the next of six colours, and
  jumps to the stock handler with the original arguments. Its press counter is
  a plain load and store: exclusive access (`ldrex`/`strex`) is avoided because
  the hook runs on one task only.
- Checked before installing: the packed file passes all checks and the safety
  checks of `pack --base`; the patch was disassembled from the packed file (mode
  check, framebuffer addresses 0x87e24400 and 0x87e6f400, no exclusive access,
  tail call to 0x80dac0d5 with r0 to r3 untouched and r4 to r8 restored).

To build it again:

```bash
cargo build -p x8-patch --release --target thumbv7em-none-eabihf
```

```bash
python3 tools/x8patch.py "Tascam_X8_official/extracted/#PortaX8_0217.203" target/thumbv7em-none-eabihf/release/x8-patch "work/patch/#PortaX8_0219.203" --build 219
```

**What to expect.** Install it like the step 2 file (the update screen shows
B219). In a recording app such as MANUAL, each press of MARK, PLAY or REC, or
each wheel turn, should draw a coloured bar across the middle of the screen, a
new colour each time, until the app redraws that part of the screen. The
buttons should still do what they always did. Turning the wheel is the best
test, since it sends many events. No bar at all is not a failure in itself: the
app may redraw that strip at once. Only a freeze or a restart means the hook
misbehaved.

**If it goes wrong.** If the unit freezes or restarts when a button is pressed
in an app, turn it off, hold HOME and REC while turning it on, and install the
official `#PortaX8_0217.203` from the `UTILITY` folder. Update mode does not run
the patch. The power button is read by the same task as the other buttons, so
if that task hangs the unit may not turn off: then remove the batteries and USB
power, and do the HOME and REC recovery.

## Step 4 test file: live heterodyne (built 2026-11-02, not yet installed)

`work/patch2/#PortaX8_0220.203` (B220, SHA-256 `7e06ff40…c2b00b0b`), built the
same way from `crates/x8-patch` (header version 2, 43.7 KB including fonts). The
updater writes the same sectors as for the official file.

- `key_hook` (still the key handler word) checks the system mode, then on its
  first call after the stock audio setup replaces the audio DMA callback (the
  word at 0x2001024c, checked to hold 0x2adfd first) with `audio_hook`.
- `audio_hook` reads the ring index, calls the stock callback, then, at 192 kHz
  with the heterodyne on, mixes inputs 3 and 4 (words 12k and 12k + 6) for the
  4 frames, heterodynes them (45 kHz to start, bat-dsp's `Heterodyne`), gains
  them by 8 with a ceiling of -6 dBFS, multiplies by the stock headphone gain
  (0x29ed8, so the volume setting and mute still apply) and writes both output
  words of each frame.
- Three MARK presses within 1.5 s toggle the heterodyne. While it is on, the
  wheel tunes it in 1 kHz steps (15 to 95 kHz) and those turns are swallowed.
- A 13-pixel status strip across the top shows: heterodyne state and
  frequency, whether the audio callback was replaced, the rate class (2 is
  176.4/192 kHz), how many times a second the key hook runs, and the last
  input event (kind and code: kind 0 code 2b is MARK, kind 2 is the wheel).

**What to expect.** The status strip from the end of the boot screen on,
flickering, saying "audio hooked". In MANUAL set to 192 kHz with microphones on
inputs 3 and 4: "rate 2"; three quick MARK presses turn "HET ON" and the
headphones play the heterodyned inputs; the wheel changes the kHz shown and the
pitch; three more MARK presses turn it off and the stock monitoring returns.
Start with the headphone volume low.

**If it goes wrong.** As for step 3. The audio callback runs about 48,000 times
a second, so a fault there would show as a freeze as soon as the patch installs
it (about when the launcher appears).

## Step 5 test file: the bat app on the X8 (installed 2026-11-02: runs, too slow)

**Result.** Installed on the user's X8 on 2026-11-02, without microphones on
inputs 3 and 4. The bat screen opened with the three MARK presses and drew the
app (title, status bar, meters, waterfall, last-call panel, mode buttons);
PLAY changed mode and the wheel worked, both with a noticeable lag; HOME went
back; nothing crashed. The key hook ran about 15 times a second when idle and
up to 46 while touching the launcher. The strip showed "lost 1,236,056"
(input frames dropped because the ring was full) and "dry 63" (output
underruns), so processing in task 1 did not keep up, and a band of the screen
blanked for a frame about once a second (the app draws straight into the
buffer being shown). Step 6 moves the work into a task of its own.

`work/patch3/#PortaX8_0221.203` (B221, SHA-256 `18d396c4…ddc791b2`): the whole
bat app (`crates/bat-app`) inside the patch, 410 KB in all (128 KB of code, the
app's 179 KB of state and a 64 KB stack, in the patch region). The updater
writes flash sectors up to 0x18b, past the stock image but far below the
settings. It replaces test 2.

How it fits in the stock firmware:

- **Opening and closing.** Three MARK presses within 1.5 s open the bat screen;
  HOME or three more MARK presses close it. MARK and REC still reach the stock
  app as well (a mark, or start and stop recording); PLAY steps through the
  listening modes (HET, COMB, FD, TE, PITCH) and the wheel adjusts the current
  mode, both swallowed while the bat screen is up.
- **Audio.** While the bat screen is up at 192 kHz, `audio_hook` copies inputs
  3 and 4 of every block into a ring buffer, and plays the app's output from a
  second ring buffer at the stock headphone gain, once 16,384 samples (85 ms)
  are buffered. Below 192 kHz it leaves the audio alone.
- **Processing.** `key_hook` runs the app on each call from task 1 (the patch
  does not create a task), on the patch's own 64 KB stack because task 1's is
  6 KB (from the RTOS task table at 0x8131dac4: entries of id, attributes, 0,
  entry, priority, stack size, stack base). The RTOS task switch only saves and
  restores the stack pointer, so this is safe. Each call processes up to
  24,576 frames and redraws about 20 times a second.
- **Display.** Qt's present function (0x80e2b574) writes the display
  controller's next-buffer register through one literal, 0x80e2b71c, read by
  nothing else. While the bat screen is up the patch points that literal at a
  dummy block of RAM, so Qt keeps drawing and presenting unseen, and sets the
  next-buffer register to its own buffer. Closing restores the literal and
  Qt's last buffer. Touch polling is stopped meanwhile (the word at
  0x8198edbc), so touches cannot reach Qt's hidden buttons, and restored after.
- **Memory outside the patch region:** the display buffer, the time
  expander's 0.5 s of storage and both ring buffers are in SDRAM after Qt's
  framebuffers (0x87eb0000 to 0x88000000), which no stock code refers to and
  the MPU maps uncached.
- **Status line.** With the bat screen closed, the strip across the top of
  Qt's screens shows whether the audio is hooked, the rate class and the key
  hook rate. With it open, a strip across the top of the bat screen shows the
  rate class, the key hook rate, the frames waiting in each ring, and counts
  of output underruns and dropped input.

**What to expect.** In MANUAL at 192 kHz with microphones on inputs 3 and 4:
three MARK presses show the bat screen with a moving waterfall; the
headphones play the heterodyne after a short delay; PLAY changes mode; the
wheel retunes; HOME returns to MANUAL. Recording in MANUAL carries on as normal
underneath.

**Not known until tried:** how often task 1 calls the hook (it sets the
latency and whether processing keeps up: watch "dry" and "lost" in the top
strip); whether the display switch shows tearing; and how the stock app copes
with running unseen.

**If it goes wrong.** As for the earlier tests: a freeze when the bat screen
opens should still allow HOME and REC at power-on and the official file. If
the screen stays on the bat display after closing it, turning the unit off and
on clears everything (nothing is saved).

## Step 6 test file: the bat app in its own RTOS task (installed 2026-11-03: smoother, PITCH freezes)

**Result** (user's X8, 2026-11-03). Much smoother and more responsive. The
task was created. In HET the strip read "dsp 73% draw 20%" (wall time), so the
app used most of the time even in its cheapest mode. Choosing PITCH froze the
screen and the unit would not power off; STOP brought it back. In PITCH the
headphones gave a short buzz about once a second, and the other modes gave no
audible output. The DSP task processed everything waiting before it slept, so
in a mode slower than real time it never slept and starved Qt (priority 17),
which also handles powering off. Pressing MARK three times in MANUAL without a
file only showed the stock "no file to mark" pop-up, which then took the next
presses; it worked from the launcher. With one microphone on input 3 (phantom
on 3 only), meters 3 and 4 both moved, 4 more than 3, so the input word
mapping is in doubt. A dog repeller showed on the waterfall up to about 40 kHz.

`work/patch4/#PortaX8_0222.203` (B222, SHA-256 `45998666…7a200ac`). Changes
from step 5:

- **A DSP task.** The first time the bat screen opens, the patch creates an
  RTOS task with the kernel's `cre_tsk` (0x2d4a2: ids 1 to 23 allowed, -41
  when taken), trying the ids unused at start-up (23, 14, 13, 8, 3), at
  priority 16: below task 1 (15) and every audio and recording task, above
  task 21 (17, a 1 MB stack, probably Qt). It has a 48 KB stack. It processes
  the input ring, redraws about 15 times a second, and sleeps 4 ms (2 or 3
  kernel ticks of 2 ms, `dly_tsk` at 0x1a7f4) after every pass so Qt still runs
  the hidden app. If `cre_tsk` fails, processing falls back to task 1 as in
  step 5, and the strip shows the error.
- **Task 1 only queues.** The key hook queues PLAY, the wheel, MARK and REC as
  actions for the DSP task and no longer runs the app, so buttons get through
  at once.
- **Off-screen drawing.** The app draws into a cached frame in the patch
  region and the frame is copied to the display buffer in one pass, so a
  half-drawn frame is no longer shown.
- **Figures.** The strip on the bat screen shows "task" or "no task", the key
  hook rate, the share of time in processing and drawing (as a percentage of
  wall time, measured with the audio block count), frames waiting in each
  ring, underruns and dropped input.
- **Memory.** The app, the off-screen frame and the stacks (563 KB) are in a
  `.noinit` section that the update file leaves out (the patch writes them
  before reading them), so the file is 170 KB longer than the official one.

**What to expect.** As step 5, but with no "lost" input once processing keeps
up, quick response to PLAY and the wheel, and no blanking. The "dsp" and
"draw" figures say how much of the processor the app needs at 192 kHz.

## Step 7 test file: capped passes, gain, input word levels (installed 2026-11-03: stutters)

**Result.** No crashes, but the audio stuttered in every mode: an 8 ms pass,
then about 13 ms of drawing and a 4 ms sleep, made less audio than the
headphones used. Retesting B222 showed inputs 3 and 4 the right way round and
audible output in COMB (keys, a dog repeller) at higher volume; the waterfall
showed nothing above about 40 kHz.

`work/patch5/#PortaX8_0223.203` (B223, SHA-256 `36f9b8ef…09800f2481`). Changes
from step 6:

- **No more starving.** Each DSP pass stops after 8 ms of wall time, then
  draws (if due) and sleeps 4 ms, so a mode that cannot keep up drops input
  ("lost" rises) instead of freezing the unit.
- **Output gain.** The app's output gets +18 dB, with a -6 dBFS ceiling,
  before the stock headphone gain: bat calls and microphone noise are often
  40 to 60 dB below full scale.
- **MARK.** With the bat screen closed, MARK presses are held back; three
  within 1.5 s, each within 0.5 s of the last, open the bat screen, and
  otherwise the held presses go on to the stock app 0.5 s after the last, by
  calling the stock key handler from task 1 with a MARK message. So the
  trigger works in MANUAL, and a single mark lands half a second late. With
  the bat screen up, MARK goes only to the bat app (a stock pop-up there would
  be hidden, and would take the keys while touch is off).
- **Figures.** Three lines across the top of the bat screen: task, key hook
  rate, DSP and drawing shares, and the core clock (from the Cortex-M7 cycle
  counter, which the audio hook turns on); the share of all cycles spent in
  the stock audio callback and in the patch's part of the audio hook, the
  ring levels, underruns and dropped input; and the peak level of each of the
  12 input words, in dB below full scale, to settle which word is which input.
- **Faster.** Both rings are now in cached memory (the input ring in the patch
  region, the output ring at 0x865e2000 in a cached gap no stock code refers
  to), the app redraws 10 times a second, and the frame goes to the display
  buffer in one block copy.

**What to read off.** With a microphone on input 3 only, which word's figure
rises when you rub or tap the microphone; and in each mode the "dsp", "draw",
"core" and "stock" figures, and whether "lost" keeps rising.

## Step 8 test file: hot code in ITCM, hot state in DTCM (installed 2026-11-03: HET keeps up)

**Result.** HET kept up: "dsp 53% draw 15%", "dry 2 lost 0" (from 73% in
step 6). PITCH did not ("dsp 78%", input lost, stuttering noise). The core
runs at 600 MHz, and the stock audio callback alone takes 32% of the cycles at
192 kHz, the patch's part of the audio hook 4%. Input word peaks with
microphones connected: word 0 (input 3) -68 dBFS, word 6 (input 4) -61 dBFS,
the low-gain words about -78, inputs 5 and 6 about -78, inputs 1 and 2 zero.
The waterfall showed steady lines near 37 kHz and 72 kHz, probably the X8's
known ultrasonic hum. HET and COMB are the priority from here; PITCH is set
aside.

`work/patch6/#PortaX8_0224.203` (B224, SHA-256 `e9e3e7fc…bee7b16c`). The FlexRAM is
288 KB of ITCM and 224 KB of DTCM (IOMUXC_GPR17 = 0xffffeaaa, set by
0x80e3f958). The stock ITCM code ends at 0x31a92 and the stock DTCM data,
zeroed ranges and main stack end at 0x20011cd8, leaving about 90 KB and 152 KB
that no stock code refers to (Ghidra finds no references, and the raw words
that look like addresses there are instruction bytes or image data). Both are
single-cycle memories, with no cache to miss.

- **Hot code in ITCM.** The linker script places the app's processing (36 KB,
  most of the DSP inlined into it), the DSP library, the FFT, libm, the
  snapshot builder and the input loop, 55.8 KB in all, to run at 0x31b00; the
  patch stores it after its header and copies it into ITCM on the first
  opening, before anything calls it. The linker adds 21 long-branch veneers
  between SDRAM and ITCM. A checksum of the ITCM code is checked once a
  second; if anything has changed it, the DSP task stops and the strip says
  "ITCM CHANGED".
- **Hot state in DTCM.** `bat-app` now takes its large buffers (waterfall,
  snapshot history, snapshot: 136 KB, in SDRAM) from the caller, so the app's
  own state (43 KB), the input ring (64 KB) and the DSP task's stack (32 KB)
  fit in DTCM from 0x20011d00.
- **Processing.** Passes process until they catch up again, for at most 30
  ms; input waiting beyond 4,096 frames (21 ms) is then dropped, so a mode
  that cannot keep up plays current sound with gaps. The task sleeps 2 ms
  after each pass.

**What to read off.** The same figures as step 7, in each mode: whether
"dsp" has dropped from about 73% in HET, whether PITCH now keeps up, and
whether "lost" stays still.

## Firmware map (V2.03, in progress)

Found with `tools/x8ram.py` (unpacks the RAM runs) and a headless Ghidra 10.4
project set up by `tools/ghidra/x8_setup.py`; `tools/ghidra/q.sh` decompiles
functions and lists references from the command line. Ghidra's auto-analysis of
the whole application takes about 20 minutes.

**RAM at start-up** (from IAR's init table at 0x817847ac): ITCM code
0x400 to 0x31a92 (202 KB, includes the RTOS's PendSV and SysTick handlers, the
eDMA interrupt handlers and the audio processing), DTCM data from 0x20000000,
initialised SDRAM data at 0x81900000, and seven zeroed ranges, including the
framebuffers at 0x87e00000.

**Display.** The LCDIF is set up by the function at 0x80e2b82c: 320 x 480,
RGB565 (the format the bat app draws), first buffer at 0x87e00000. The zeroed
range there is 681 KB, room for two 300 KB frames. The LCDIF interrupt
(0x80e2b7c8) counts finished frames at 0x81910748. The function at 0x80e2b574
waits for the PXP to finish and then writes the LCDIF's control register; how the
second buffer is switched in is not read yet. Qt for MCUs draws through the PXP.

**Audio.** All the audio runs through SAI1, set up by the ITCM function at
0x2aa48:

| What | Where |
|---|---|
| Input DMA | eDMA channel 9 (DMAMUX source 19, SAI1 receive) from RDR0 (0x403840a0), 12 bytes per request, into three 384-byte blocks at 0x2000e340 |
| Output DMA | eDMA channel 8 (DMAMUX source 20, SAI1 transmit) from three 64-byte blocks at 0x200104c0 to TDR0 (0x40384020) |
| DMA descriptors | Scatter-gather rings in OCRAM: input at 0x20204000, output at 0x20204c00 |
| eDMA handles | `s_EDMAHandle[]` at 0x20010628; the channel 8 and 9 handles at 0x20010234 and 0x2001024c (callback pointer first, then user data) |
| Audio callback | 0x2adfc (ITCM), registered on channel 9 by `EDMA_SetCallback`. Each time an input block arrives it runs one processing routine on that block and the output block after the one playing, then moves both ring indices on (they are at 0x20010264 and 0x20010268) |
| Processing routines | Chosen by a mode byte at 0x2a2ca: 2 runs 0x12368, 3 runs 0x13a68, 4 runs 0x81595d34, 5 runs 0x81337020, 6 runs 0x12a90, anything else runs 0x11380 (the largest, about 800 lines of floating-point code). A second byte at 0x2a2cb, probably the sample rate class, sets 8 or 16 descriptors per input block |
| Who sets the modes | ITCM functions at 0x158b8, 0x15c78, 0x15d7a and 0x15f50 |

**Input block layout** (from the processing routines; checked by reading all
twelve loads in 0x12368). A frame is 12 signed 32-bit words, 4 time slots on
each of 3 SAI receive lines, read RDR0, RDR1, RDR2 in turn, so word w is line
w % 3, slot w / 3. A block holds 2 frames below 176.4 kHz and 4 frames at
176.4 and 192 kHz (8 or 16 descriptors of 12 bytes); the rest of the 384 bytes
is unused. Each input has two words, one per gain path of its ADC; both words of
an input share one gain value in the code:

| Input | Word that follows the gain setting | Other gain path |
|---|---|---|
| 1 | 2 | 1 |
| 2 | 5 | 4 |
| 3 | 0 | 3 |
| 4 | 6 | 9 |
| 5 | 8 | 7 |
| 6 | 11 | 10 |

Inputs 1 and 2 swap when the byte at 0x2a2ce is 1 (probably the mic capsule
position), EXT IN shares the input 5/6 slots, and the raw words of inputs 3 and
4 are inverted. The firmware switches from the first path to the second when the
first nears -3 dBFS, so the first is the high-gain path at normal settings
(fairly sure, not certain).

**Output block layout.** A frame is 2 words, left then right (left/right fairly
sure), 2 or 4 frames per block like the input. The stock code clamps to plus or
minus 1, scales by 2^31 - 1 and multiplies by a negative output gain. The
source is the monitor mix (pointers at 0x2a1ec and 0x2a1f0, normally the mix at
0x200103b0); the 1 kHz test tone and the tuner's oscillator go out the same way.
Only one stereo pair leaves the processor, so the headphones and the line output
probably carry the same signal.

**Mode bytes.** 0x2a2cb is the rate class: 0 for 44.1/48 kHz, 1 for 88.2/96,
2 for 176.4/192 (set in 0x15c78). 0x2a2ca, the processing mode, normally equals
the rate class (0 and 1 run 0x11380, 2 runs 0x12368, so a recording app at
192 kHz runs 0x12368). Some apps override it: the tuner gets mode 5,
metronome-style sample playback probably mode 4, another app mode 6, and USB
audio interface use mode 3.

**Timing.** At 192 kHz a block is 4 frames, 20.8 microseconds: the callback
runs 48,000 times a second, leaving about 12,500 cycles per block at 600 MHz
for the stock processing, the rest of the firmware and any hook. So the bat
DSP cannot run in the callback as it is: a hook there must stay small (copy
inputs 3 and 4 into a ring buffer, and copy the bat audio from another ring
buffer into the output words), while the heavier work (the FFTs for the
detector, waterfall and pitch shift, about 64 to 512 samples at a time) runs
in an RTOS task or a low-priority interrupt. The bat app's code already
accepts any block length, so only the plumbing around it changes.

**Hook idea.** The audio callback is a pointer in RAM (the channel 9 handle at
0x2001024c), not in code; `EDMA_SetCallback` (0x27fd4) is called once, from
the run-once part of 0x2aa48, so a replaced pointer should survive sample rate
changes. A replacement first reads the ring index at 0x20010268 (input block
0x2000e340 + index x 0x180, output block 0x200104c0 + ((index + 2) % 3) x
0x40), calls the stock callback, and then, for frames k = 0 to 3 at 192 kHz,
reads input 3 from word 12k and input 4 from word 12k + 6, and writes the bat
audio to output words 2k and 2k + 1. That needs no change to the packed ITCM
code. Still open: stock code that runs regularly (or when an app opens) to
install the pointer from, and a place to run the heavier DSP.

Full notes from this part of the work are in `work/notes/audio.md` (not in
git).

**Memory protection.** The MPU is set up by 0x80e011d0 (regions 0 to 9; later
regions override earlier ones, and the default map is on for privileged code):
peripherals from 0x40000000; flash 0x60000000 (512 MiB, cached, no execute);
ITCM 0x0 (512 KiB, executable); DTCM 0x20000000 (no execute); OCRAM 0x20200000;
all SDRAM 0x80000000 (128 MiB, uncached, shareable, no execute), overridden by
cached, executable regions for 0x80000000 (64 MiB), 0x84000000 (32 MiB),
0x86000000 (4 MiB) and 0x86400000 (2 MiB). So the application and the patch
region (0x81830000) run from cached SDRAM, while 0x86600000 onwards, including
the framebuffers at 0x87e00000, is uncached and cannot hold code.

**Phantom power** (checked in 0x80da8e4c and 0x81360ae4; details in
`work/notes/phantom.md`). The firmware switches it per input, inputs 3 to 6 as
index 0 to 3, not per pair: four on/off bytes in the settings, four FPGA bits,
and the same index in the UI messages. Stereo link copies the mic/line setting
to the partner input but not phantom; only the factory test mode switches it in
pairs.

- Settings: the settings record is at 0x81e8c87c (returned by 0x80d9ff40),
  phantom on/off is one byte per input at +0x254 to +0x257, and the phantom
  voltage is one byte at +0x1dd (0 = 48 V, the default; 1 = 24 V, fairly sure).
  The record is the `DR-TURBO` record in the settings sectors (16 slots of
  4 KB, byte-sum checksum at +0xffc), loaded by 0x80da0774 and saved by
  0x80da2778 at power-off and from a save task.
- Hardware: FPGA register 6, bit 1 << index, written as a 2-byte frame on
  LPSPI4 by 0x81360b70 (reached through the FPGA driver table at 0x819212b4,
  entry +0x14). The shared supply is FPGA register 3 bit 0x08; the voltage is
  register 3 bit 0x20 with register 10 bit 0x08. Not a processor GPIO pin and
  not an ADC register.
- 0x80da8e4c(index, on) turns the shared supply on first if needed and waits
  until it is ready, sets the input's bit, and turns the supply off when no input
  has phantom on. A supply task (0x80dacbde) mutes all six inputs while the
  supply comes up from off (30 ms, then polls a ready bit every 100 ms). There is
  no ramp and no battery check.
- At start-up and on changing app the firmware does not switch phantom on by
  itself: it marks the input as pending and shows a prompt (UI event 0x87).
- From a patch: post the stock message from an RTOS task with 0x80dd01ee: 20
  bytes, u16 values {0, 0, 0x0ff0, index, 1, 0, ...}, after checking that
  settings+0x26a is 0, the word at 0x8196a444 is 0, and the pending flags at
  0x81e86864 to 0x81e86867 are clear. Then wait for 0x8196866b to return to 0
  and give the microphone time to settle. Calling 0x80da8e4c directly switches
  the supply straight back off unless the settings byte is already 1. Never call
  any of this from an interrupt: the SPI bus lock is skipped there and some of
  these functions wait on the RTOS.
- Plug-in power for inputs 1 and 2 seems to follow the jack-detect inputs
  (FPGA register 8 bit 0x02 and AK5704 register 0x01 bit 0x10, on LPI2C1
  address 0x10 and LPI2C4 address 0x11); no user setting was found. Not
  confirmed against the AK5704 datasheet.

**Buttons, wheel and touch** (checked: the key-name table in 0x810e4244 and
the handler word at 0x80da9a20; details in `work/notes/input.md`). No input
control is on a processor GPIO pin or interrupt; one task (task 9, 0x80daca8e)
polls them all, every 3 RTOS ticks (tick length not confirmed).

- Buttons: one active-low byte read over LPSPI4 from device 5 (chip select is a
  3-bit address on GPIO1_IO17 to IO19), latched by a pulse on register 9 of
  device 6 first. Bit 0 is MARK (code 0x2b), bit 1 REC (0x07), bit 5 HOME
  (0x06), bit 6 PLAY (0x05). POWER (0x11) and HOLD (0x0d) are bits in device 6
  register 2. A change must be seen on two scans in a row; a held button
  repeats.
- Wheel: a signed 8-bit count in device 6 register 0x0c, read every scan and
  summed over 27 scans. Device 6 is probably the FPGA (not confirmed).
- Touch: LPI2C1, address 0x55, 400 kHz, polled every second scan (no interrupt
  pin); writes register 0x12 and reads 4 bytes, single touch, a layout like
  Sitronix's ST1232. Each change goes to 0x80e0364a(x, y, pressed) (called only
  from 0x80df501a), which posts to the Qt event queue at 0x8190faf0. Whether x
  and y need rotating for the portrait screen is not known.
- Events: 0x80d6c058(type, code) posts button and wheel events (1 press,
  2 release, 3 repeat, 5 wheel) to mailbox 1; task 1 (0x80d6c9f0) passes them
  to the current screen handler, the pointer at 0x8196a44c, called at
  0x80d6ceae. A handler swallows an event by writing 0x4d at msg+8 (for the
  wheel, by setting the kind at msg+4 to 9). Touch never goes through mailbox 1.
- Hook with a data change only: the word at 0x80da9a20 (0x80dac0d5) is what
  0x80da98e0 installs as the app screens' shared key handler, and nothing else
  reads it. Pointing it at a wrapper sees every button and wheel event while any
  app screen is open.
- Apps are Qt screens; the app number is a property at root+0xd360 (1 MANUAL,
  2 PODCAST, 3 VOICE, 4 MUSIC, 5 FIELD, 6 TUNER, 7 METRONOME, 8 ASMR). How the
  launcher tells the C side which app is open was not traced.
- Regular code: 0x80dad81c, a 10-byte idle counter called at 0x80e036c4 on
  every pass of the Qt task's loop, outside Qt's lock and its drawing: a good
  place to draw the bat screen or install the audio hook. 0x80e2b574 presents
  each frame Qt draws (buffers at 0x87e00000 + i x 0x4b000) but runs only when
  Qt redraws. Task 2 (0x80da9a24, the level meters) runs about every 50 ms.

**Every one of these input paths also runs in firmware update mode**, where
0x80dfbe94 installs the update screen's key handler in the same slot. A hook in
any of them must check the system mode byte first (rule 2 below).

**Not mapped yet:** the recording apps' screens beyond the app number; the WAV
writer; how to start a new RTOS task (the task table is near 0x8131db00).

## Keeping the bricking risk low

**What can still brick a unit, even when following the rules below:**

1. Losing power or removing the SD card during any update, official or patched. The
   updater erases the bootloader sector first, and a half-written main application
   cannot start update mode. A viewer comment on a teardown video reports a unit
   bricked during an official firmware update.
2. A change to the boot header, the bootloader, or code that runs before or during
   update mode.
3. Patched code that writes to flash, including the settings sectors.
4. A change to code that runs before `main()`, such as C++ static constructors. The
   Qt for MCUs user interface has many of them.

**Rules for patches:**

1. Change only code that runs after `main()` has chosen the system mode, such as
   audio processing and the recording apps' screens. `x8fw.py pack --base` refuses
   changes to the bootloader and to the start-up and update-mode code found so far.
   That list is incomplete (it does not yet cover the SD card, FAT file system and
   text display code that update mode uses), so it is a backstop, not a proof.
2. Make patched code do nothing in update mode. Every hook first reads the system
   mode byte (0x81969DD9 in V2.03) and goes straight back to the original code
   unless the mode is 0 or 1 (normal start). This matters most for input and
   screen hooks: the button scan, the event mailbox, the key handler slot and
   the Qt task all run in update mode too.
3. Turn the bat app on only when a particular file (for example `BAT.CFG`) is on the
   SD card. If the bat app crashes the unit, take the card out, delete the file on a
   computer, and the unit starts as stock.
4. Never write to flash from patched code.
5. Keep the official update file on the SD card's computer copy, ready to install
   from update mode. It installs over a patched image with the same version (see
   the version check under [Update file format](#update-file-format)).
6. Install updates on USB power, not batteries.
7. Optionally, run the hook code in an emulator (for example Unicorn) on the real
   firmware image before flashing it, to catch crashes on a desktop.

**Prove the recovery path before any code patch.** Install the string-only patch
(step 2 of the plan), then hold HOME and REC at power-on and install the repacked
official file. If both work, SD card recovery is proven on that unit.

**Estimate.** Following these rules, a bug in patched code should only ever need SD
card recovery. The remaining risk of a hard brick is about the same as for an
official update: power loss during flashing. This comes from reading the code and
has not been tested on a unit.

**Licensing.** Modifying the firmware voids the warranty. A patched update file
contains Tascam's code and licensed third-party code (Qt for MCUs, the Fraunhofer
MP3 codec), so it should not be published as a file. Publish a patch that users
apply to the official update file they download, or get Tascam's permission.

## Opening the unit and hardware recovery

Two teardown videos are public: [What Dennis Does](https://www.youtube.com/watch?v=ryWlHvm3rk4)
(English, 2022) and [iFixThis](https://www.youtube.com/watch?v=YKcWbtdZgvI)
(Polish, 2025). They show:

- **Opening.** Remove the batteries, SD card and mic capsules. Two screws under the
  rubber feet and six more free the back, which stays attached by the cable for
  inputs 1/2. The logic board has four screws (two also hold ground wires) and sits
  on a board-to-board connector that has to be pried up. The LCD and its ribbon cable
  are under the logic board.
- **Chips.** MIMXRT1062DVL6B processor, LCMXO2-1200ZE FPGA, two SDRAM chips, one
  flash chip, two AK5704 ADCs, OPA1678 op-amps, PCM1754 DAC, TPA4411 headphone amp.
- **Test points.** The logic board has many labelled test points (TP2, TP80, TP83 and
  others are visible in the video). Neither video identifies SWD, JTAG or UART pads.
  No schematic or service manual is published.

Difficulty for someone new to this:

| Task | Difficulty | Notes |
|---|---|---|
| Opening the case and lifting the logic board | Moderate | Screwdrivers and a plastic pry tool. The risks are tearing a ribbon cable, bending the board-to-board connector, and static. Watch the teardown video first. |
| Finding the SWD pads | Hard without a schematic | The processor is a BGA chip, so its balls cannot be probed directly. The SWD signals (SWDIO on GPIO_AD_B0_06, SWCLK on GPIO_AD_B0_07) have to be traced from vias or test points with a multimeter and the chip's ball map. |
| Attaching wires | Moderate to hard | Fine wire (about 30 AWG) soldered to small pads, or a pogo-pin clip. Practise on a scrap board first. |
| Reading and writing flash over SWD | Moderate | A SEGGER J-Link, or a CMSIS-DAP probe such as a Raspberry Pi Pico running Raspberry Pi's debugprobe firmware. SEGGER's tools support the RT1062 with external flash directly; `probe-rs` may need a flash algorithm for this flash chip. If the firmware reuses the SWD pins, connect while holding the processor in reset. Only works if Tascam has not disabled the debug port with fuses. |
| USB serial download mode | Hard | Needs the BOOT_MODE pins changed at power-on, which on this board probably means a resistor or test point. Then NXP's MCUXpresso Secure Provisioning tool or SPSDK (`sdphost`, then `blhost`) over the X8's USB-C port, if that port is wired to the processor's USB OTG1. BOOT_MODE[1:0] = 01 selects serial download; check this in the RT1060 reference manual. |

**Practise on cheap hardware first.** A Teensy 4.1 or NXP's MIMXRT1060-EVKB board
has the same processor, so SWD, serial download and FlexSPI flashing can be learned
there with no risk to an X8.

**The easiest option is to ask Tascam** for a unit with SWD and UART wired out, and
for the pad locations.

Not yet confirmed: whether the boot ROM falls back to serial download mode by itself
when the flash image is invalid. If it does, a unit bricked by an interrupted update
could be recovered over USB without changing the BOOT_MODE pins. Check the boot
chapter of the RT1060 reference manual.

## Plan

**Step 0: recovery path.** Open a unit and photograph both sides of the board. Find
SWD pads, the BOOT_MODE resistors and the debug UART. Read the chip markings for the
processor, SDRAM, flash, FPGA and headphone DAC. Prove the recovery path before
writing anything: read all 32 MiB of flash, and compare its first 25,493,504 bytes
with the flash image from `x8fw.py unpack`.

**Step 1: map the firmware in Ghidra.**

- Load the flash image from offset 0x20000 at address 0x80000000 (ARM Cortex,
  Thumb, little-endian), and the bootloader from offset 0 at 0x60000000.
- Find the ITCM copy loop in the reset handler and add the copied code at its ITCM
  address.
- Apply an i.MX RT1062 SVD file for peripheral register names (from the MCUXpresso
  SDK, or generated by `imxrt-ral`). The chip is an MIMXRT1062DVL6B.
- Name SDK driver functions from their assertion strings and by comparing with the
  SDK source (for example `fsl_sai.c`, `fsl_sai_edma.c`).
- Find: the SAI receive and transmit DMA callbacks, the monitor mix routine (the
  audio sent to the headphones), the WAV writer, the app launcher, the EXPANDED PRE
  REC buffer, the framebuffer addresses, and the code that sets the sample rate.

**Step 2: a patch with no code change.** Change one visible string, raise the build
number, pack the update file with `x8fw.py pack --base`, and install it with the
official update procedure. This confirms that the updater accepts a repacked update
file. Then install the official file again, to prove the SD card recovery path. The
files are built; see [Step 2 test
files](#step-2-test-files-installed-2026-11-02-the-x8-accepts-a-repacked-file).

**Step 3: add code.**

- Rust, `no_std`, target `thumbv7em-none-eabihf`, `panic = "abort"`, no memory
  allocator, linked at a fixed address by a custom linker script.
- Placement, option 1: after the end of the current flash image, running directly
  from flash through the FlexSPI memory map (0x60000000 + offset), with hot loops
  copied into free ITCM or DTCM at startup. This needs the updater to accept a
  longer flash image, which is unknown.
- Placement, option 2: over a built-in image, font or sound file that the bat app
  makes unnecessary, keeping the flash image the same length. Needs those files
  mapped first.
- Calling stock functions: declare them as `extern "C"` function pointers at the
  addresses found in Ghidra, with one address table per firmware version.
- Code that runs from the ITCM, probably including the audio interrupt handlers,
  is stored LZ77-packed in flash and unpacked at start-up. Patching it means
  unpacking that run, editing it and packing it again (qubero decodes the format;
  an encoder is still needed). Patching the code in SDRAM that calls it avoids
  this.

**Step 4: audio hook.** Redirect one call (a `BL` instruction) in the monitor mix
routine to a Rust function `bat_process(input, output, frames)`. The first test
passes the audio through unchanged. The second test adds a fixed 45 kHz heterodyne.

**Step 5: bat app screen.** Draw the bat app's screen directly into the framebuffer,
over one of the recording apps (FIELD or MANUAL), so that the stock recorder keeps
working. Open and close it with a button combination, and read the buttons, wheel
and touch input while it is open. For a first display test, taking over the TUNER
app is simpler, because it already has a live display, but the TUNER app does not
record.

**Step 6: FPGA.** Leave it as it is. Revisit only for experiments above 192 kHz.
The FPGA design cannot practically be recovered from its configuration data, so any
change means writing the FPGA's whole function again from scratch, with its pinout
and timing, and without a schematic. Lattice Diamond (the free licence covers
MachXO2) with Verilog is the reliable toolchain. Yosys with nextpnr and Project
Trellis supports MachXO2, but that support is still experimental. Rust-based
hardware description languages such as RustHDL generate Verilog, but they are not
mature enough to depend on here. The firmware already has an FPGA update path (see
[Start-up and update mode](#start-up-and-update-mode-v203)), which would avoid
needing a JTAG cable to load a new FPGA design.

## Tools needed

- Ghidra (not installed yet)
- Rust target: `rustup target add thumbv7em-none-eabihf`
- `cargo-binutils` or `arm-none-eabi` binutils, for disassembling patches
- `probe-rs` and a debug probe, if SWD is reachable
- NXP SPSDK (`pip install spsdk`), for USB serial download recovery
- A Teensy 4.1, for measuring DSP cycle counts on the same chip family

## Terms

| Term | Meaning |
|---|---|
| FCFB | Tag of the FlexSPI configuration block, which tells the boot ROM how to read the flash |
| IVT | Image vector table: the i.MX RT boot header with the entry point and boot data |
| DCD | Device configuration data: register writes the boot ROM runs before starting the image |
| HAB, CSF | High Assurance Boot, NXP's signed-boot feature. The CSF (command sequence file) holds the signature; a CSF pointer of 0 means the image is not signed. |
| XIP | Execute in place: running code directly from memory-mapped flash |
| ITCM, DTCM | Tightly coupled memory: small, fast RAM inside the Cortex-M7 for code (ITCM) and data (DTCM) |
| SEMC | The i.MX RT's external memory controller, used for SDRAM |
| SAI | Synchronous audio interface: the i.MX RT's I2S/TDM audio ports |
| eDMA | The DMA controller that moves audio samples between the SAI and memory |
| USDHC | The SD card controller |
| SVD file | A description of a chip's peripheral registers, which Ghidra and debuggers use to name them |
| `BL` | The Arm Thumb instruction for a function call |
| `cpsid i` | The Arm instruction that disables interrupts, typical at the start of a reset handler |
| SWD | Serial Wire Debug, the Arm debug port |
| BOOT_MODE pins | Processor pins that select whether it boots from flash or waits for a download over USB |
