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

The updater rejects older firmware with "Old Version". The exact rule is unknown,
so give a patched update file a higher build number than the installed firmware.

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

`pack` recomputes sum32 in the header and in the flash image's last 4 bytes. With
`--base`, it lists every changed byte range, refuses changes to the boot header and
bootloader, and refuses changes to the start-up and update-mode code listed in
[Keeping the bricking risk low](#keeping-the-bricking-risk-low) unless
`--allow-boot-path` is given. Files in `work/` are ignored by git.

## Flash image layout (V2.03)

| Flash image offset | Address when running | Contents |
|---|---|---|
| 0x0000000 | 0x60000000 | FlexSPI NOR configuration block (tag `FCFB`) |
| 0x0001000 | 0x60001000 | Image vector table (IVT). Entry point 0x60002000. Boot data at 0x60001020: image start 0x60000000, size 32 MiB. No DCD. CSF pointer 0, so no HAB signature. |
| 0x0002000 to 0x000A83D | 0x60002000 | Bootloader ("BOOT" in Tascam's source paths), about 34 KB. Runs directly from flash (XIP). Initial stack pointer 0x20080000, reset handler 0x6000A0D5. |
| 0x000A83E to 0x000FFFF | | Unused (0xFF) |
| 0x0010000 to 0x001FFFF | | Erased (0xFF) |
| 0x0020000 to 0x184FFFB | 0x80000000 to 0x8182FFFB (SDRAM) | Main application, about 25.4 MB. Flash image offset 0x20000 is address 0x80000000. |
| 0x184FFFC to 0x184FFFF | | sum32 trailer |
| 0x1850000 to 0x1F9FFFF | | Outside the flash image, about 7.3 MiB. Not yet known whether the firmware uses it. |
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
   | 0 | 8 | Not yet identified (possibly a settings reset) |
   | 3 | 5 | Not yet identified |
   | none, and the function at 0x80DA9582 returns true (probably external power) | 1 | Normal start |
   | none | 0 | Normal start |

3. `main()` then starts the RTOS. A system task runs a state machine (0x80DC7A3C)
   that calls the update code in mode 3.
4. **The update procedure** (0x80DFC57E, flash writer at 0x80E23ED4) reads the whole
   update file into SDRAM, descrambles it, checks sum32, turns off the data cache,
   then erases and writes 64 KiB sectors in order, starting at sector 0 (the boot
   header and bootloader) and ending at the last sector of the flash image. It
   retries a sector up to 4 times.
5. **The FPGA is not touched by a normal firmware update.** In mode 3 the update
   procedure calls only the main flash writer. In other modes it descrambles the
   file, checks it, and calls the FPGA programming routine (0x80E24334) up to 3
   times. That path is probably what boot combination 2 (mode 6) is for, with a
   separate FPGA update file that Tascam has not published.

The version check ("Old Version") has not been traced yet.

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
   unless the mode is 0 or 1 (normal start).
3. Turn the bat app on only when a particular file (for example `BAT.CFG`) is on the
   SD card. If the bat app crashes the unit, take the card out, delete the file on a
   computer, and the unit starts as stock.
4. Never write to flash from patched code.
5. Keep an official update file on the SD card, repacked with a build number higher
   than the patched firmware's, ready to install from update mode. Whether the
   updater accepts an equal or lower build number is not known yet.
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
file. Then install the official image repacked with a still higher build number, to
prove the SD card recovery path.

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
