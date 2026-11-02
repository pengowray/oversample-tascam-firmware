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
- **Firmware update mode is in the main application.** The bootloader has no SD card
  driver. If a patched main application fails to start, the SD card update cannot
  run, and the unit can only be recovered through SWD or the processor's USB serial
  download mode. Finding one of those is step 0.
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
python3 tools/x8fw.py pack work/v203-patched.bin "work/#PortaX8_0218.203" --version 203 --build 218 --date 2026-11-02
```

`pack` recomputes sum32 in the header and in the flash image's last 4 bytes. Files
in `work/` are ignored by git.

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
| 0x1850000 to 0x1FFFFFF | | Outside the flash image, about 7.7 MiB. Unknown whether the firmware stores anything here. |

**Bootloader.** It uses SDK drivers for clocks, SEMC (the SDRAM controller), LPSPI,
LPUART, GPIO and eLCDIF. Its only messages are display errors ("Display
initialization failed", "Error: LCD INIT ERR SPI ERR"). It sets up the SDRAM and the
display, then starts the main application. It has no SD card driver. The main
application is built to run from SDRAM up to at least address 0x818072E5 (its
default interrupt handler), so the bootloader probably copies almost all of the
main application into SDRAM. This is not yet confirmed in the bootloader code.

**Main application.** Its vector table is at flash image offset 0x20000: initial
stack pointer 0x20011CC8 (DTCM), reset handler 0x8173206D (flash image offset
0x175206C, which starts with `cpsid i` and sets the stack pointer). Most interrupt
vectors point to two-instruction stubs that jump to low addresses such as
0x0002834F. Those addresses are in ITCM, so the startup code copies time-critical
code into ITCM. The audio interrupt handlers are probably among them.

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
| FPGA programming | MachXO3 configuration flash routines (`MachXO3CFMAccessEnable`, `MachXO3CFMErase`, `MachXO3CFMInitializeAddress`, `MachXO3CFMProgramDone`, `MachXO3CFMBypass`) and the message `Write PLD : 0x%08lX`, at flash image offset 0xE44754. Just before them is a table of MachXO3 programming opcodes (0x26, 0x0E, 0xF0, 0x3C, 0x46, 0x70, 0x5E, 0xFF) and a page count of 2,164. |
| Debug log | `fsl_debug_console.c` and the log format `[%s (%d)] ...`, so a debug UART probably exists |
| Source tree | `C:\Perforce\projects\SSR\DR-TURBO\firmware\work\BOOT\...` and `...\work\main\...` |

The FPGA configuration data has not been located. The standard Lattice bitstream
preamble (0xFFFFBDB3) does not appear in the flash image, so the data is probably
stored as raw configuration flash pages.

## Risks

1. **Bricking.** A patched main application that crashes before update mode starts
   cannot be fixed from the SD card. Official updates carry the same risk if power is
   lost during the update.
2. **Boot header and bootloader.** The flash image includes them. Changing those
   bytes risks a unit that does not boot at all.
3. **FPGA configuration.** The main application can reprogram the FPGA's
   configuration flash. A patch that disturbs that code could leave the FPGA
   unconfigured. Leave that code alone.
4. **Licensing.** Modifying the firmware voids the warranty. A patched update file
   contains Tascam's code and licensed third-party code (Qt for MCUs, the Fraunhofer
   MP3 codec), so it should not be published as a file. Publish a patch that users
   apply to the official update file they download, or get Tascam's permission.

## Recovery options on the i.MX RT1060

Both options need the unit opened.

- **SWD with a debug probe** (J-Link or CMSIS-DAP; `probe-rs` supports the i.MX RT
  family). Works if the debug port has not been disabled by fuses. The probe can
  rewrite the FlexSPI flash through a flash loader.
- **USB serial download mode.** With the BOOT_MODE pins set to serial download
  (BOOT_MODE[1:0] = 01), the processor's boot ROM accepts a flash loader over USB.
  NXP's SPSDK tools (`sdphost`, then `blhost`) can then rewrite the flash. The X8's
  USB-C port is probably connected to the processor's USB OTG1 port, because the
  X8 works as a USB device (audio interface and card reader). This needs checking
  on the board.

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
  SDK, or generated by `imxrt-ral`). The RT1061, RT1062 and RT1064 share these
  registers.
- Name SDK driver functions from their assertion strings and by comparing with the
  SDK source (for example `fsl_sai.c`, `fsl_sai_edma.c`).
- Find: the SAI receive and transmit DMA callbacks, the monitor mix routine (the
  audio sent to the headphones), the WAV writer, the app launcher, the EXPANDED PRE
  REC buffer, the framebuffer addresses, and the code that sets the sample rate.

**Step 2: a patch with no code change.** Change one visible string, raise the build
number, pack the update file with `x8fw.py`, and install it with the official update
procedure. This confirms that the updater accepts a repacked update file.

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
MachXO3) with Verilog is the reliable toolchain. Yosys and nextpnr support MachXO2
and MachXO3 experimentally. Rust-based hardware description languages such as
RustHDL generate Verilog, but they are not mature enough to depend on here.

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
