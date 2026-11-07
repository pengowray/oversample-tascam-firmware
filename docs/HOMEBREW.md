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
  second; if anything has changed it, the DSP task stops and the screen freezes
  with no message.
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

## Step 9 test file: the BATS tile, recording, pop-ups (installed 2026-11-03: works)

**Result.** Tapping BATS opens the bat screen over MANUAL. MANUAL's phantom
power question shows first (the pop-up handling gives it the display) and the
bat screen follows once it is answered. REC works as in MANUAL: the REC LED
blinks in standby and lights while recording. Holding HOME returns to the
LAUNCHER. Debug strip: app 2, st 1, dsp 55%, draw 14%. Audio, headphones and
the wrong-rate notice were not tried. The user asked for PLAY instead of MARK
x3 to switch screens (MARK x3 left marks in takes), touch for the mode buttons
and HET tuning, the waterfall from 0 Hz, and a 1:1 mode: step 10.


`work/patch7/#PortaX8_0225.203` (B225, SHA-256 `384bfbf2…7d114cfe`). The bat
screen now opens like an app and works over MANUAL's recorder.

- **BATS tile.** The TUNER tile's handler ends with a tail call
  `b.w 0x80d6946c` (the Qt bridge setter, property 6, Qt app 7) at 0x81732c10;
  the patch retargets it to `launch_hook`, which asks for Qt app 1 (MANUAL, C
  app 2) instead and notes the tap. Task 1 opens the bat screen once the app
  byte 0x81e8ca90 changes (or after 0.5 s if it already said MANUAL; it gives
  up after 5 s). The tile's title and description are the patch's strings: the
  tool repoints the English literal and all 10 language pointers of each, and
  changes the English length in `mov.w r1,#length<<8` (launcher.md section 6).
  The tile keeps the tuner icon, and the TUNER app can no longer be opened
  from the LAUNCHER.
- **Closing.** The bat screen closes when the app byte changes from the app it
  was opened in, so holding HOME returns to the LAUNCHER as in the stock apps.
  A short HOME press goes on to the app (it stops a take or playback). MARK
  three times still shows or hides the bat screen in any app; on the LAUNCHER,
  HOME also closes it.
- **Recording.** REC goes to MANUAL as before. A second key hook replaces the
  recording screen's handler (literal 0x80dd3a24, 0x80dd3f9d), which the
  firmware installs from record standby to the end of a take, so the wheel,
  PLAY and MARK x3 keep working during a take. During a take MARK also goes on
  to the recorder at once, so the file gets a mark where it was pressed (a
  triple leaves two). The status bar shows the recorder's real state from
  0x8196a3e5 (stopped, standby with a blinking dot, recording, paused) and the
  take's length from the recorder block, read directly rather than through
  `FUN_80dae75c`, which writes.
- **Pop-ups.** Pop-ups (No Card, No Armed Track and so on) install their own key
  handler, so they used to sit hidden under the bat screen and take the next
  presses. While task 1's state byte 0x8196a3e0 is 6, the DSP task gives the
  display and touch back; the bat screen returns when the pop-up closes.
- **Sample rate.** Below or above 192 kHz the app gets no input, so a notice
  over the waterfall gives the current rate and how to change it (record rate
  0x81cbe678).
- **Modes.** PLAY cycles HET, COMB, FD and TE; PITCH is hidden on the X8.
- **Debug strips.** Both strips now show `app` (the app byte), `st` (task 1's
  state) and `rec` (the recorder byte).

**What to check.** Tap BATS: does the bat screen open by itself, and what does
MARK x3 show underneath (MANUAL, the tuner, or something half-drawn)? Press REC
twice (standby, then recording) with the bat screen up: the status bar should
blink, then turn red and count; after stopping, is there a 192 kHz file on the
card, with marks where MARK was pressed? Do the wheel and PLAY work during the
take? Does a pop-up (REC without a card) show and then give the bat screen back?
Does holding HOME return to the LAUNCHER? At 48 kHz, does the notice show?

## Step 10 test file: PLAY switches screens, touch, 0 Hz, 1:1 (installed 2026-11-05: works)

**Result.** Works: touch, PLAY swapping screens and the 1:1 mode behaved as
expected. HET, and maybe 1:1, seem slightly stuttery during a take; not yet
checked again. The user asked for the bat app to have its own launcher tile,
so that TUNER keeps working, and for a second tile to play existing recordings
through the bat app: step 11. Still to do: listen to inputs other than 3 and 4,
ideally the ones MANUAL has selected.

`work/patch8/#PortaX8_0226.203` (B226, SHA-256 `006146be…0c17955e`).

- **PLAY** swaps between the bat screen and MANUAL's own screen, in the app
  BATS opened (MANUAL's own playback is not used there; a short HOME press
  stops playback if it starts some other way). MARK x3 is gone, so the bat
  screen opens only from the BATS tile. MARK with the bat screen up marks the
  bat app (in TE, captures a call), and during a take also marks the file;
  with the bat screen hidden, MARK goes to MANUAL unchanged.
- **Touch.** Touch polling now stays on. The patch retargets the touch poll's
  only call to Qt (`bl 0x80e0364a` at 0x80df501a, task 9) to `touch_hook`:
  while the bat screen is up, touches go to the bat app (the release of a touch
  Qt saw start still goes to Qt). A touch down is a tap (mode buttons, the
  menu button, the settings screen, tuning on the waterfall); moving the
  finger on the waterfall keeps tuning HET or COMB. Whether the poll's x and y
  are screen pixels (320 x 480, portrait) is not known: the second debug line
  shows the last touch ("touch down x,y").
- **Waterfall** from 0 to 96 kHz (was 15 to 96 kHz).
- **1:1 mode** plays the input unshifted, for audible sound and to check that
  the input works. PLAY no longer cycles modes; tap the buttons.

**What to check.** Do taps land where the finger is (mode buttons, the menu
button at the top left), and does dragging on the waterfall in HET move the
tuning line? Does PLAY swap screens both ways, also during a take? With mics
and headphones: is there sound in 1:1 and HET, and does the 0 to 20 kHz part of
the waterfall show audible sound?

## Step 11 test file: BAT DETECTOR and BAT PLAYBACK tiles, TUNER back (built 2026-11-05, not yet installed)

`work/patch10/#PortaX8_0228.203` (B228, SHA-256 `6e24482a…c3b6f402`). B227
(`work/patch9`) was the first build of this step; a review found that a tap
in delete mode could delete a preset hidden behind a bat tile, so it was
never installed.

- **Tiles.** The bat app moves from the TUNER tile to the launcher's USER
  SETTINGS 2 and 3 tiles, now BAT DETECTOR and BAT PLAYBACK. TUNER is stock
  again. A ninth app tile would need new compiled Qt code, but the three USER
  SETTINGS tiles are shown only while their slot holds a saved preset. The
  patch replaces the bridge's slot getter (0x80d6a238, the only source of the
  launcher's slot properties) so that slots 2 and 3 always report a MANUAL
  preset, and wraps the launcher's title, six description lines and title
  position so that those two tiles look like app tiles with the patch's texts.
  Both tiles show MANUAL's icon (the icon binding's default). USER SETTINGS 1
  is unchanged.
- **Opening.** The two tiles' handlers end with `b.w 0x80d6a1c0` (load preset
  slot r1); the patch retargets both to `tile_hook`, which opens MANUAL as the
  MANUAL tile does (bridge property 6 = 1), and the bat screen follows as in
  step 9. No preset is loaded. The tap also sets bridge property 4 to 0 (from
  the slot's MANUAL value), as the MANUAL tile does.
- **Presets.** The first free slot (0x80d6a252, read by the apps' save
  screens) now offers only slot 1, so a save cannot go to a slot hidden behind
  a bat tile; with slot 1 in use the save screens see no free slot, as with
  all three in use on a stock unit. A preset already saved in slot 2 or 3 is
  hidden. The bat tiles are never deleted: in delete mode they keep their
  icon and show no delete badge or delete label, a tap ends delete mode (as a
  tap on an app tile does) and opens the tile, and the delete listener ignores
  slots 2 and 3. Since the launcher now always sees a slot in use, delete mode
  can be entered with no real preset, and deleting the last real preset no
  longer ends it by itself.
- **BAT DETECTOR** is the step 10 bat screen, now titled BAT DETECTOR, with
  the tile's name in the wrong-rate notice.
- **BAT PLAYBACK** (experimental). The bat screen is titled BAT PLAYBACK. The
  app hears the X8's own output instead of inputs 3 and 4: the audio hook reads
  the output block after the stock callback and divides by the output gain
  (frame 0 by the gain before the stock routine's once-per-block update), then
  replaces it with the app's output as before. Until a file plays (DSP byte
  0x2a2d9 is 1), MANUAL's output is the live input mix, so the app hears
  silence then. PLAY goes on to MANUAL's player (play or pause the current
  file) and, from MANUAL's screen, brings the bat screen back. A short HOME
  press goes on (it stops playback) and shows MANUAL's screen, to choose
  another file. The output level setting does not change what the app hears.
  While a file plays or is paused, the wrong-rate notice checks the converter
  rate, which follows the file, and says to choose a 192 kHz recording. The third debug
  line shows the engine state (0x81cbe780: 1 playing, 2 recording, 0 idle or
  paused), the DSP play byte, the converter rate and the level the app hears. The status bar
  still shows the recorder, not the player.

**What to check.** Are both tiles on the LAUNCHER, with the right titles and
descriptions placed like an app tile's? Does tapping each open MANUAL and
then the bat screen? Does TUNER work again? In delete mode (press and hold
the LAUNCHER), do the bat tiles keep their icon with no badge, and does a tap
end delete mode and open them? In BAT PLAYBACK with a 192 kHz file: before
PLAY the level on the debug line should be -99; does PLAY start playback
(engine 1, play 1, a level other than -99; a paused file shows engine 0,
play 1)
with bat sound in the headphones, does PLAY pause it, and does a short HOME
press show MANUAL's screen? If playback has its own key handler (as recording
does), the wheel and HOME may not reach the patch while a file plays. With a
48 kHz file, does the notice ask for a 192 kHz recording? With a preset in
USER SETTINGS 1, what does SAVE USER SETTINGS do?

## Step 12 test file: the bat app hears the inputs turned on in MANUAL (built 2026-11-06, not yet installed)

`work/patch11/#PortaX8_0229.203` (B229, SHA-256 `40435549…105dd302`). Includes step 11,
which has not been installed either.

- **Which inputs.** BAT DETECTOR now hears two of the inputs turned on in
  MANUAL (the red tracks on its INPUT screen), through MANUAL's INPUT SELECT
  assignment, read about 10 times a second, so a change takes effect when you go
  back with PLAY. All six tracks are on by default. If more than two different
  inputs are on, it takes inputs 3 to 6 and EXT IN first, in track order, then
  inputs 1 and 2 (the included mics' jacks), so the default is inputs 3 and 4 and
  mics on other inputs need the unused tracks turned off. With one input on, the
  app hears that input alone. With none on, the X8 powers its inputs down, so the
  app hears silence and a notice covers the waterfall.
- **On screen.** The meters are labelled with the inputs heard (1 to 6, EL and
  ER for EXT IN, L and R in BAT PLAYBACK); with one input there is one meter.
  Listen to shows Both inputs, or the input's name (Left and right, Left or
  Right in BAT PLAYBACK), and has no buttons with one input.
- **Debug line.** The third debug line starts with the on-track bits and the two
  input frame words heard, then gives the level of words 0 to 11 in order, for
  example `on 3f w0,6 dB 40 99 ...`, so all twelve now fit on the screen.
- **Polarity.** INPUT 3-4 and INPUT 5-6 are inverted in the raw words; inputs 1
  and 2 and EXT IN are not. The patch reads the raw words, so this matters only
  for Both inputs with one input from each group.
- **ITCM.** `process_input` was inlined into code in SDRAM, so the linker script's
  pattern for it matched nothing. It now runs from ITCM as intended, which may
  help the stutter noted in step 10.

**What to check.** With mics on inputs 3 and 4 and every track on, are the meters
labelled 3 and 4, with sound? Turn off tracks 3 and 4 on MANUAL's INPUT screen
with mics on 5 and 6: do the meters say 5 and 6, and is the sound from them?
With one track on: is there one meter, and sound from that input? Use INPUT
SELECT to put INPUT 5-6 on tracks 1-2: do the labels still say 5 and 6? With a
mic on input 1 or 2 alone, does it work at 192 kHz, and how far up does it reach?
With every track off: does the notice show, and what do words 0 and 6 read on the
debug line? Is the stutter during a take better?

## Step 13 test file: MANUAL's mix, debug figures on a switch, auto level, bat glyph (built 2026-11-06, not yet installed)

`work/patch13/#PortaX8_0231.203` (B231, SHA-256 `ee5d9a37…cd05d248`). Includes steps 11
and 12.

- **Listen to** now chooses between MANUAL's mix and the raw inputs: MANUAL's
  mix (the default), Mix, left side, Mix, right side, then the two inputs from step
  12 (both, or either). The mix is the headphone signal, after MANUAL's faders, pans and
  dual-gain merge; the audio hook reads it from the output block as BAT PLAYBACK
  does. The meters say L and R for the mix. The raw inputs are read before the
  mixer, so faders do nothing to them. Which is better, and whether ultrasound
  survives the mix path, is what this build is for.
- **Formats.** Both reach the app as a 32-bit integer and then a 32-bit float.
  A raw input is the word of the path that follows the input gain, and clips at
  its full scale. The mix comes from the merged 32-bit float tracks, which can go
  above 1.0 as the recorded files do, through the faders and pans, and is then
  clamped to full scale before the output. For inputs 3 to 6 the merged value is
  on the raw word's scale and peaks D dB above it, D being the gain set (at most
  35 dB). So at a fader of 0 dB the mix clips at the same level as the raw word,
  and pulling a fader down gains headroom only up to D dB, so none at a gain of 0.
  For inputs 1 and 2 the mix is (gain - 20) dB relative to the raw word, gain being
  the value set, 0 to 36 dB. Tracks on one side add up, so several together clip
  sooner. By default tracks 1, 3 and 5 are hard left and 2, 4 and 6 hard right at 0
  dB, so a single mic on input 3 is at the raw word's level on the left and silent
  on the right. The default Listen to averages the two sides, -6 dB, which is the
  same as both raw inputs with one active.
- **Silent mix.** When the firmware has the input of every track that is on muted,
  and the reason is one of these two, a notice covers the waterfall. One of MANUAL's
  settings pages left open under the bat screen comes first: the firmware mutes the
  inputs on most of them (ITCM 0x161ec, page byte 0x2a2d1), and ignores REC until it
  closes. Otherwise, with REC PAUSE on (settings+0x194 is 0) and the recorder stopped,
  the fix is REC. The brief mute while an app opens has no notice. The patch reads
  the mute words at 0x29e5c + 0x14 per channel. Whether the raw inputs carry sound with REC PAUSE on
  is not known; no reader of settings+0x194 that the review checked turns input power
  off directly, but the return to the app screen skips `FUN_80da5e94` in that state.
  In MANUAL's default, REC from stop enters the recording guide (transport 9, or
  standby without the guide); the status bar shows it as standby.
- **Debug figures** are off by default, with a switch on the settings screen, and
  replace the last-call panel, so the menu button stays visible. The strip on
  MANUAL's and the LAUNCHER's screens follows the same switch. While they are off
  the interrupt does not measure the input words. Line 1: mode, what is heard, the
  recorder's state, the converter rate, and `auto` if Auto volume is on. Line 2: CPU
  % for the stock callback, the rest of the hook, the DSP task (and its highest
  since the settings changed) and drawing. Line 3: output underruns and lost input
  frames since those settings changed, with the seconds, and the ring levels. Line
  4: on-track bits, the mix's level or the two words heard, then words 0 to 11 in
  dB. Changing the mode, what is heard, Auto volume, the recorder's state or the
  tile restarts the counts, and so does each opening of the bat screen, including
  coming back with PLAY.
- **Auto volume** (off by default, settings screen) changes the headphones only: a
  gain at the end of the app's processing, +24 to -24 dB, aiming for -12 dBFS. The
  level of a call fades with a 1.5 s time constant, so calls in a pass keep the gain
  steady and it rises about 6 dB per second after a sound stops. The gain falls with
  a 1 ms time constant, so the first call after a quiet spell can reach the output
  ceiling for a few ms.
- **Tile icon.** Both bat tiles show a bat from the Oversample logo. The launcher's
  icons are alpha masks drawn in white, so it is one colour, 38 x 34 px
  (`tools/make_glyph.py` writes `bat_glyph.rs`). The patch replaces the two tiles'
  image, height and width bindings. **This is the riskiest part of the build.** If
  the launcher hangs when a bat tile comes into view, recover as in "If it goes
  wrong" in step 3: remove the batteries and USB power, then start update mode.
  B229 has no glyph.
- **Settings screen.** The two stepped panels are 10 px shorter to fit the row of
  two switches.

**What to check.** Both tiles show the bat, centred. In BAT DETECTOR with mics on
inputs 3 and 4 and MIC off: step Listen to through the mix and the inputs and
compare the waterfall above 20 kHz, the noise and the sound. Does ultrasound
survive the mix? Pull a fader down and pan a track: does the mix follow? With REC
PAUSE on and stopped: does the notice show, and is there level on the raw inputs?
Leave a settings page open and press PLAY: does the page notice show? Turn on Debug
info: do the lines update and fit? Record in each mode and note CPU,
dry and lost. Turn on Auto volume: are quiet calls lifted without too much hiss
between them?

## Step 14 test file: headphones keep playing during a take, cheaper analysis, Auto volume with a noise limit, Bats only (built 2026-11-06, not yet installed)

`work/patch14/#PortaX8_0232.203` (B232, SHA-256 `47cc597a…d010bdb2`). Includes steps 11
to 13. No hook, table word or header field changed, so `BUILDS` in `tools/x8patch.py` is
the same as for B231. Nothing in this step has run on a unit. Every figure below is
from a host build, a model or the firmware listing, and estimates are marked.

**Field report on B231** (a real unit, debug strip on, tracks 3 and 4 on). Headphones
were choppy while recording and fine on standby. While recording, the waterfall showed
bright vertical full-band lines, and the detector put tick marks under them. Standby
had neither.

| State, mode | irq | hook | dsp (max) | draw | dry | lost | seconds | ring in | ring out |
|---|---|---|---|---|---|---|---|---|---|
| Standby, 1:1 | 31 | 6 | 55 (57) | 17 | 0 | 0 | 12 | 1,912 | 6,080 |
| Standby, HET | 31 | 6 | 57 (60) | 17 | 0 | 0 | 11 | 1,900 | 7,300 |
| Recording, 1:1 | 43 | 7 | 71 (74) | 21 | 27 | 224,864 | 16 | 5,624 | 6,792 |
| Recording, HET | 43 | 7 | 72 (74) | 18 | 53 | 458,564 | 19 | 3,704 | 4,560 |

**Diagnosis.** Confirmed from the code and tests, not on a unit:

- dsp and draw are shares of wall time in a task at priority 16, so they include time
  taken by the interrupt and the recording tasks. The app's real work on standby was
  about (55 + 17)% x (100 - 37)% = 45% of the CPU. While recording the interrupt and hook
  take 50%, and the SD card writes take more, so the app got about 42% (an estimate).
  It fell behind by 7 to 12%, which matches lost / 192,000 per second (14,000 and 24,000
  a second).
- `drop_backlog` threw away input after each pass, so the app heard a jump in the signal:
  a click in the headphones and a wide-band line on the waterfall. A test confirms that
  such a jump, with no restart of the analysis, gives a false call.
- The listening mode was not the load: analysis was about 92% of `BatApp::process` on
  the host, which is why HET and 1:1 differed by 2% of wall time.

Corrected or added:

- Dropping input never helped the headphones. The frames in the two rings stay constant
  while they run, and that total is the longest hold-up of the task that the headphones
  survive. Each drop lowered it, and only an underrun raised it again. So B231's drops
  probably caused some of its underruns, as well as the jumps (from a model, not measured).
  At SD 16 to 20% of wall time, that model of B231 gives 2.4 to 2.8 dry a second, against
  2.8 in the field for HET.
- B231's `lost` added two different things, drops by the task and frames that did not
  fit in the ring. The strip now shows them separately.

**What changed: less work while recording.**

- `FFT_HOP` goes from 128 to 192, 1,000 FFT columns a second. Hop 256 loses calls and hop
  192 does not. Bat calls in 238 library recordings: 5,399 at hop 128, 5,387 with the
  same hop started 64 samples later (the size of the change from column timing alone),
  5,486 at 192, 4,495 at 256. False bat calls in 91 minutes with no bats: 474, 491, 489,
  319. Length readouts now step in 1 ms, and snapshots have 10 columns per 10 ms.
  `HISTORY` goes from 256 to 192 columns, and `Buffers` from 136 to about 120 KB.
- `to_pixels` runs once per waterfall column on a per-bin maximum, not on every FFT
  column. A bin-to-row table replaces per-bin float maths. The waterfall draw loop has no
  modulo and no per-pixel bounds check. The analysis closures and `Meter::update` are now
  in ITCM (they had landed in SDRAM).
- Host cost in ms per second of audio, with auto-vectorisation off, since the M7 has no
  SIMD. These are proportions, not X8 figures.

| Mode | B231 | B232 | Share | Estimated need on the unit | Margin against 42% |
|---|---|---|---|---|---|
| HET | 12.27 | 5.30 | 43% | 20.5% | 2.0 times |
| COMB | 13.33 | 6.61 | 50% | 24.6% | 1.7 times |
| FD | 15.76 | 8.91 | 57% | 31.4% | 1.3 times |
| TE, waiting | 11.66 | 4.75 | 41% | 19.3% | 2.2 times |
| 1:1 | 11.61 | 4.75 | 41% | 19.3% | 2.2 times |

  The estimates assume Auto volume is off; On or Bats only adds about 3 to 4% of CPU (an
  estimate). A busy pass adds about the same for its snapshots.
- 5 redraws a second while the transport is recording, 10 otherwise. A redraw waits while
  more than 5,120 frames are waiting, for at most 0.5 s.
- The frame is drawn, then a catch-up pass of at most 10 ms (`CATCH_UP_BLOCKS`) runs,
  then the frame is copied to the display, so input does not wait for the whole frame.
  The longest time before the task sleeps goes up by about 10 ms, which only Qt and the
  settings save task can notice.

**What changed: when the task falls behind anyway.**

- **Analysis skip.** Above 5,120 frames waiting (26.7 ms) the app stops analysing, and
  below 2,048 it starts again. HET, COMB, FD, PITCH and 1:1 are bit-identical with or
  without the skip (tested, including with Auto volume on). TE is too, except that it
  does not auto-trigger on a call during a skip; MARK still works. Skipped time shows
  as dark grey columns (`WF_NO_DATA`) with no ticks, each about 3 ms longer than the skip,
  because a restart needs one 512-sample window.
- **Drops.** Input is dropped only when more than 6,144 frames are waiting (`BACKLOG_FRAMES`)
  and the two rings together hold more than 14,336 (`MOST_HELD_FRAMES`, 75 ms), and only
  down to that total. The rings hold 8,192 until an underrun, so a hold-up by a recording
  task no longer drops anything. In the model, 32 ms hold-ups every 250 ms for 10 s gave
  the earlier rule of dropping on input level alone 2,828 frames dropped in 17 jumps and 2
  underruns, and this rule nothing lost and no underruns. With random hold-ups, this rule
  matched never dropping within 1 underrun a minute, and the earlier rule gave 10% more to 3
  times as many.
- **Joins.** After lost input (`input_lost`, from a drop or from frames that did not fit
  in the ring) the sound passed to the listening modes and the TE recording fades
  from where it stopped, over 64 samples. It has to come before the modes, because a step
  in the input passes through their mixers and filters. Biggest step after a 2,048-frame
  jump, against the biggest in the 0.1 s before, median and worst, on 261105_0086: HET
  +5.8 / +22.1 dB on B231, -5.6 / +7.9 dB now; COMB -2.7 / +16.1, -5.9 / +8.0; 1:1
  +26.7 / +40.4, -1.4 / +6.4. A jump in a TE recording no longer raises the biggest step
  in its playback.
- **Underruns.** If the output ring still runs dry, the output falls away from its last
  sample (60 dB in 0.5 ms) instead of stepping to zero, and fades in over 64 samples when
  it starts again. This adds about 12 instructions per block to the hook on its usual path
  (read from the disassembly), about 0.1% of the CPU (an estimate). The silence itself
  still lasts at least 43 ms, while the ring refills.
- **Analysis after a gap.** `SpectrumAnalyzer::skip` restarts the window and keeps the
  sample count, so no column spans a gap and times stay true. `CallDetector::gap` keeps the
  floor. A call still loud when the gap began waits for the first column after the gap.
  If that column is quiet, the call is reported at the length it had reached. If it is
  loud, the sound is taken to have continued, so a steady sound reaches the 100 ms limit
  and becomes background however often it is cut. With 16 ms skipped every 100 ms, a
  steady 6 kHz wide sound gave 253 false bat calls and a gate open 84% of the time before
  this rule, and now none, 0.7%. The cost is that two calls either side of a gap can be
  reported as one. Bat calls missed on 0066 at the same skip pattern did not change (19).
- **Regular pulses.** A run of 8 or more detections with the same spacing within 1 ms, peak
  within 1.5 kHz and up to 3 in a row missing now counts as a steady tone (`CallKind::Tone`),
  because these are not bats. In 286 recordings, runs in recordings of bats stopped at 5.
  12 recordings had runs of 7 to 445, spaced about 10, 17, 33 or 400 ms, at one frequency
  each. Some of those spacings fit the mains, so they may be electrical interference; that
  has not been checked. 261105_0082 goes from 3,572 bat calls to 0, and its Bats only gate
  from open 95% of the time to 0.5%. 261105_0081 goes from 1,134 to 4. Bat calls changed in
  10 of the 286 recordings, none of them the bat walk or the roosts:

| Recording | Bat calls without the rule | With it |
|---|---|---|
| 261105_0082_3-4 | 3,572 | 0 |
| 261105_0081_3-4 | 1,134 | 4 |
| 260225_0051_2 | 1,015 | 855 |
| 20260130_233618 | 462 | 277 |
| 260225_0051_5-6 | 314 | 272 |
| 20260131_205322 | 107 | 67 |
| 20260130_222228 | 94 | 55 |
| 260225_0052_5-6 | 69 | 64 |
| 261105_0083_3-4 | 62 | 26 |
| 261105_0084_3 | 36 | 12 |

  What made these sounds has not been identified. Please check them against what you know of
  those nights. A constant-frequency bat calling at perfectly regular spacing for 8 or more
  calls would now count as a tone. The survey found none, but a test of exactly that case
  (`narrow_calls_with_gaps_between_them_are_bat_calls`) had to be shortened from 10 calls to 7 to
  keep passing. The first 7 pulses still count as bat calls, so such a sound opens Bats only for
  about a second when it starts. A pulsed sound with changing spacing, such as a cricket, is still
  taken for bat calls. The Steady tones row still describes only tones.

**Debug strip.** Line 3 is now `dry 0 lost 0 skip 0 in 12 s | ring in 1900 out 7300`.
`lost` counts frames that did not fit in the input ring, in steps of 4. `drop N` appears after
it only once the task has dropped input, because this should almost never happen. `skip`
counts chunks of 256 samples (1.33 ms each, 750 a second) not analysed. `dsp` now includes
the catch-up pass, `draw` covers drawing and the copy, and `ring in` is read before the
catch-up pass. Line 1 says `auto on` or `auto bats only`. All counts restart when the
settings change. Line 3 fits the 320 px screen at realistic counts, 317 px with `drop` showing. At
every count's old maximum it is 329 px wide without `drop` and 365 px with it, and the end of the
line is cut off.

**Auto volume** is now a row of three choices on the settings screen, Off, On and Bats only.

- **On** runs a gain stage that aims for -12 dBFS at the headphones, as in step 13, and
  also limits the gain so the background noise comes out at most 36 dB below that
  (`NOISE_BELOW_TARGET_DB`), about -46 to -48 dBFS. That can take the gain below 0 dB. The
  floor follows the 10 ms level of the output (`FLOOR_WINDOW_S`), in dB, rising with a 2 s time
  constant and falling with 0.5 s, so a 5 ms call 40 dB above it raises it about 0.2 dB. Windows
  below -120 dBFS (`SILENCE_DB`) leave it alone, so sound after a digital mute is not lifted.
  Changing the mode or Listen to sets the floor again from the next window, so the old mode's
  level does not apply for seconds. After HET to 1:1, the output reaches -40 dBFS in about 2 s. A
  new steady sound takes about 6 s to settle. FD and TE have no floor, so On matches step 13
  there: most of what TE plays is low frequencies slowed below hearing, and the floor
  would take the median above 300 Hz down by 9 to 21 dB.
- **Bats only** is On plus a gate that is silent between calls. It opens on any kind marked
  on the settings screen, and on a `Single` at least 18 dB above the floor if Bat calls is
  marked. It opens in 5 ms (`GATE_OPEN_S`, square law, no step), stays open 0.6 s after an
  unconfirmed call and 1.0 s after a confirmed bat call (`GATE_HOLD_S`, `GATE_HOLD_BAT_S`), and
  closes in 200 ms. The hold counts only analysed sound, so a skip does not close it. The
  event arrives after a call ends, so typically the first 2 calls of a pass are not heard.
  TE ignores the gate.
- Levels at the headphone stage at the stock headphone gain, B231 to On. HET is tuned to the
  median bat peak, or 45 kHz. Quiet gain and noise are the median of quiet 10 ms windows. Call
  peak is the median peak of each detected call. These are judged from figures, not by ear.

| Recording | Mode | Quiet gain, dB | Noise, dBFS | Call peak, dBFS |
|---|---|---|---|---|
| 261105_0086, night, no bats | HET | +24 to +18.4 | -40.6 to -46.4 | -26.3 to -33.0 |
| | COMB | +12.5 to -6.0 | -28.0 to -45.8 | -20.1 to -34.1 |
| | 1:1 | -0.4 to -22.1 | -24.8 to -45.4 | -20.3 to -36.5 |
| 260315_0067, bat walk, low input gain | HET | +24 to +24 | -56.2 to -56.2 | -38.9 to -39.1 |
| | COMB | +24 to +12.0 | -34.6 to -46.3 | -22.9 to -34.4 |
| | 1:1 | +18.1 to -3.3 | -24.6 to -45.9 | -16.8 to -37.8 |
| web/samples/0066 | HET | +24 to +24 | -55.8 to -55.8 | -37.3 to -37.3 |
| | COMB | +23.8 to +8.6 | -32.2 to -46.4 | -22.1 to -35.1 |
| | 1:1 | +13.3 to -6.8 | -25.6 to -46.1 | -17.2 to -38.7 |
| web/samples/roost-54khz | HET | +1.7 to -7.7 | -38.7 to -48.2 | -17.3 to -28.0 |
| | COMB | -8.0 to -23.9 | -30.4 to -46.3 | -17.7 to -32.5 |
| | 1:1 | -18.7 to -24.0 | -25.6 to -31.3 | -18.5 to -22.8 |
| 260319_0069, 392 s, no detections | HET, COMB, 1:1 | +24 to +24 | -59.4, -51.3, -51.7, unchanged | |

  Calls get quieter along with the noise, because a call must be 36 dB above the background
  to reach -12 dBFS. "Auto volume got quiet" is the likely first reaction. On the loud roost
  file, 1:1 sits at the -24 dB limit.

| Bats only, HET | Open | Openings a minute | Bat calls heard whole | Missed per pass, before / after the first heard |
|---|---|---|---|---|
| 260319_0069, nothing | 0% | 0 | none in file | |
| 261105_0086, night, no bats | 1% | 0.8 | none in file | |
| 261105_0082, regular pulses | 0% | 0 | none in file | |
| 260225_0053, daytime, no bats | 3% | 1.6 | 38 of 70 false ones | |
| 260225_0054, daytime, no bats | 8% | 3.3 | 260 of 415 false ones | |
| 260315_0067, bat walk | 43% | 6.2 | 1,020 of 1,076 | 2.00 / 0.43 |
| web/samples/0066 | 37% | 4.3 | 95 of 98 | 1.50 / 0.00 |
| web/samples/roost-54khz | 49% | 5.0 | 54 of 56 | 2.00 / 0.00 |
| web/samples/roost-echolocation | 100% | 4.0 | 273 of 274 | 1.00 / 0.00 |

**Silent mix notice.** It is now a small solid box in the waterfall's top left corner, and the
waterfall stays in view. With REC PAUSE on and stopped: `Press REC once to listen`. With a page
open in MANUAL: `Press PLAY and close the page open in MANUAL to listen`, on two lines, which also
covers the INPUT screen. With REC PAUSE off, stopped, on MANUAL's main page, after the mix has
been muted for 4 s (`MUTE_NOTICE_AFTER_S`): `Press REC to record and listen`. The firmware
analysis says this last case should not happen. The 4 s is longer than the mute while an app
opens: the firmware mutes every track while it powers up an input rail (`FUN_80da6f98`, 900 ms a
rail, adding the time left from the rail before, up to about 2.9 s), which it does when the last
app left that rail off.

The help line above the switches now reads `If on: marked on the waterfall, played in TE, heard in
Bats only.`

**Tools.** New `bat-app` examples: `hop_compare` (hops against each other, with the null
test) and `auto_volume` (the tables above). `calls.rs` takes `HOP=`, and `screenshot.rs` takes
`SKIP=`, `AUTO=` and `MIX=stopped`. ITCM is 61.3 of 89.2 KB and DTCM 138.4 of 152.8 KB.

**What to check.**

- With Debug info on and mics on inputs 3 and 4, record for about a minute in each mode. Are the
  headphones continuous? Note `dry`, `lost`, `drop` and `skip`. Predicted while recording (an
  estimate): HET dsp about 33, 1:1 about 31, COMB about 42, FD about 57, draw about 10, `lost` and
  `dry` near 0. On standby, dsp about 23 to 25 and draw about 16.
- Record a take on B231 and note the times of the bright lines on the waterfall. On a computer,
  look at the WAV at those times. The clicks should be absent from the file, which means the
  lines came from the patch and the recording was not affected. A click in the file would mean
  the recording is affected too, so stop using these builds for takes and report it. Check a B232
  take the same way at any grey columns.
- While recording, are there grey columns, and how many? `skip` above 0 with `lost` at 0 means
  analysis stopped and the sound carried on. FD is the mode most likely to skip.
- Does the screen feel too slow at 5 redraws a second, and do REC, MARK and touch still respond?
- Does the unit power off normally during a take? Qt waits up to about 10 ms longer in the worst case.
- Auto volume On, in quiet, in HET, COMB and 1:1: is the hiss acceptable, are calls loud enough, and
  is 36 dB right? With REC PAUSE on, does debug line 4 read `mix -99 dB`? If it does not, the floor
  falls during the mute and hiss is loud for up to about 6 s after REC.
- Bats only on a real pass: how many calls go by before the sound starts, and does it stay open
  through the pass? Does walking or handling open it? No recording made while walking with no bats
  has been checked. Does the gate sound clean?
- Does `Press REC to record and listen` ever appear? If it does, note what was on screen before it.
- Open BAT DETECTOR from the LAUNCHER after VOICE with REC PAUSE off: no notice should flash.

## Step 15 test file: stereo headphones in HET, COMB and 1:1 (built 2026-11-07, not yet installed)

Install B234 (Step 16) in place of this file. B234 changes MANUAL's mix with one input on or none, and
Step 16 lists those changes. The sizes below are B233's. The rest of this step applies to B234 as well.

`work/patch15/#PortaX8_0233.203` (B233, SHA-256 `659bdb02…eb9a410b`). Includes steps 11
to 14. The last build installed on a unit is B231, so the "What to check" lists of steps 11 to
14 apply to this build as well as the list below. No hook address, table word or header field
changed, so `BUILDS` in `tools/x8patch.py` is the same as for B232. Nothing in this step has run on a
unit, and nobody has heard the stereo. Every figure below is from a host build, a model or the
firmware listing, and estimates are marked.

**What changed.** With both sides chosen in Listen to, HET, COMB and 1:1 play the first input
buffer passed to `BatApp::process` in the left ear and the second input buffer in the right ear.
This covers a pair of inputs, MANUAL's mix, and BAT PLAYBACK's left and right. There is no new setting, and
these three modes no longer have a way to play the average of two inputs. So the Step 13 line
"The default Listen to averages the two sides" now holds only for the analysis and for FD and TE. In
B234, these three modes also play the average on MANUAL's mix with one input on or none (Step 16).

| What | Heard or analysed as | Why |
|---|---|---|
| HET, COMB, 1:1 with both sides chosen | Stereo | The request for this step |
| HET, COMB, 1:1 with one side chosen, or one input on | One chain, copied to both ears | Costs the same as B232 and runs no second filter |
| FD | Mono: the average, or the chosen side | Stereo would need about 46% of the CPU while recording (an estimate), against a budget of about 42%. Figures from the unit decide this: see "What to check". |
| TE | Mono | The recording has one channel of storage |
| PITCH | Mono | Hidden on the X8 |
| Waterfall, detector, classifier, TE recording | Mono: the average, or the chosen side | One analysis runs, on the same signal as B232 |

- **HET and COMB.** Each heterodyne voice has one oscillator shared by both channels and one
  low-pass filter per channel. A shared oscillator keeps the left and right phase relationship
  of the input, so a sound equally loud and in phase at both mics stays centred. Retuning moves
  both channels. The mono path is unchanged.
- **The mix.** MANUAL's mix with both sides chosen is always stereo, even with one input on,
  because the two buffers are the mix's left and right. At the default pans (see Step 13), one mic
  on the mix is heard in one ear only. This is a question for the owner, below.
- **Auto volume.** One gain and one gate for both ears, because separate gains would bring left
  and right to the same level and flatten the stereo image. The envelope follows the larger of
  the two absolute values. The noise floor uses each ear's sum of squares over the 10 ms window
  and takes the larger at the end of the window. A mean of the two squares was tried first and
  rejected: it reads 3 dB low with one ear silent, which put the other ear's noise 3 dB above the
  limit. The limit is 36 dB below the Auto volume target, which is -48.0 dBFS at the headphones.
  The table below has the figures. With the same signal in both ears, each ear is bit-identical to
  B233's one-chain path. With one ear silent, the other ear is. Tests check both, with Auto volume
  set to On and to Bats only.
- **Joins.** After lost input, each ear's input fades from that ear's last sample over 64 samples,
  as the mono signal does in B232. `last_ears` keeps the last sample of each chain up to date, so a
  change between mono and stereo starts from what was heard. If a join and a change from stereo
  to one chain happen in the same chunk, the HET or COMB filter starts from the left ear's last
  sample, because that is what the filter last received, and the TE recording still starts from
  the average, as in B232.
- **A change between one chain and two.** Going to stereo, the second filter takes a copy of the
  first filter's state. Each ear's output then fades from where it was over 64 samples
  (`switch_step`), because the right ear otherwise steps from one filter's output to another's.

| Median RMS of 10 ms windows from 3 s on, louder ear, Auto volume On, dBFS at the headphones (limit -48.0) | B232 | Mean of squares | B233 |
|---|---|---|---|
| 261105_0086 HET, right input silent | -47.9 | -44.7 | -47.7 |
| 261105_0086 COMB, right input silent | -47.0 | -44.1 | -47.0 |
| 261105_0083 HET, right input silent | -47.7 | -45.2 | -47.7 |
| 261105_0083 COMB, right input silent | -47.2 | -44.5 | -47.2 |
| 0066 COMB, right input silent | -47.0 | -44.0 | -47.0 |
| 261105_0086 HET, as recorded | -46.9 | -47.1 | -47.9 |
| 261105_0083 HET, as recorded | -47.3 | -47.9 | -48.7 |
| 0066 COMB, as recorded, channels 5.5 dB apart | -45.9 | -45.0 | -46.3 |

**Compared with a build of B232 (8e7b23b), on six recordings.** Each recording ran in each of the
6 modes, with Listen to on both sides (Both), the first side (First) and the second side (Second),
and with Auto volume Off, On and Bats only. Each combination ran twice: once plain, and once with
wheel turns, MARK, analysis skips, and jumps in the input that the app was or was not told about.

| Check | Result |
|---|---|
| Waterfall, snapshots, detections, last call, TE recording and screen | Bit-identical in all 648 comparisons |
| Output with First or Second in all modes, and with Both in FD, TE and PITCH | Bit-identical (part of the 648) |
| One input on, with Listen to set to Both before the other input went off | Bit-identical in 108 of 108, and one chain |
| Identical inputs: Both against First and against B232 | Bit-identical in 108 of 108 |
| Each ear with Auto volume off, against B232's chain on that input alone, with joins and retunes | Bit-identical in 72 of 72 |
| The ear that has a silent input, in HET, COMB and 1:1 | Exactly zero in 108 of 108, so isolation has no figure: it is infinite |
| Tone pairs, phase differences 0 to 170 degrees, levels 0 to -20 dB, after retunes and mode changes | Worst phase error 0.0058 degrees, worst level error 0.0008 dB |
| Stereo output with analysis skips against without | Bit-identical in 45 of 54. The other 9 are Auto volume Bats only, which waits for the detector. B232's own runs with and without skips differ at the same samples on the same recordings |

Size of the click at each event, in dB: the largest change between neighbouring samples in the 400
samples after the event, against the largest in the 0.1 s before it. Each cell gives the median and
then the worst across the test runs. A negative figure means the largest change after the event is
smaller than the largest before it.

| Event | HET | COMB | 1:1 |
|---|---|---|---|
| 2,048-frame jump, B232 | -6.9 / +5.0 | -6.9 / +5.2 | -4.2 / +7.9 |
| 2,048-frame jump, B233 left ear | -6.8 / +7.8 | -6.7 / +9.2 | -3.7 / +7.2 |
| 2,048-frame jump, B233 right ear | -6.9 / +5.1 | -6.7 / +6.9 | -4.5 / +6.2 |
| Both to First with a join in the same chunk, B232, left ear | -5.6 / +6.7 | -4.7 / +7.4 | |
| Both to First with a join in the same chunk, B233, left ear | -5.6 / +2.3 | -5.8 / +2.9 | |
| Listen to between Both and First, B232 | -3.8 / +14.9 | -6.4 / +9.1 | +7.4 / +32.4 |
| Listen to between Both and First, B233 left ear | -7.4 / +4.5 | -7.2 / +2.2 | -4.3 / +9.2 |
| Listen to between Both and First, B233 right ear | -5.1 / +20.7 | -6.0 / +14.7 | -4.4 / +11.4 |

The +20 dB worst figures are in the ear whose sound changes to the other mic. B232's own change
between First and Second gives +20.8 dB. When the sound going into a filter changes at a switch,
the filter's output still moves quickly for a moment: 0.21 of the input's peak in one sample in HET
and 0.30 in COMB. A small tick is possible when Listen to changes during a loud sound.

**Output ring.** The ring now holds frames of `[f32; 2]`, left then right, at 0x865e2000.
`OUT_FRAMES` is 15,360 frames, which is 122,880 bytes and ends at exactly 0x86600000, and a
compile-time assertion requires it. `PRIME_FRAMES` stays 8,192. Evidence that the whole range is
unused by stock code:

- The init table (`work/ram/map.txt`) has a zero run ending at 0x865e1d88 and the next one starting
  at 0x86600000. The highest code literal below the gap is 0x865e1b84.
- MPU region 9 (set up from 0x80e01404 to 0x80e0144c) has RBAR 0x86400019 and RASR 0x03030029: base
  0x86400000, 2 MiB, TEX 0, C 1, B 1, S 0, no subregions disabled. So the ring is cached,
  write-back with no write allocate, and ends at the end of the region.
- `refsin` over 0x865e1d88 to 0x86600004, in `work/ghidra` and `work/ghidra-rec`, finds one
  reference, the reset handler's 0x86600000. `work/notes` does not mention the range.
- A raw scan of the app, ITCM, DTCM and SDRAM runs finds no `movt` with an upper half from 0x8640
  to 0x8660. Five aligned words in the image fall in the range (at 0x801a92c0, 0x8038911c,
  0x803aa74c, 0x803adc34, 0x812337b0). Their neighbours read as data, and the two queried in Ghidra
  have no references.
- The reset handler points the stack at 0x86600000 for one call to 0x80e3f958, which uses no stack,
  then moves it to DTCM. Those are the only two `msr msp` in the image, so nothing is stored there.

This evidence is static. An address computed at run time cannot be ruled out, IAR no-init data
is not listed in the init table, and `work/ram` holds the flash contents and not a RAM dump.
B231 used the range only up to 0x865f1fff, so 0x865f2000 to 0x865fffff (56 KB) has never been used
on a unit.

15,360 is not a power of two, so the ring position comes from `%`. `OUT_WRITE` and `OUT_READ` count
frames in 32 bits, so each wraps at 2^32, after 6.2 hours of sound on the bat screen. When a count
wraps, the ring position it gives moves back 4,096 frames, because 2^32 is not a multiple of 15,360.
The write count wraps first, and the read count follows about 8,192 frames later, which is the usual
fill of the ring. So about 21 ms of sound (the 4,096 frames) plays early and then plays again in its
right place, with a click at each of the two jumps. The `%` keeps the position inside the ring, so
nothing else happens. This is stated at `OUT_WRITE` in `state.rs`.

The input ring, the chunk being processed and the output ring can together hold up to 17,407 frames:
8,191 in the output ring before it starts playing, 1,024 in the chunk and 8,192 in the input ring.
The output ring holds 15,360, so not all of those would fit. Two rules cover this:

- `process_input` leaves input in the input ring while the output ring has no room. It used to
  throw away processed output, with no fade.
- `drop_backlog` has a second rule. As well as its Step 14 rule, it drops whatever is above 15,360
  frames across both rings, even when fewer than `BACKLOG_FRAMES` (6,144) are waiting in the input
  ring, and it tells the app, which fades the jump.

| Block-by-block model of both rings: 264 cases, 792 simulated minutes, task speeds 1.05 to 5 times real time. Hold-ups and speeds are model inputs, not unit figures. | B232 | New ring, B232's rules | B233 |
|---|---|---|---|
| Frames thrown away after processing | 252, in 8 events | 82,940, in 2,796 events | 0 |
| Underruns | 88,926 | 88,933 | 88,936 |
| Frames dropped, with a fade | 90.64 M | 90.61 M | 90.89 M |

This model reproduces Step 14's figure (32 ms hold-ups every 250 ms: no drops, no underruns). The
largest rise in drops it shows from B232 to B233 is from 22,773 to 26,775 frames a minute, at a task
speed of 1.33 times real time with 20% of the time in 32 ms hold-ups. That case already has 109
underruns a minute. A simpler model, written first for this step, showed a larger rise at the same
speed: from 35,243 to 177,824 frames a minute with 32 ms hold-ups, and about 224 underruns a minute.
It shows no drops at 1.5 and 2 times real time.
`MOST_HELD_FRAMES` stays 14,336, which is still below the size of the ring.

**Interrupt hook.** The ring's left sample goes to `output.add(2 * k)` and its right sample to
`output.add(2 * k + 1)`. The two float reads, the fade-in, the dry decay and the clamp and scale are per channel, and `OUT_LAST`
is two values. `read % OUT_FRAMES` runs once per hook call and compiles to one `umull` and shifts,
with no division. `OUT_READ` is always a multiple of 4, and so is `OUT_FRAMES`, so the 4 frames of a
hook call never cross the end of the ring and the hook needs no compare. Instructions executed per
hook call, counted from `rustc`'s assembly of the LTO builds of both commits, with `it` as one instruction (no objdump is installed):

| Path through `audio_hook` | B232 | B233 |
|---|---|---|
| Inputs, primed, no fade (the usual path) | 269 | 312 |
| MANUAL's mix or BAT PLAYBACK, primed, no fade | 385 | 428 |
| Dry | 244 | 288 |
| Bat screen closed | 57 | 57 |
| Worst: per-second block, debug peaks, playback, just primed, fade on all 4 frames | 689 | 736 |
| Whole function, static | 712 | 811 |

The stack frame is 64 bytes in both. The only call is the stock callback, with no allocation and no
panic path. The hook still runs from SDRAM, as in B232. Both samples of a frame are written before
the Release store of `OUT_WRITE`.

**Fixed along the way, both present in B232.**

- `PRIMED` was never reset on opening, so frames from the last opening could play at full level for
  up to 20 ms, until the DSP task woke and emptied the ring, and that counted as one `dry`.
  `enter_work` now empties the ring and clears `PRIMED` while `BAT_ACTIVE` is false, when the
  interrupt leaves them alone.
- `new_session` could leave `OUT_WRITE` 4 frames behind `OUT_READ` if the interrupt fell between its
  load and its store. It now repeats until the read count has not moved. `process_input` depends on
  this, because a write count behind the read count would read as a full ring.

**Cost.** Host figures in ms per second of 192 kHz audio, with auto-vectorisation off, best of 4
runs of `bench`, which plays its noise with calls 15 times in each run. The unit figures are
estimates: Step 14's estimated need for that mode, plus 2.93 points of CPU per host ms of difference
from the B232 column here. The 2.93 comes from B231's HET on standby, which read dsp 57 with the
interrupt and hook taking 37%, so it used 57% x 63% of the CPU for 12.27 host ms.

| Mode | B232, Both | B233, one side | B233, Both | B233, Both, Auto volume on | Estimated need on the unit, Auto volume off / on | Margin against 42%, off / on |
|---|---|---|---|---|---|---|
| HET | 5.50 | 5.39 | 5.82 | 6.81 | 21.4% / 24.3% | 2.0 / 1.7 times |
| COMB | 6.79 | 6.73 | 7.97 | 8.93 | 28.1% / 30.9% | 1.5 / 1.4 times |
| 1:1 | 4.94 | 4.82 | 4.85 | 5.89 | 19.3% / 22.1% | 2.2 / 1.9 times |

- With the analysis skipped, the host cost of B232 and then B233 with Both, in ms a second, is:
  HET 1.09 and 1.43, COMB 2.35 and 3.61, 1:1 0.49 and 0.49. So if the task falls behind, the skip should still keep the headphones going in
  all three modes. That has not been tested on a unit.
- The host figures leave out the larger hook and the doubled ring traffic. Estimates: the hook's 43
  extra instructions a block are 0.35 to 1% of the CPU, at 1 to 3 cycles an instruction (B231's strip
  gives about 2.8). 24,000 more cache line fills a second are about 0.3 to 0.4%, at an assumed 80 to
  100 cycles a fill. Twice as many ring writes are at most about 1.1%. Together that is 1 to 2.5
  points of CPU, so COMB with Auto volume on needs about 32 to 33% against about 42%, or about 1.3
  times. That is the margin FD has in B232.
- The one tight case is COMB with Auto volume on during a busy bat pass. On roost-echolocation the
  host figure is 9.59 in B232 and 10.85 in B233. Scaled two ways (Step 14's figure plus 2.93% per ms, and in
  proportion to COMB's row in Step 14's table) that is an estimated 37 to 40% of the unit, or 1.05 to
  1.15 times against 42%, before the 1 to 2.5 points above. B232 in the same case is an estimated
  33 to 36%. Step 14 said a busy pass adds 3 to 4%. On this file it adds about 2 ms a second, which
  is 6 to 7 points, with or without stereo. COMB is the mode most likely to show grey columns.

**Levels on recordings.** Auto volume off, at the headphone stage, median over detected bat calls,
from the `stereo_levels` example. "Ear - mono" compares the louder ear with B232's mono output, and
correlation is between the two ears. The last three columns are medians per call, so they are not
differences between the level columns.

| File and mode | Left, dBFS | Right, dBFS | Mono, dBFS | Left against right, median / 90th percentile, dB | Ear - mono, dB | Correlation, calls / quiet |
|---|---|---|---|---|---|---|
| x8-3-4, HET 47.0 kHz | -67.7 | -65.8 | -71.9 | 4.5 / 8.9 | +4.5 | 0.07 / 0.18 |
| x8-3-4, COMB | -63.2 | -62.2 | -63.6 | 1.0 / 6.0 | +1.7 | 0.37 / 0.75 |
| roost-54khz, HET 53.5 kHz | -25.7 | -28.2 | -30.3 | 2.8 / 8.8 | +3.9 | 0.00 / 0.01 |
| roost-54khz, COMB | -16.9 | -18.0 | -18.9 | 2.1 / 9.1 | +3.6 | 0.09 / 0.15 |
| roost-echolocation, HET 32.0 kHz | -25.9 | -24.9 | -27.7 | 3.2 / 8.5 | +4.0 | 0.00 / 0.21 |
| roost-echolocation, COMB | -22.2 | -23.3 | -24.3 | 3.5 / 9.6 | +3.9 | 0.02 / 0.24 |
| 0066, HET 47.5 kHz | -70.7 | -64.9 | -69.1 | 6.8 / 12.3 | +5.2 | 0.02 / 0.12 |
| 0066, COMB | -57.6 | -53.0 | -56.1 | 4.4 / 14.3 | +3.7 | 0.37 / 0.58 |

- The ears differ by a median of 3 to 7 dB in HET, and 9 to 12 dB at the 90th percentile.
- Correlation is near 0 in HET during calls. That is expected even for one call reaching two mics
  a few wavelengths apart, so it does not prove that the mics heard different sounds. Expect a wide
  image, and not a clean pan.
- In quiet parts of the files, with Auto volume off, the louder ear is 2.6 to 4.1 dB above mono in HET.
  With Auto volume on the difference is 0.2 dB or less on the two roost files, and 2.3 to 2.9 dB on
  x8-3-4 and 0066, where the gain sits at its +24 dB limit. With a channel so noisy that the gain
  sits at its -24 dB limit, that ear is about 6 dB louder than B232's average.
- Share of samples at the output ceiling with Auto volume off, as B232, then B233 left and right:
  roost-54khz COMB 0.59%, then 2.39% and 1.94%; roost-echolocation COMB 0.64%, then 1.36% and 1.25%;
  roost-echolocation HET 0.60%, then 0.65% and 1.22%. There is none on x8-3-4 or 0066, which peak at -24 dBFS or lower. With Auto
  volume on it is 0.002% or less. `OUTPUT_GAIN` and the ceiling are unchanged, because lowering
  the gain would cost every quiet recording and the mono modes.

**What you will notice.**

- Each ear hears its own input in HET, COMB and 1:1. With Auto volume off, the louder ear is about
  4 dB above B232's mono during calls. FD and TE play the average in both ears. With numbered inputs
  chosen and only one of them on, that input sounds the same in both ears. MANUAL's mix is different:
  see the first question below.
- Bats only opens and closes both ears at the same sample.
- With one mic unplugged and both tracks on, one ear has only what the dead input carries, and the
  other ear has the live mic 6 dB louder than B232 with Auto volume off (261105_0083 with the right
  input zeroed, HET: -46.4 dBFS against -52.4). Choosing the live input in Listen to plays it in both
  ears. With both inputs chosen, the waterfall and the detector use the average, so the live mic
  shows 6 dB lower there, as on B232.
- Stepping Listen to while one input is on replaces a stored Both, even if you step straight back.
  `Input 3` then stays chosen when the second input returns, and the Listen to value shows that.
  B232 worked the same way, but the choice now also decides between stereo and one chain.

**Sizes.**

| Item | B232 | B233 | Limit |
|---|---|---|---|
| ITCM | 61.3 KB | 68.9 KB | 89.2 KB |
| DTCM | 138.4 KB | 138.6 KB | 152.8 KB |
| `APP` | 43,368 bytes | 43,640 bytes | |
| `BatApp::process`, inlining the stereo code, in ITCM | 31,508 bytes | 38,676 bytes | |
| `process_input`, in ITCM | 1,124 bytes | 1,316 bytes | |
| Worst DSP task stack, following direct calls | 20,416 bytes | 20,624 bytes | 32,768 bytes |

The DSP task's priority is still 16, and the app still allocates nothing.

**On screen.** Exact text:

| Where | Text | Width |
|---|---|---|
| Under Listen to, a pair of numbered inputs | `Left ear: input 3. Right ear: input 4. Mono in FD and TE.` | 259 px of 298 |
| Under Listen to, MANUAL's mix, Left and right, Both inputs | `Stereo in HET, COMB and 1:1. Mono in FD and TE.` | 240 px |
| Under Listen to, any single choice, or one input on | `Heard in both ears and shown on the waterfall.` | 218 px |
| Under Listen to, No input | (empty) | |
| Under Sounds to mark | `Marked on waterfall, played in TE and Auto volume Bats only.` | 281 px of 298 |
| Silent mix notice with a page open in MANUAL | `Headphones muted. Press PLAY` / `then close the page in MANUAL` | 205 and 202 px, in a box 220 px wide |

The silent mix notice has a 1 px border, because the waterfall's grey columns are the same grey as
its box. The Auto volume label now lines up with the other labels on the screen.

**Tools.** New `bat-app` example `stereo_levels` (the table above). `bench` has a table of one chain
against two. `screenshot` takes `LISTEN=first` and `LISTEN=second`. `auto_volume` plays the average,
so its figures stay comparable with Step 14. `web/README.md` says how to hear stereo in the emulator.

**Questions for the owner.**

- Answered in Step 16. In this build, one mic panned hard left or right on MANUAL's mix, which is the
  default feed, is heard in one ear only. B232 played it in both ears at -6 dB. The owner chose both
  ears. Step 16 also says why the default pans are not settled.
- Inputs 3 to 6 are inverted in the raw words and inputs 1, 2 and EXT IN are not (Step 12). A stereo
  pair with one input from each group, such as inputs 3 and 1 or inputs 3 and EXT IN, has opposite
  polarity in the two ears. It is heard out of phase, most of all at low frequencies and in 1:1. B232
  partly cancelled the same pair in its average, and the analysis still does. Should the polarity of
  one group be corrected? That would change the waterfall and the detector for that pair, which this
  step has kept bit-identical to B232.

**What to check.** Everything in the "What to check" lists of steps 11 to 14 first, because none
of those builds has been installed. Then:

- **Which ear.** With Listen to on `Inputs 3 and 4`, tap the mic on input 3. Is it in the left ear?
  The first sample of each output frame (word 0) is probably the left channel: the firmware shows
  I2S with word 0 in the left slot, but what the FPGA does with the signal after that is unknown. If
  the ears are swapped, MANUAL's mix and BAT PLAYBACK still come out as stock, because the hook reads
  and writes the same word positions. Only `Left ear: input 3` and the match between the on-screen
  meters and the ears are then wrong.
- **HET and COMB in stereo**, on standby and while recording, with mics on inputs 3 and 4 and Debug
  info on, for about a minute each. Is the image useful? Are the headphones continuous? Note `dry`,
  `lost`, `drop` and `skip`, in each mode. These are the corrected dropout checks in Step 14's list.
  `dsp` and `draw` are shares of wall time that include the interrupt and the recording tasks (see
  Step 14), so they read higher than the CPU percentages above.
  Predicted while recording with Both (an estimate): HET dsp about 35, 1:1 about 31, COMB about 50,
  FD about 57, draw about 10, `lost` and `dry` near 0, with up to about 5 points more on each dsp for
  the larger hook and ring traffic. On standby, HET about 2 points and COMB about 6 points above Step
  14's prediction. The strip's `hook` should go from 6 or 7 to about 7 or 8.
- **Auto volume** On and Bats only, in stereo, with a call on one mic only. The two ears should keep
  their level difference, and open and close together. Is the hiss acceptable, and has it moved
  compared with one input chosen?
- **One mic unplugged**, with both tracks on. On the host, a silent input gives an ear with exactly
  zero output, but a real unplugged input carries preamp noise and maybe hum. What does that ear
  sound like, and does Auto volume stay at the level of the live ear?
- **BAT PLAYBACK** in stereo: a two-channel file in HET, COMB and 1:1. Is each channel in its own ear?
  What does a one-channel file do? That was not checked.
- **The top of the ring.** Anything odd in the first minutes that could come from the stock code
  using 0x865f2000 to 0x865fffff: stock features failing, or wrong sound after a long run. `ring out`
  can now read up to 15,360.
- **Changing Listen to** during sound: is the tick acceptable?
- **Whether FD can go stereo.** These figures decide it. Stereo adds FD's stage again, 4.22 host ms a
  second, which is 3.4 times what stereo adds to COMB (1.24). So, with Auto volume off, while
  recording, read `dsp` in COMB with Inputs 3 and 4 and with Input 3 chosen. Call the difference C.
  Then read `dsp`, `draw`, `skip` and `dry` in FD. FD should fit in stereo if FD's dsp + draw + 3.4 x C
  stays below about 85 and `skip` stays near 0. On the estimates here, C is 8 (COMB's predicted 50 here
  against 42 in Step 14), so that is 57 + 10 + 3.4 x 8 = 94, which is why FD stays mono for now.

## Step 16 test file: one input turned on with MANUAL's mix plays in both ears, play button on BAT PLAYBACK (built 2026-11-07, not yet installed)

`work/patch16/#PortaX8_0234.203` (B234, SHA-256 `a2f2ecfc…c08b89e4`). Includes steps 11 to 15. Install
this file in place of B233. No stock hook site or header field changed, so `BUILDS` in `tools/x8patch.py`
needs no change, and all the checks in that tool pass on this file. None of this step's changes has run
on a real recorder.

**What changed.** The app now runs one chain per ear only with two or more inputs turned on in MANUAL
(`wants_stereo`). This now also applies with Listen to on `MANUAL's mix`. With one input turned on and
Listen to on `MANUAL's mix`, HET, COMB and 1:1 average the mix's left and right sides, run that average
through one chain, and send the chain's output to both ears, as B232 did. The owner asked for this in
answer to the first question in Step 15.

**An input turned on is a track turned on in MANUAL, not a mic plugged in.** The patch has no plug
detection. All six tracks are on by default, so with one mic plugged in and the default tracks, the app
takes inputs 3 and 4, and the mix stays stereo. A stereo-linked pair counts as two inputs. For one chain,
turn off every track except the one with the mic.

| Inputs turned on in MANUAL | Listen to on `MANUAL's mix`, in HET, COMB and 1:1, B233 | The same, B234 |
|---|---|---|
| Two or more | Left side in the left ear, right side in the right ear | No change |
| One | Left side in the left ear, right side in the right ear | The average of the two sides in both ears |
| None | Left side in the left ear, right side in the right ear | The average of the two sides in both ears |

In both builds, the notice `No inputs turned on in MANUAL` covers the waterfall when no input is on. Apart
from the line under Listen to (see "On screen"), `Mix, left side`, `Mix, right side`, the numbered inputs,
BAT PLAYBACK, FD, TE, the waterfall, the detector and the TE recording work as in Step 15.

- **Level.** Take one mic on input 3, with only that track on, panned hard left. The right side of the mix
  is silent. With Listen to on `MANUAL's mix`, the mic plays in both ears, 6 dB below its level on the left
  side of the mix. With Listen to on `Mix, left side`, it plays in both ears at its level on that side.
- **Two tracks panned to one side.** The app does not read the pans. With only tracks 1 and 3 on, both
  panned hard left, and Listen to on `MANUAL's mix`, both mics play in the left ear and the right ear is
  silent. With Listen to on `Mix, left side`, both mics play in the left ear and in the right ear.
- **Turning a track on or off.** A track can only change with the bat screen closed, so the number of chains
  changes when PLAY brings the bat screen back. The output fade from that opening already starts from
  silence. `input_lost(0)` now also sets the last output sample to 0, because the fade between one chain
  and two otherwise started from the last sample of the previous opening, which adds a bump of about a
  quarter of that sample for 0.33 ms.
- **CPU cost.** With Listen to on `MANUAL's mix` and one input on or none, one chain runs, so no second
  filter uses CPU time. Step 15's cost column `B233, Both` now applies to the mix only when two or more
  inputs are on. The audio hook's code and the size of the output ring are the same as in B233.

**The default pans are not settled.** Step 13 says that tracks 1, 3 and 5 are hard left and tracks 2, 4
and 6 are hard right by default, whether or not they are stereo-linked. That comes from reading the
firmware, it assumes that MANUAL is DSP app 2, and it has not been checked on a unit. The owner
remembers tracks with STEREO LINK off as panned to the centre, and linked pairs as L20 and R20. If
tracks with STEREO LINK off are at the centre, the two sides of the mix are equal, and:

- One mic sounds the same in B233 and B234, whichever tracks are on. `MANUAL's mix` and `Mix, left side`
  play it at the same level. With one track on, B234 runs one chain where B233 runs two.
- Two mics with STEREO LINK off, with Listen to on `MANUAL's mix`, are both heard in both ears, so there
  is no stereo, although the line under Listen to says `Stereo in HET, COMB and 1:1`. This is the same in
  B233 and B234. Turning on STEREO LINK, or choosing `Inputs 3 and 4`, puts each mic in its own ear.

**Tile icon.** BAT PLAYBACK now shows a play button, and BAT DETECTOR keeps the bat. Both are alpha masks
drawn in the same white (`ICON_COLOUR`), on a canvas of 38 x 34 px. The height and width hooks did not
change, and a compile-time assertion requires the two sizes to match. The triangle is 26 x 30 px before its corners are
rounded to a 1.5 px radius, and about 25 x 28 px as drawn. `tools/make_glyph.py play` writes `play_glyph.rs`. `tools/make_glyph.py` now takes
`bat` as its first argument to write the bat, and writes the same bat pixels as before.

Read back from the built ELF: `TILE_IMAGES` has one 20-byte descriptor per tile, each with colour
0xffffffff, 38 x 34 px, one tile, stride 38, layout 0x1e498, and flags, offset, trim and cache fields of 0.
Each descriptor's pixels match the generator's output. In `rustc`'s assembly, the USER SETTINGS 2 hook writes
the address of `TILE_IMAGES`, and the USER SETTINGS 3 hook writes that address + 20. The descriptor layout and
draw path are the same as for the bat glyph, which B231 showed working on a unit, but this image has not
been drawn on a unit.

**On screen.** One changed string: the line under Listen to with no input on now uses the heading of the
main screen's notice. The other lines already existed, and they now appear in different situations.

| The line under Listen to, for this choice | B233 | B234 |
|---|---|---|
| `MANUAL's mix`, two or more inputs on | `Stereo in HET, COMB and 1:1. Mono in FD and TE.` | No change |
| `MANUAL's mix`, one input on | `Stereo in HET, COMB and 1:1. Mono in FD and TE.` | `Heard in both ears and shown on the waterfall.` |
| `MANUAL's mix`, no input on | `Stereo in HET, COMB and 1:1. Mono in FD and TE.` | `No inputs turned on in MANUAL` |
| `Mix, left side` or `Mix, right side`, no input on | `Heard in both ears and shown on the waterfall.` | `No inputs turned on in MANUAL` |
| `No input` | (empty) | `No inputs turned on in MANUAL` |

**B234 compared with a build of B232 (8e7b23b).** Both builds ran with `MANUAL's mix` offered and chosen in
Listen to, on up to 8 s of each of four recordings: 0066, roost-54khz, roost-echolocation and x8-3-4. The
two sides of the recording went to the two sides of the mix. Each recording ran in each of the 6 modes, with
Listen to on `MANUAL's mix`, `Mix, left side` and `Mix, right side`, and with Auto volume Off, On and Bats
only. On 0066, roost-54khz and roost-echolocation, each run also had an analysis skip of 20 blocks, two
jumps of 517 samples in the input, and 3 detents of the wheel. x8-3-4 lasts 1.25 s, so its 162 runs ended
before any of those events. The main screen was compared every 200 blocks. The line under Listen to is on the
settings screen, so this comparison does not cover it.

| Inputs turned on, and Listen to | Runs | Result |
|---|---|---|
| One, any choice | 216 | Output, main screen, detections and TE recording bit-identical in all 216 |
| None, any choice | 216 | Output, main screen, detections and TE recording bit-identical in all 216 |
| Two, a side of the mix in any mode, or `MANUAL's mix` in FD, TE or PITCH | 180 | Output, main screen, detections and TE recording bit-identical in all 180 |
| Two, `MANUAL's mix` in HET, COMB or 1:1 | 36 | Output differs in all 36, because B234 plays these runs in stereo and B232 in mono. So the comparison can detect a change in the output. Main screen, detections and TE recording bit-identical in all 36 |

**Tests.** New tests in `crates/bat-app/src/tests/stereo.rs` check the following:

- With Listen to on `MANUAL's mix` and one input on or none, each ear's output is bit-identical to one chain
  fed the average of the two sides, in HET, COMB and 1:1, with Auto volume Off, On and Bats only.
- With Listen to on `MANUAL's mix`, one input on and Auto volume off, a tone on one side of the mix is heard in
  both ears, 6.02 dB (within 0.01 dB) below the same tone with that side chosen.
- With the number of tracks on changing between one and two every 1,000 samples, the largest change between
  neighbouring samples in either ear stays under 0.35 of the peak of the louder input. This is the limit
  Step 15 uses for a change of Listen to.
- After `input_lost(0)` and a change between one chain and two, silent input gives exactly zero output.
- An EXT IN pair is stereo, and with one input on the mix, Listen to reaches all three mix choices.

The emulator page has no MANUAL's mix, so none of this has been played there.

**What to check.** Everything in the "What to check" lists of steps 11 to 15 first. Then:

- **The LAUNCHER.** Does BAT PLAYBACK show the play button, centred on its tile and the same colour as
  BAT DETECTOR's bat? If the launcher hangs when a bat tile comes into view, recover as in "If it goes
  wrong" in step 3.
- **The pans.** What pan does MIXER show for tracks 3 and 4 with STEREO LINK off, and with it on? With two
  mics, STEREO LINK off and Listen to on `MANUAL's mix`, is each mic in its own ear?
- **One mic on input 3, with every other track turned off in MANUAL**, Listen to on `MANUAL's mix`, in HET,
  COMB and 1:1. Does it sound the same in both ears? Then choose `Mix, left side`. With the track panned hard
  left, the mic should be about 6 dB louder than with `MANUAL's mix`, in both ears. With the track at the
  centre, it should be at the same level.
- **Input 4's track.** With input 3's track on and Listen to on `MANUAL's mix`, press PLAY, turn input 4's
  track on or off in MANUAL, close the INPUT screen, and press PLAY. With both tracks on and panned apart,
  the left ear and the right ear should play different sound. With one track on, both ears should play the
  same sound. Is the return to the bat screen free of clicks?

## Step 17 test file: the chime test screen (installed 2026-11-07: keeps up with Auto volume off, TE and COMB lose input with Bats only, chime test reads the time)

`work/patch17/#PortaX8_0235.203` (B235, SHA-256 `8649341c…8a744013`). Includes steps 11 to 16. Install this file in
place of B234. No stock hook site or header field changed, so `BUILDS` in `tools/x8patch.py` needs no
change, and all the checks in that tool pass on this file. None of this step's changes has run on a real
recorder.

**What changed.** The X8 can now read the AudioMoth chime from its inputs and show the time straight away,
so testing a chime no longer needs a recording and `decode_wav`. SETTINGS has a new `Chime test` button
beside `Done`, because Debug info fills the half row beside Auto volume on the X8. The button opens a screen
titled `CHIME TEST`. The screen does not change the X8's clock, because we have not found where the firmware
keeps the time.

Below, a channel is one of the two signals the host passes to `BatApp::process`: an input, a side of
MANUAL's mix, or a side of BAT PLAYBACK.

- **The clock.** Large digits show the time from the latest chime read, plus the time since that chime's
  first start symbol began, measured by counting samples, so the digits keep ticking. Below them are the
  date and UTC offset, and how long ago the decoder finished reading that chime. Before any chime is read, the
  digits show `--:--:--`.
- **The table.** One column per channel. The rows show the number of chimes read, the number of chimes that
  failed, the carrier level and SNR of the latest chime read on that channel, and the level in the 17 to 19 kHz
  band now, with a grey bar whose scale runs from -100 to 0 dBFS.
- **The failure line.** In yellow, the reason the latest chime failed. When a chime is read on either channel,
  the line clears, and the `Chimes failed` counts stay.
- **A new test.** Opening the screen starts a new test. Pressing PLAY to come back to the screen also starts
  a new test, because no samples are counted while the bat screen is closed, so the time from the earlier chime
  would be wrong.
- **Lost input.** Frames lost outside a chime are added to the sample count, so the digits stay right. Frames
  lost during a chime can leave a chime that still passes its CRC, with a time wrong by the length of the loss.
  The app counts such a chime as failed, with the line `Input 3: input frames lost during the chime`.
- **The bat analysis stops while the screen is open.** It is the same stop as when the DSP task falls behind,
  but it is not added to `skip`. So the waterfall has grey columns for that period, and no calls are detected. A
  call in progress ends when the screen opens, and is reported then. `Bats only` stays in the state it was in: an
  open gate stays open, and a closed gate stays closed. With Auto volume On, the gain does not change.
- **The headphones keep playing.** With Auto volume off, the output in HET, COMB, 1:1 and FD is bit-identical with
  the screen open and closed. A recording in progress is stock code and carries on.
- **Debug info.** With it on, the debug strip now also shows on this screen, on top of the second help line and the
  lines about the analysis, so the cost can be read on a real recorder. Its figures restart when the screen opens.

**What the decoders listen to.** The `Listen to` setting chooses the feed, as it does for the headphones. Each channel
of that feed gets its own decoder, and the decoders do not use the average, because two mics whose distances to the
phone differ by about 10 mm can cancel at 18 kHz.

| Feed and inputs on | Decoders running | Columns on the screen |
|---|---|---|
| Numbered inputs, EXT IN or BAT PLAYBACK, with two channels | 2 | The two names, such as `Input 3` and `Input 4` |
| Numbered inputs or EXT IN, one on | 1 | One name, such as `Input 3` |
| `MANUAL's mix`, `Mix, left side` or `Mix, right side`, one or more inputs on | 2 | `Mix, left side` and `Mix, right side` |
| No input on | 0 | The notice `No inputs turned on in MANUAL`, and nothing else |

If the mix is muted, `Press REC once to listen` or the other words of the main screen's corner notice replace the
help lines. If the sample rate is not 192 kHz, the main screen's `Wrong sample rate` notice shows, and nothing else.

**Cost.** Figures from this PC, in ms of processing per second of 192 kHz audio, from the new table in `bench`, with
auto-vectorisation off and no chime in the input. On this PC, one decoder costs about 1.9 ms a second with no chime in
the input, and about 2.4 ms a second while chimes play with no gap between them.

| Mode | Main screen, Listen to on two channels | Chime test, two channels | Chime test, one channel |
|---|---|---|---|
| HET | 5.97 | 5.10 | 2.92 |
| COMB | 8.14 | 7.22 | 4.18 |
| 1:1 | 4.88 | 4.09 | 2.31 |
| FD | 9.04 | 8.31 | 6.53 |

So on the PC, stopping the bat analysis saves more time than the two decoders use. The X8's figures may differ, because
the app's processing runs from ITCM and DTCM, and only `FrontEnd::push`, 644 bytes that run for every input sample, was
moved to ITCM. `ChimeTest::listen`, 19,848 bytes, is larger than the 17,632 bytes free in ITCM, so it runs from
SDRAM. The data of the two decoders, about 19 KB each, is in `Buffers`, also in SDRAM. Drawing the screen takes about
0.1 ms per display frame on the PC. If the DSP task falls behind while the decoders are reading a chime, `drop_backlog`
discards input, and that chime fails.

| Item | B234 | B235 | Limit |
|---|---|---|---|
| ITCM | 70.8 KB | 73.8 KB | 89.2 KB |
| DTCM | 141.9 KB | 142.1 KB | 152.8 KB |
| Patch stored in the update file | 206.4 KB | 243.2 KB | |
| `.noinit`, the end of the patch's memory | ends at 0x818d4d88 | ends at 0x818e6f40 | 0x81900000 |

**On screen.** A fresh subagent that had not seen the code drafted this text, and it was then checked against the code.

| Where | Text |
|---|---|
| SETTINGS button | `Chime test` |
| Title | `CHIME TEST` |
| Above the digits | `No chime read yet`, or `Time from the chime. X8 clock not changed.` |
| Under the date | `Last chime read 12 s ago`, `Last chime read 4 min 12 s ago`, `Last chime read 1 h 12 min ago` |
| Table rows | `Chimes read`, `Chimes failed`, `Last carrier dBFS`, `Last SNR dB`, `17-19 kHz now` |
| SNR with no noise measured | `not measured` |
| Failure line | `Input 3: carrier stopped after 6 complete bytes`, `Input 3: CRC check failed`, `Input 3: chime was too long`, `Input 3: two different readings passed CRC`, `Input 3: message refused after CRC passed`, `Input 3: input frames lost during the chime` |
| Help | `On the phone, press "Play chimes for 1 minute".`, `Hold the phone steady near a mic, with the volume up.` |
| About the analysis | `Bat analysis is stopped while this screen is open.`, `The waterfall gets grey columns for this time.`, `A recording in progress continues.` |
| Button | `Back` |

**Tests.** `crates/bat-app/src/tests/chime_test.rs` plays chimes from the Rust port of the encoder through the app. It
checks:

- The time, that it ticks, how long ago the chime was read, and the clock over 200 s.
- That the screen's pixels change as they should: the digits, each channel's column, and a failure on each channel.
- The levels and the SNR with noise under the chime.
- A time zone on each channel, and lost frames before a chime and during one.
- Each row of the "What the decoders listen to" table, with the chime on the channel that would miss it if the wrong
  number of decoders ran.
- A chime cut off and then a chime read, a new test on opening and on `input_lost(0)`, and nothing listening with the
  screen closed.
- The headphones with Auto volume off, the grey columns, `Bats only` staying open and staying closed, a call in
  progress when the screen opens, and that the text fits.

**Emulator.** I played a 12 s two-channel file at 192 kHz through the emulator page. It has chimes starting at 1 s, 3.5 s
and 6 s, and a chime starting at 8.5 s that is cut off before it ends. The screen showed the right time, the digits
ticked, and each channel had its own counts. The emulator does not have MANUAL's mix, so the mix rows have not been run
there. `screenshot` takes `chime` as its mode.

**What to check.** Everything in the "What to check" lists of steps 11 to 16 first. Then, on the phone, open `Bat recorder
setup by sound`, and press `Play chimes for 1 minute` on the `Tascam X8` tab.

- **Reading a chime.** In SETTINGS, tap `Chime test`, and hold the phone near a mic. How close does the phone need to
  be? Do the digits match `Phone's clock` on the page, within about 1 s? The phone's audio delay can add to any difference.
- **The 17 to 19 kHz bar.** Does it rise when the phone plays? What level does it show when a chime is read, and when
  one fails?
- **MANUAL's mix and the numbered inputs.** Do the decoders read a chime on each feed? With Listen to on the mix and REC
  PAUSE on, does `Press REC once to listen` show, and do the decoders read chimes after you press REC?
- **The cost.** Turn Debug info on, use two mics, set Listen to on `Inputs 3 and 4`, and choose COMB. While chimes play,
  note `dsp`, `dry`, `lost` and `drop`, on standby and while recording. Do the headphones play without gaps? Do any
  chimes that read on standby fail while recording?
- **Going back.** Tap `Back` and `Done`. Does the waterfall show grey columns for the period on the test screen, and does
  detection restart? Does a recording made during the test sound the same as one made with the screen closed?

**Results on a unit (2026-11-07).** Read from photos of the debug strip. All are at 192 kHz, with Listen to on `MANUAL's
mix`, both sides, two inputs on (`on 0c`) and Auto volume off, so HET, COMB and 1:1 ran in stereo. These are the first
figures from a unit since Step 13.

| Mode, state | irq | hook | dsp (max) | draw | dry | lost | skip | ring in / out | Predicted dsp (Step 15) |
|---|---|---|---|---|---|---|---|---|---|
| HET, recording | 43 | 9 | 41 (43) | 13 | 0 | 0 | 16 in 10 s | 2,592 / 5,680 | about 35, up to 40 |
| COMB, recording | 43 | 9 | 55 (56) | 12 | 0 | 0 | 120 in 8 s | 2,364 / 5,308 | about 50, up to 55 |
| 1:1, recording | 43 | 9 | 35 (39) | 13 | 0 | 0 | 24 in 15 s | 2,544 / 5,728 | about 31, up to 36 |
| 1:1, recording, 5 s later | 43 | 9 | 37 (40) | 12 | 0 | 0 | 24 in 20 s | 2,964 / 5,308 | about 31, up to 36 |
| HET, standby | 31 | 8 | 29 (30) | 18 | 0 | 0 | 0 in 22 s | 2,012 / 6,260 | |
| COMB, standby | 31 | 8 | 39 (40) | 18 | 0 | 0 | 0 in 9 s | 2,032 / 6,240 | |

- `drop` did not appear in any photo. The strip shows it only when it is above 0, so no input was dropped.
- B231's HET while recording, in Step 14's table, read dsp 72, draw 18, dry 53 and lost 458,564. So Step 14's analysis cuts
  and its skip work on the unit, and the estimates in Steps 14 and 15 were about right. `dsp` is at or just above the top of each predicted
  range. `hook` is 9, against a predicted 7 or 8.
- `skip` counts chunks of at most 256 frames, so COMB, the highest here, skipped at most about 2% of the time. Skips come in
  bursts: 1:1 added none in the 5 s between its two photos. The waterfall showed them as thin grey columns.
- These runs say the patch kept up. They do not explain clicks, if any were heard during them. CPU overload shows in these
  counts, so a click with all of them at 0 has another cause.

**Results with Auto volume on `Bats only` (2026-11-07).** The same setup, read from a second set of photos. The sound was a
dog repeller, pressed on and off, not bat calls. The owner reports that the audio seemed fine. Pairs of rows are the same
take, except standby.

| Mode, state, take time | irq | hook | dsp (max) | draw | dry | lost | skip | ring in / out |
|---|---|---|---|---|---|---|---|---|
| TE playing, recording, 1:07 | 43 | 9 | 77 (82) | 13 | 3 | 13,220 | 2,772 in 13 s | 4,268 / 8,860 |
| TE playing, recording, 1:25 | 43 | 9 | 80 (82) | 11 | 5 | 25,516 | 5,888 in 30 s | 3,264 / 6,312 |
| COMB, recording, 0:14 | 43 | 9 | 71 (71) | 9 | 1 | 0 | 444 in 14 s | 2,760 / 8,332 |
| COMB, recording, 0:23 | 43 | 9 | 71 (71) | 12 | 1 | 1,072 | 808 in 23 s | 3,216 / 6,804 |
| FD, recording, 0:46 | 43 | 9 | 66 (70) | 12 | 0 | 0 | 464 in 17 s | about 2,626 / 7,298 |
| FD, recording, 0:50 | 43 | 9 | 67 (70) | 12 | 0 | 0 | 576 in 21 s | 2,628 / 7,392 |
| COMB, standby | 31 | 8 | 48 (50) | 18 | 0 | 0 | 0 in 29 s | 2,036 / 6,252 |

- `drop` did not appear in any photo. `lost` is input that did not fit in the ring.
- TE analysed about 72 to 76% of the time: `skip` is about 200 chunks a second, at most 256 frames each. It also lost about
  0.5% of its input and ran the headphones dry 5 times in 30 s, and this was not heard as a problem. TE does not auto-trigger
  during a skip, so it can miss calls.
- COMB skipped about 4 to 5% of the time. Between the two photos it lost 1,072 frames, and it had one underrun in the take.
- FD skipped about 3.5% of the time and lost nothing.

Bats only costs more than Step 14's estimate of 3 to 4% of CPU. COMB's `dsp` goes from 39 to 48 on standby and from 55 to 71
while recording. Taking out the interrupt and hook's share, that is about 5.5 and 7.7 points of CPU. The recording figure
includes the dog repeller, and the Auto volume off runs had almost no loud sound, so it is a high estimate.

Host cost, in ms per second of audio with auto-vectorisation off, best of 5 runs on `260223_0048_3-4 -- small selection`
(1.3 s, X8 inputs 3 and 4), at 256 frames a call, both sides:

| Mode | Off | On | Bats only |
|---|---|---|---|
| HET | 8.05 | 9.01 | 8.38 |
| COMB | 10.23 | 11.97 | 11.72 |
| FD | 11.21 | 12.89 | 13.44 |
| TE | 6.98 | 8.83 | 9.85 |
| 1:1 | 6.41 | 7.80 | 7.41 |

**TE costs far more on the unit than on the host.** TE is cheaper than FD on the host, but reads 10 to 13 `dsp` points higher
on the unit. The likely reason, from the code and the memory map and not measured: `TE_STORAGE` is at 0x87f00000, which
the MPU leaves uncached, and playback reads 4 samples per output sample. That is 768,000 uncached SDRAM reads a second
while playing, which the host cannot show. The same ring takes 192,000 uncached writes a second in every mode, because the
TE recording always runs.

Not tested yet: TE with Auto volume off, HET with Bats only, Auto volume On, one input on, runs longer than about 90 s.
Takes have not been checked for clicks in the WAV (Step 14's list).

**Chime test on the unit.** With Listen to on the mix, the screen showed a date, a time zone and a time that ticked. It
read 10 chimes and failed 22 on the left side, and read 16 and failed 15 on the right. The last chime read had a carrier at
-77.9 and -73.9 dBFS and an SNR of +18 and +21 dB. The last failure was `carrier stopped after 1 complete byte` on the
right side. It was not lost frames, so that failure does not point to the CPU. Why about half of the chimes failed is not
known: the carrier was quiet, and the phone's distance and volume were not noted. The other questions in this step's list
are still open.

## Step 18 test file: start on the LAUNCHER after using a bat tile (built 2026-11-07, not yet installed)

`work/patch18/#PortaX8_0236.203` (B236, SHA-256 `fbf2ba14…900f18ee`). Includes steps 11 to 17. Install this file in
place of B235. All the checks in `tools/x8patch.py` pass on this file. This step's change has not run on a real recorder.

**The problem.** The bat tiles open MANUAL with the bat screen over it, so the settings record is saved with MANUAL as the
open app. The next start then showed MANUAL's own screen, and leaving it took a long HOME press.

**What changed.** If a bat tile opened the current app when the unit powers off, the settings are saved with the LAUNCHER
as the open app (settings + 0x214 = 0). The next start opens the LAUNCHER, one tap from BAT DETECTOR. MANUAL opened from its
own tile still comes back as MANUAL. No other stored setting changes: the stock firmware saves this same value when it is
powered off on the LAUNCHER, and factory settings also use 0.

- **The hook.** `power_off_save_hook` replaces the `bl 0x80da2778` at 0x80dca996 in the power-off sequence `FUN_80dca8b8`,
  which runs with interrupts off. It writes 0 to the live app byte, calls the stock save and puts the byte back. No task
  can see the 0, and the save writes to flash the same way as before.
- **MANUAL's settings are kept.** MANUAL's per-app copies in the record are written when each setting changes, and the
  app change handler (case 0xd04 in `FUN_80dbe324`) has no step that stores MANUAL's settings on the way to app 0. So
  skipping that handler at power-off loses nothing.
- **The periodic save is left alone.** The settings save task `FUN_80dd0254` runs alongside task 1, which would read the 0,
  close the bat screen and ignore REC for the length of the flash write. A start after the battery was pulled, with no
  normal power-off, therefore still opens MANUAL.
- **Why not reopen the bat screen at start-up.** That needs a flag that survives power-off, stored in the flash settings
  record or as a file on the SD card. Bat settings are not stored in the recorder's flash, and the patch cannot write files
  to the SD card yet.

**What to check.**

- From BAT DETECTOR, power off and on. Does the LAUNCHER open? Does the same happen with the bat screen swapped to
  MANUAL's screen with PLAY, and from BAT PLAYBACK?
- Open MANUAL from its own tile, power off and on. Does MANUAL open?
- Go to the LAUNCHER with a long HOME press from the bat screen, open MANUAL from its tile, power off and on. Does MANUAL open?
- Change a MANUAL setting from the bat screen's MANUAL (turn on a track, for example), power off from there and on, then open
  MANUAL. Is the change still there?
- Does the unit power off normally with the bat screen up? This change only works if that path runs.

**If it goes wrong.** A hang at power-off needs the battery taken out. The next start then opens MANUAL, from the last
periodic save. Recover as in "If it goes wrong" in the earlier steps.

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

**Apps and the launcher** (details in `work/notes/launcher.md`; the TUNER
tile code at 0x81732be8 checked). The launcher's tiles are compiled Qt code:
a ninth entry is not possible without code, but a tile's label is data (the
per-language pointers at 0x813b0e48) and each tile's handler ends by calling
0x80d6946c(bridge, 6, n, 0) (TUNER: n = 7, at 0x81732c08, then `b.w` at
0x81732c10). The C core keeps the current app in the byte at **0x81e8ca90**
(settings+0x214), in its own numbering: 0 launcher, 1 ASMR, 2 MANUAL, 3 MUSIC,
4 VOICE, 5 FIELD, 6 PODCAST, 7 TUNER, 8 METRONOME, 9 POCKET STUDIO, 10 and 11
its tuner and metronome (MANUAL, MUSIC, FIELD, TUNER, METRONOME and POCKET
STUDIO firm; ASMR, VOICE and PODCAST from indirect evidence). The settings load
writes it too, so a patch should poll it. Recording only works in the
recording apps (C 1 to 6 and 9); TUNER runs its own DSP mode and cannot
record. The sample rate is one global setting. Holding HOME (code 0xae) sends
the launcher's number (0); a short HOME press only stops the transport.

**Tracks and inputs.** MANUAL's on/off for tracks 1 to 6 is bits 0 to 5 of
settings+0x200 (on, not stereo linked) or settings+0x204 (on, stereo linked; MIC
on the INPUT screen is tracks 1 and 2 linked). All six are on by default.
`FUN_80dd4cac` handles a tap on a track number, `FUN_80da70c4` edits the words,
and ITCM 0x161d8, `FUN_80da5e94` (input power and routing) and REC's "No Armed
Track" check read them. An input pair with no track on is powered down. Bits 6
and 7 are the master track. The input pair
a track hears is the byte at settings+0x30 + 0x20 x track (copied from the app's
preset when the app opens): 0 none, 1 INPUT 1-2, 2 INPUT 3-4, 3 INPUT 5-6, 4 EXT
IN, 5 probably USB. Tracks 1, 3 and 5 hear the left input of the pair, and tracks
2, 4 and 6 the right.

**Launcher icons.** A tile's image binding writes a plain image pointer to its
result; pointers with flag bits clear are not reference counted. The descriptors
are in the SDRAM data (lc_manual.png at 0x819a188c). Image: +0 colour (0xAARRGGBB),
+4 width, +8 height, +0xc tile count, +0x10 tile pointer. Tile: +0 pixels, +4 layout
(0x1e498 for 8-bit alpha, format 5), +8 flags, +0xc x and y offset, +0x10 width and
height (16 bits each), +0x14 stride, +0x18 trim bytes, +0x1c cache size, +0x20 cache
entry. **Static constructors write the colour and layout at start-up (the table at
0x81382be4), so `work/ram/sdram-81900000.bin` shows both as 0; a hand-made descriptor
must set them, or the draw path's "textureLayout" assert hangs the unit.** For an area
of 50 pixels or more, the CPU copies the mask into a scratch buffer of ARGB pixels at
0x20200000 (`FUN_80e03294`) and the PXP blends that; smaller areas go to the CPU blender
`FUN_0001ca54`. Both step through the mask by the stride, which needs no alignment. Other
layouts exist: 0x1e488 (4 bytes per pixel, format 0) and 0x1e490 (2 bytes, format 4). Height
and width are separate bindings, and x and y centre the item from them: (parent + 1) / 2
- size / 2 (`FUN_813e0120`). The parent is 50 x 50.

**USER SETTINGS tiles** (cursor 11 to 13). Each is shown only while its slot
holds a preset. The slots are three words at 0x81e9da60, each the Qt number of
the saved app (1 MANUAL, 2 PODCAST, 9 POCKET STUDIO) or 0; the root's
properties at +0xd3a0, +0xd3a8 and +0xd3b0 (and +0xd3b8, any slot in use) are
computed from them through `FUN_80d6a238(bridge, slot)`, also on a change
(`FUN_813e0a58`). The title is "USER SETTINGS " plus the slot number, placed
75 px lower than an app tile's title (`FUN_80f23220`), since these tiles have
no description. The tiles' handlers (0x81733cd0, 0x81733d08, 0x81733d40) end
with `b.w 0x80d6a1c0` (bridge, slot), which sends 0xfe1 to the C core to load
the preset; separate handlers in the same tap (0x80f22d78.., 0x80f22ed0..,
0x80f23028..) set bridge property 4 to 0, 1 or 2 when the slot holds 1, 2 or
9. In delete mode (root+0xd370, entered by holding a finger on the LAUNCHER),
the tap handlers (0x8161d9dc and 0x8161d880 for slots 2 and 3) open a delete
pop-up instead, and after YES the listener at 0x81728120 calls
`FUN_80d6a0f8(bridge, slot, 0, 0)` to delete the preset; a badge binding per
tile (0x80f2a7e4, 0x80f2b2b8 for slots 2 and 3) and a label over the title
(0x80f29138, "USER SETTINGS " plus the number) show in delete mode. `FUN_80d6a252()` gives the first free slot (1
to 3, or 0), for the apps' save screens. The launcher's text bindings are each
called once: the title from 0x814a765c, description lines 1 to 6 from
0x814a7730, 0x814a7804, 0x814a78d8, 0x814a79ac, 0x814a7a80 and 0x814a7b54, all
as (out, binding, root). A literal string is 7 words: the text at +0 and
length << 8 | kind at +0x18 (kind 0 static, 2 and 6 allocated), assigned with
`FUN_80e3ea10` and released with `FUN_80e03154`.

**Recording** (details in `work/notes/recording.md`). The recorder state is
the byte at **0x8196a3e5**: 0 stopped, 2 standby, 3 recording, 4 paused, 5
closing the file, 7 leaving standby, 8 self-timer (1 and 6 medium
confidence). The frames in the current take are `[0x819140c4] + [0x81914178]
- [0x81913f60]` (or the frozen copy at 0x8191448c when the byte at 0x81913f70
is non-zero), at `[0x81914120]` Hz; the record rate setting in Hz is at
0x81cbe678. From record standby until the take ends, the stock firmware
installs a separate key handler for the recording screen, 0x80dd3f9d, from the
literal at **0x80dd3a24** (the only place it occurs), so the app key handler
hook does not see keys while recording; that literal can be redirected the same
way. MARK's pop-up codes are the CAUTION message number + 4: 7 "No File",
9 "No Card", 0x1e "File Protected", 0x1f "File Length", 0x15 "MARK FULL".

**Not mapped yet:** how Qt reads the recording time; the WAV writer.

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
