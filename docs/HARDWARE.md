# Portacapture X8 hardware

Reference for the TASCAM Portacapture X8 as it matters to the bat detector patch: the chips, the
inputs, the sample rates, the display and controls, and what is still unknown. For the firmware and
how the patch changes it, see [FIRMWARE.md](FIRMWARE.md). The step-by-step findings behind most
rows are in [HOMEBREW.md](HOMEBREW.md), mainly its [firmware map](HOMEBREW.md#firmware-map-v203-in-progress).

Build numbers such as B224 are test builds of the patch; [FIRMWARE.md](FIRMWARE.md#terms) explains
them and the listening mode names (HET, FD, PITCH and others).

Written on 10 November 2026. The only unit tested shows `F/W: Version 2.03 Build 217` and
`H/W: Version 1.00 Build 010` on GENERAL SETTINGS > SYSTEM > Information. Other hardware versions
have not been seen.

## Sources

Each fact has one source in the Source column:

| Source | Meaning |
|---|---|
| Tascam spec | TASCAM's Reference Manual, Rev H (firmware V2.00), section 19 "Specifications", or other pages of that manual |
| Teardown | Chip markings seen in two public teardown videos (links under [Opening the unit](#opening-the-unit-and-debug-access)) |
| Datasheet | The chip maker's datasheet for a part identified in a teardown |
| Firmware | Read in the V2.03 firmware (build 217) with Ghidra or a disassembler |
| Measured | Measured on the owner's unit, or in recordings made on it |
| Not confirmed | A guess from indirect evidence, or not known at all |

## Main parts

| Part | Details | Source |
|---|---|---|
| Processor | NXP i.MX RT1062, part MIMXRT1062DVL6B. One Arm Cortex-M7 core with a double-precision FPU (FPv5-D16). | Teardown, Firmware |
| Core clock | 600 MHz, read from the Cortex-M7 cycle counter by a test build (B223). | Measured |
| On-chip RAM | 512 KB of FlexRAM, which the firmware splits into 288 KB of ITCM (code) and 224 KB of DTCM (data) by writing 0xffffeaaa to IOMUXC_GPR17. Plus OCRAM at 0x20200000, which holds the audio DMA descriptors. | Firmware |
| Program flash | 32 MiB of FlexSPI NOR flash, one chip, part number not read. Erased and written in 64 KiB sectors. | Firmware, Teardown |
| SDRAM | Two chips on the SEMC controller. The firmware uses addresses from 0x80000000 up to 0x87FFFFFF (the framebuffers are at 0x87E00000), which suggests 128 MiB. A controller window larger than the chips would also allow this, so 64 MiB is still possible. At least about 60 MB is real: the updater reads the whole 25.5 MB update file to 0x822F3AD8. | Teardown, Firmware, Not confirmed (size) |
| FPGA | Lattice MachXO2, part LCMXO2-1200ZE-1MG132C: 1,280 LUTs, no hardware multipliers. The firmware's programming routine writes 2,164 configuration pages, which matches this part. | Teardown, Firmware |
| FPGA's jobs | Read in the firmware: phantom power switches, the button latch and wheel counter (probably; the firmware calls it SPI device 6 on LPSPI4), and the jack detect for inputs 1 and 2. Audio clocking and joining the converters' outputs into one stream are likely but not traced. | Firmware, Not confirmed (audio) |
| ADCs | Two AKM AK5704 chips. Each has 4 channels, mic preamps and rates up to 192 kHz. The firmware writes AK5704 registers at I2C address 0x10 on LPI2C1 and at 0x11 on LPI2C4, probably one chip on each bus. | Teardown, Datasheet, Firmware |
| Analog stages | OPA1678 op-amps in the input stages. | Teardown |
| DAC | TI PCM1754, for the headphone and line output. | Teardown |
| Headphone amplifier | TI TPA4411. | Teardown |
| Real-time clock | A separate clock chip on I2C bus 2, read and set in BCD (date at register 0x32, time at 0x33). The processor's own SNVS clock is not used. Part number not known. | Firmware |
| Touch controller | On LPI2C1 at address 0x55, 400 kHz, polled with no interrupt line, single touch. Its register layout is like Sitronix's ST1232; the part is not identified. | Firmware |

## Inputs

The input frame is the block of 12 signed 32-bit words that the processor receives for each sample
period (see [Audio path](#audio-path-in-the-processor)). Each input has two words, one for each gain
path of its ADC channel pair.

| Input | Connector and rating | Power for mics | Words in the input frame (high gain, low gain) | Source |
|---|---|---|---|---|
| 1, 2 | 3.5 mm TRS mini jacks, unbalanced, 85 kΩ or higher, maximum +1 dBV, minimum −69.3 dBV. The included mic capsules connect here. | Plug-in power. The firmware seems to switch it from the jack detect; no user setting was found. | Input 1: 2, 1. Input 2: 5, 4. The firmware swaps inputs 1 and 2 when its DSP state byte at 0x2A2CE (in ITCM) is 1, probably for the capsule position. | Tascam spec, Firmware |
| 3 to 6 | XLR/TRS combo jacks, balanced. MIC: 2.2 kΩ or higher, maximum +2 dBu, minimum −75 dBu. LINE: 9 kΩ or higher, nominal +4 dBu, maximum +24 dBu. | Phantom power, 24 V or 48 V, XLR only. The firmware switches it per input, not per pair. | Input 3: 0, 3. Input 4: 6, 9. Input 5: 8, 7. Input 6: 11, 10. | Tascam spec, Firmware |
| EXT IN | 3.5 mm stereo mini jack, line level, 8.5 kΩ or higher, nominal −10 dBV, maximum +10 dBV. Cannot be used at the same time as inputs 5 and 6. | None | Uses the words of inputs 5 and 6. One gain path only (Tascam: "does not have dual ADC"). | Tascam spec, Firmware |

- **Polarity.** The raw words of INPUT 3-4 and INPUT 5-6 are inverted; those of inputs 1, 2 and
  EXT IN are not (Firmware).
- **Which converter serves which input is not known.** Six dual-gain inputs need 12 converter
  channels, and TASCAM's block diagram marks inputs 1 to 6 as dual ADC. Two AK5704 chips have only 8
  channels. So inputs 1 and 2 probably use another converter that neither teardown named, and the
  firmware's two AK5704 addresses do not settle it (Not confirmed).
- The bat patch reads the high-gain word of each input it listens to. With all six of MANUAL's
  tracks on (MANUAL's default), it listens to inputs 3 and 4.

## Outputs

| Output | Details | Source |
|---|---|---|
| Headphones | 3.5 mm stereo mini jack, 45 mW + 45 mW into 32 Ω at 0.1% THD+N or less. | Tascam spec |
| LINE OUT | 3.5 mm stereo mini jack, unbalanced, 200 Ω, nominal −14 dBV, maximum +6 dBV. | Tascam spec |
| Built-in speaker | 0.4 W, mono. | Tascam spec |
| Signal | One stereo pair leaves the processor (SAI1 transmit), so the headphones and LINE OUT probably carry the same signal. The bat patch replaces this pair while its screen is open. | Firmware, Not confirmed (LINE OUT) |

## Sample rates and formats

| Item | Details | Source |
|---|---|---|
| WAV recording | BWF, 44.1, 48, 96 or 192 kHz; 16-bit, 24-bit or 32-bit float. | Tascam spec |
| MP3 | 44.1 or 48 kHz only, 128 to 320 kbps. Not available at 96 or 192 kHz. | Tascam spec |
| Channels | Up to 6 input channels and 8 recording tracks (6 tracks and the stereo mix). | Tascam spec |
| Rate classes | The firmware groups rates into three classes, kept in the byte at 0x2a2cb: 0 for 44.1 and 48 kHz, 1 for 88.2 and 96 kHz, 2 for 176.4 and 192 kHz. Class 2 runs a different processing routine and 4 frames per DMA block instead of 2. | Firmware |
| Not available at 192 kHz | EQ; reverb and MP3 dual recording (also at 96 kHz); VSA and KEY playback (also at 96 kHz); USB audio. | Tascam spec |

### 32-bit float and the two gain paths

The two gain paths of each input are merged into one float value in the processor, not in the FPGA
or the converters (Firmware). The merge, read in the firmware and checked against about 280 recorded
channels from the owner's unit (Measured):

- The output is `A × (1 − w) + B × 10^(D/20) × w`, where A is the high-gain word, B the low-gain
  word, D the gap between the paths in dB, and w a weight that fades between 0 and 1.
- The firmware fades to the low-gain path when the high-gain path nears −3 dBFS, and back when it
  stays below −9 dBFS. At 192 kHz a fade takes about 10 ms.
- Each 32-bit word carries 22 bits: the low 10 bits are always zero, so the step is 2^−21 of full
  scale.
- D is half the LEVEL setting in dB, within 0.05 dB in 67 of the 71 channels where both gain paths
  were used.
- EXT IN has one path, so its samples are not merged.
- The built-in mics' channels are not on the 2^−21 grid, probably because the firmware's EQ filter
  for the built-in mics runs after the merge.

Details: [Merge checked against recordings](HOMEBREW.md#firmware-map-v203-in-progress) in HOMEBREW.md.

### Frequency response at 192 kHz

| Item | Details | Source |
|---|---|---|
| TASCAM's figure | Inputs 3 to 6 to PCM data at 192 kHz: 20 Hz to 60 kHz, +0/−2.5 dB (JEITA). | Tascam spec |
| AK5704 digital filter at 192 kHz | Within ±0.12 dB up to 37 kHz; −1 dB at 51 kHz; −3 dB at 65 kHz; −6 dB at 78 kHz; −72 dB or lower from 145.5 kHz. | Datasheet |
| Aliasing | Sound between 96 and 145 kHz is only partly filtered and appears at (192 − f) kHz. A call at 120 kHz shows up near 72 kHz, quieter. | Datasheet |
| Analog circuit above 60 kHz | Not measured. A line-level sine sweep into input 3 would settle it. | Not confirmed |
| Inputs 1 and 2 above 20 kHz | TASCAM gives no figure. They may use a different converter. | Not confirmed |
| Steady ultrasonic tones | Owners report an ultrasonic hum from radio interference. On the owner's unit, a test build's waterfall showed steady lines near 37 kHz and 72 kHz (B224). Whether these were the hum or an outside source was not checked. | Measured, Not confirmed (cause) |
| Dynamic range | Inputs 3 to 6: 113 dB or higher at 48, 96 and 192 kHz (A-weighted, 20 kHz low-pass). | Tascam spec |
| Mic preamp EIN | Inputs 3 to 6: −126 dBu or lower. | Tascam spec |

## Audio path in the processor

| Item | Details | Source |
|---|---|---|
| Serial audio | All audio runs through SAI1: 3 receive lines with 4 time slots each, so 12 words per input frame. Word w is line w % 3, slot w / 3. | Firmware |
| Input DMA | eDMA channel 9 into three 384-byte blocks at 0x2000E340. A block holds 2 frames (96 bytes) below 176.4 kHz and 4 frames (192 bytes) at 176.4 and 192 kHz; the rest of each block is unused. | Firmware |
| Output DMA | eDMA channel 8 from three 64-byte blocks at 0x200104C0. A frame is 2 words, left then right. | Firmware |
| Audio callback | Runs once per input block: 24,000 times a second at 48 kHz, and 48,000 times a second at 96 and 192 kHz. At 192 kHz a block lasts 20.8 µs, about 12,500 processor cycles. | Firmware |
| Stock processing cost | The stock audio callback alone uses 32% of the processor's cycles at 192 kHz (B224). | Measured |

## Processing headroom (measured)

These figures replace the estimates in [FEASIBILITY.md](FEASIBILITY.md). They come from the bat
patch's CPU test on B251 (9 November 2026), with two inputs on, the MIX file off, and the test's
synthetic bat calls on. `dsp` is the
share of wall time that the patch's DSP task spent processing, in percent, with the highest
one-second value in brackets.

| Listening mode | Recorder stopped: `dsp` | Input lost | Recording: `dsp` | Input lost |
|---|---|---|---|---|
| 1:1 mono, Auto volume off | 27 (28) | 0 | 35 (36) | 0 |
| FD, Auto volume off | 40 (41) | 0 | 51 (52) | 0 |
| FD with the phone remote's chime decoders | 50 (50) | 0 | 66 (66) | 0 |
| PITCH mono, Auto volume off | 76 (82) | 0 | 81 (84) | about 2,000 frames per 8 s |
| PITCH stereo, Auto volume off | 87 (88) | about 0.3 million frames per 8 s | 87 (87) | about 0.6 million frames per 8 s |

- HET, COMB, FD, TE and 1:1 keep up at 192 kHz while recording, with nothing lost.
- PITCH's own cost (PITCH mono less 1:1 mono) is 41 to 49 points of `dsp` across the full table,
  measured with the waterfall partly switched off, so the full cost is higher. FD's own cost is 13
  to 16 points.
  FEASIBILITY.md estimated 11 to 16% per channel for PITCH. PITCH is hidden on the X8 for this
  reason.
- The patch's part of the audio interrupt costs about 8% of the processor.

Full tables: Steps 21 and 27 in [HOMEBREW.md](HOMEBREW.md#step-27-test-file-pitch-in-the-cpu-test-in-stereo-too-built-2026-11-08-installed-2026-11-08-hangs-when-a-bat-screen-opens-fixed-in-b251).

## Memory free for the patch

Found by checking that no stock code refers to these ranges (Firmware). Sizes are for V2.03 build 217.

| Memory | Free range | Size | Speed | What the patch keeps there |
|---|---|---|---|---|
| ITCM | 0x00031B00 to 0x00047FFF | 89.2 KB | Single cycle | Hot code: DSP, FFT, the audio hook's copy |
| DTCM | 0x20011D00 to 0x20037FFF | 152.8 KB | Single cycle | App state, input ring, the DSP task's and card reader task's stacks |
| SDRAM after the stock application | 0x81830000 to 0x818FFFFF | 832 KB | Cached | The patch's code and data |
| SDRAM gap between zeroed ranges | 0x865E1D88 to 0x865FFFFF | 120 KB | Cached | Output ring |
| SDRAM after Qt's framebuffers | 0x87EA6004 to 0x87FFFFFF | about 1.4 MB | Uncached | The bat screen's display buffer, TE storage, task 1's patch stack, BAT PLAYBACK's read ring |

At B256 the patch uses 76.4 KB of the ITCM range and 149.1 KB of the DTCM range, and its SDRAM
region ends at 0x818FDA70.

## Display and controls

| Item | Details | Source |
|---|---|---|
| Screen | 3.5-inch colour touchscreen. | Tascam spec |
| Resolution and format | 320 × 480, portrait, RGB565, driven by the eLCDIF. Qt has two framebuffers at 0x87E00000 and 0x87E4B000. The bat screen draws at this size and format on the unit. | Firmware, Measured |
| Graphics | The stock UI is built with Qt for MCUs and draws through the PXP 2D engine. | Firmware |
| Buttons | HOME, REC, PLAY and MARK are one byte read over LPSPI4, not processor GPIO pins. POWER and HOLD are bits in another register on the same bus. Button codes: MARK 0x2b, REC 0x07, HOME 0x06, PLAY 0x05, POWER 0x11, HOLD 0x0d. | Firmware |
| Wheel | A signed 8-bit count in a register on the same bus. The polling task adds up the counts of 27 scans. | Firmware |
| Polling | One RTOS task polls the buttons, the wheel and the touch controller every 3 kernel ticks (6 ms; the kernel tick is 2 ms). The touch controller is read every second scan. | Firmware |
| Touch coordinates | Touches from the poll are in screen pixels; taps on the bat screen land where the finger is (B226). | Measured |

## Storage, USB and Bluetooth

| Item | Details | Source |
|---|---|---|
| Card | microSD (64 MB to 2 GB), microSDHC (4 to 32 GB) and microSDXC (48 to 512 GB), on the USDHC controller. FAT and exFAT. Takes go in `SOUND` and its `FOLDER_nnnn` folders; update files go in `UTILITY`. | Tascam spec, Firmware |
| File names | UTF-16 inside the firmware's file system calls. Only 2 folder handles exist in the whole system. | Firmware |
| USB | Type-C, USB 2.0 high speed. Mass storage (SD CARD READER), and an 8-in, 2-out USB audio interface up to 96 kHz. | Tascam spec |
| Bluetooth | Port for the optional AK-BT1 adapter (Bluetooth LE), used by the Portacapture Control app and for timecode. The firmware's "TCS" stack drives it. Not tested with the patch: the owner does not have one. | Tascam spec, Firmware |

## Power

| Item | Details | Source |
|---|---|---|
| Sources | 4 AA cells (alkaline, NiMH or lithium), USB bus power, or TASCAM's PS-P520U adapter. | Tascam spec |
| Consumption | 7.5 W maximum. | Tascam spec |
| Battery life | About 11 hours on alkaline cells with the included mics, 44.1 kHz, power save on; about 5.5 hours with phantom power on two inputs. Not measured at 192 kHz or with the bat screen open. | Tascam spec, Not confirmed (bat use) |
| Phantom power switching | Per input (inputs 3 to 6). FPGA register 6 has one bit per input; register 3 turns on the shared supply, and registers 3 and 10 select 24 V or 48 V. The firmware mutes all inputs while the supply comes up. | Firmware |

## Physical

77 × 40 × 205.6 mm (width × height × depth, without protrusions); 472 g with batteries, 381 g
without; operating range 0 to 40 °C (Tascam spec).

## Opening the unit and debug access

Two teardown videos are public: [What Dennis Does](https://www.youtube.com/watch?v=ryWlHvm3rk4)
(English, 2022) and [iFixThis](https://www.youtube.com/watch?v=YKcWbtdZgvI) (Polish, 2025). The
owner's unit has not been opened.

| Item | Details | Source |
|---|---|---|
| Opening | Batteries, card and mic capsules out; two screws under the rubber feet and six more free the back. The logic board has four screws and sits on a board-to-board connector. The LCD is under the logic board. | Teardown |
| Debug pads | The board has many labelled test points. Neither video identifies SWD, JTAG or UART pads, and no schematic is published. | Teardown |
| SWD pins | SWDIO is GPIO_AD_B0_06 and SWCLK is GPIO_AD_B0_07 on the RT1062. They would need tracing from vias or test points. Whether TASCAM disabled the debug port with fuses is not known. | Datasheet, Not confirmed |
| Debug console | The firmware contains the SDK's debug console code and a log format, so a debug UART probably exists. | Firmware, Not confirmed |
| USB serial download | The RT1062's boot ROM can load code over USB when its BOOT_MODE pins select serial download. Whether the X8's USB-C port is wired to the processor's USB OTG1, and how to set BOOT_MODE on this board, are not known. | Datasheet, Not confirmed |

More detail, and the difficulty of each recovery route: [Opening the unit and hardware
recovery](HOMEBREW.md#opening-the-unit-and-hardware-recovery) in HOMEBREW.md.

## Open questions

| Question | Why it matters | How to settle it |
|---|---|---|
| Which converter serves inputs 1 and 2 | Their response above 20 kHz may differ from inputs 3 to 6 | Open a unit and read the chip markings |
| Analog response from 60 to 96 kHz | Sets the real upper limit for bat calls | Line-level sine sweep into input 3, recorded at 192 kHz |
| SDRAM size, 64 or 128 MiB | Sets how much memory is free for longer buffers | Read the SDRAM chip markings |
| Whether the steady lines near 37 and 72 kHz are the reported hum | A steady tone in that range is heard through HET | Record at 192 kHz with inputs 3 and 4 terminated, with and without a phone nearby |
| The clock chip's part number, and whether its flag bit selects 12-hour mode | The patch sets the clock from phone chimes | Open a unit, or read the chip's register map once identified |
| A hardware recovery path | Needed if an update is interrupted | Find SWD or BOOT_MODE on an opened unit |
| Battery life at 192 kHz with the bat screen open | Field use | Time a full set of AA cells |
