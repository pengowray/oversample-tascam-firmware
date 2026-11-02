# Portacapture X8 as a bat detector: feasibility

First assessment, 2 November 2026. Based on the V2.01 and V2.03 firmware update
files, the Tascam reference manual (Rev H), and the AK5704 datasheet
(019000890-E-01, 2022/01). No measurements on a unit yet. Figures marked
*estimate* are calculations, not measurements.

The firmware reverse-engineering notes and the patching plan are in
[HOMEBREW.md](HOMEBREW.md).

## Summary

- **The MVP is feasible.** Recording inputs 3/4 as a 192 kHz WAV while listening
  through heterodyne or pitch shift needs about 2 to 10% of the 600 MHz Cortex-M7
  (*estimate*). The FPGA is not needed for any bat feature.
- **The usable range is up to about 90 kHz, with some loss above 50 kHz.** At
  192 kHz the AK5704's built-in digital filter is down 1 dB at 51 kHz, 3 dB at
  65 kHz and 6 dB at 78 kHz. That covers most Australian microbats. Calls above
  96 kHz cannot be recorded, and calls between 96 and 145 kHz partly appear as
  false lower-frequency signals (aliases).
- **Rust suits the Cortex-M7 code.** The i.MX RT1060 has mature Rust support
  (`imxrt-ral`, `imxrt-hal`, the Teensy 4 crates, `probe-rs` for debugging). There
  is no practical Rust route for the FPGA, and the FPGA does not need changing.
- **A homebrew patched firmware is possible.** The update file is protected only
  by a 32-bit byte sum, and the boot image is unsigned. `tools/x8fw.py` unpacks and
  repacks both official releases byte for byte.
- **The main risk of patching is a bricked unit.** Firmware update mode is part of
  the main application, so a patch that stops the main application from starting
  also stops recovery from the SD card. A hardware recovery path (SWD debug pads or
  the chip's USB serial download mode) has to be found before any patched code is
  flashed.
- **Open questions that need a unit or Tascam's answers:** the ultrasonic hum that
  reviewers report in X8 recordings, the analog response from 60 to 96 kHz, and how
  much CPU the stock firmware leaves free at 192 kHz.

## Hardware

| Part | Details | Source |
|---|---|---|
| Processor | NXP i.MX RT1060 family: Cortex-M7, 600 MHz, single and double precision FPU | Tascam; boot header in the firmware |
| Program flash | 32 MiB FlexSPI NOR flash | boot data in the firmware image |
| RAM | On-chip RAM plus external SDRAM. SDRAM size unknown; at least 24 MB, because the 24.3 MB main application is copied into it and runs from it | firmware |
| FPGA | Lattice MachXO3 with on-chip configuration flash. The main application can reprogram it. | firmware strings `MachXO3CFMErase`, `Write PLD : 0x%08lX` |
| ADCs | AKM AK5704, 4 channels, up to 192 kHz. Inputs 1 to 6 each use two ADC channels at different gains to produce 32-bit float. | Tascam; manual |
| Inputs 1/2 | 3.5 mm TRS jacks with plug-in power. The included mics connect here. | manual |
| Inputs 3/4 and 5/6 | XLR/TRS combo jacks with phantom power | manual |
| EXT IN | 3.5 mm line input. One ADC channel only (no dual gain). Cannot be used at the same time as inputs 5/6. | manual |
| Display | 3.5-inch colour touchscreen on the NXP eLCDIF display controller, with the PXP 2D graphics engine. Resolution not yet confirmed. | manual; firmware |
| Bluetooth | Optional AK-BT1 adapter (Bluetooth LE), used by the Portacapture Control app | manual |

Plug-in power is available on inputs 1/2 only. Inputs 3 to 6 provide phantom power.
So an ultrasonic mic on inputs 3/4 needs to be phantom powered (or self powered);
a plug-in-power electret or MEMS capsule goes on inputs 1/2.

**The FPGA is a small glue-logic part, not a DSP engine.** The MachXO3 family has
no hardware multipliers. The firmware's FPGA programming routine uses a page count
of 2,164 pages of 16 bytes (about 35 KB), which fits one of the smaller MachXO3
parts (around 1,300 logic cells). The chip marking will confirm it. The FPGA's job
is probably clock generation and routing the multi-channel (TDM) audio between the
ADCs and the processor, and possibly combining the two gain paths into float. All
DSP in this plan runs on the Cortex-M7.

## Frequency range at 192 kHz

The AK5704 at fs = 192 kHz (datasheet section 8.5, "Short Delay Sharp Roll-off
Filter"):

| Frequency | Level relative to 1 kHz |
|---|---|
| up to 37 kHz | within ±0.12 dB |
| 51 kHz | −1 dB |
| 65 kHz | −3 dB |
| 78 kHz | −6 dB |
| 96 kHz (Nyquist) | not given in the datasheet; about −10 dB (*estimate*) |
| 145.5 kHz and above | −72 dB or lower (stopband) |

Tascam's own specification for 192 kHz recording is 20 Hz to 60 kHz, +0/−2.5 dB,
which agrees with the datasheet.

What this means for bat calls:

- **20 to 50 kHz:** recorded flat.
- **50 to 90 kHz:** recorded, but 1 to 10 dB quieter. The loss comes from a known,
  fixed digital filter, so the listening and display path can boost it back. The
  boost raises the noise at those frequencies by the same amount. Recordings stay
  unprocessed.
- **96 to 145 kHz:** not recorded correctly. The filter only partly removes these
  frequencies, so a call at f kHz appears in the recording at (192 − f) kHz. For
  example, a call at 120 kHz shows up near 72 kHz. The alias is weaker than the
  real call, and weaker the higher the real frequency is. Some leaf-nosed bats
  (Hipposideros, Rhinonicteris) call above 100 kHz, so their calls could show up as
  false calls between 50 and 90 kHz. Species identification needs to allow for this.
- **The analog circuit may roll off as well.** The mic preamp and any filtering on
  the board are not documented above 60 kHz. This needs a measurement.

**Above 192 kHz.** The AK5704 datasheet lists 192 kHz as the maximum sample rate.
The datasheet says the filter frequencies scale with the sample rate (note 21), so
running the clocks at 384 kHz would in theory move the −3 dB point to about
130 kHz. But this is outside the specification, and the ADC's PLL, the FPGA's
clocking, the headphone DAC and the firmware's clock setup may not support it.
Treat it as an experiment on a unit with a working recovery path, not as part of
the plan.

**Reported ultrasonic hum.** Reviewers report a steady ultrasonic tone in X8
recordings, with the included mics and with phantom-powered mics, in 24-bit and
32-bit modes. No frequency or level has been published. If the tone falls between
20 and 96 kHz, it will be heard through the heterodyne and could trigger the call
detector. A steady tone can be removed with a notch filter in the bat path, but its
frequency and level need to be measured first (see the first milestone below).

## Features

| Feature | Feasible? | Notes |
|---|---|---|
| Record inputs 3/4 as 192 kHz, 32-bit float stereo WAV | Yes, stock | Existing apps already do this. |
| Live heterodyne | Yes | About 1 to 2% CPU per channel (*estimate*). |
| Heterodyne comb (several oscillators mixed together) | Yes | About 5 to 8% CPU for 4 oscillators (*estimate*). |
| Frequency division (zero-crossing, "old-school") | Yes | Under 1% CPU. |
| Pitch shift (phase vocoder) | Yes | About 6% CPU for one channel (*estimate*). |
| Time expansion of detected calls, for listening | Yes | Under 1% CPU. Recording to the SD card continues while the expanded call plays. |
| Look-back buffer of 5 to 10 s | Partly stock | The FIELD app's EXPANDED PRE REC keeps about 8 s before REC is pressed. Not yet checked at 192 kHz. |
| Processed audio recorded as an extra track | Probably | Multi-track apps record a master (mix) file alongside the track files. If the bat audio replaces the mix, the stock recorder may save it with no extra work. Needs checking. |
| Mono notes track from input 1 | Yes, as WAV | At 192 kHz the stock firmware does not record MP3. An MP3 notes file would need the notes channel resampled to 48 kHz first. |
| Spectrogram waterfall | Yes | Computing it is under 1% CPU. Drawing it is the real work, because the stock UI is built with Qt for MCUs. |
| Call snapshots (detect a call, show it zoomed for a few seconds) | Yes | Detection is under 1% CPU. |
| Zero-crossing frequency-time plot (Anabat style) | Yes | Cheaper to draw than a waterfall. |
| Unattended bat recorder | Later | The stock AUTO REC starts recording when the level crosses a threshold. Scheduling is missing. Battery life on 4 AA cells is short, so overnight use needs USB power. |
| Settings and metadata sent from a phone as audio pulses | Yes, later | Decoding the pulses is simple DSP. The work is in applying the settings inside the firmware. |
| Settings over Bluetooth | Later | Needs the AK-BT1 and an extension to its protocol, which needs Tascam or reverse engineering. |
| GUANO metadata in WAV files | Later | Oversample already reads and writes GUANO. Writing it on the X8 means changing the WAV writer or adding the chunk when a file is closed. |

### CPU estimates

At 192 kHz, per channel, on a 600 MHz Cortex-M7 (600 million cycles per second).
All figures are *estimates*. They assume the DSP code and buffers sit in the
processor's tightly coupled memory (ITCM and DTCM). Code running from SDRAM can be
several times slower when it misses the cache.

| Mode | Work | CPU share |
|---|---|---|
| Heterodyne | complex mix and 4th-order low-pass, about 40 cycles per sample | 1 to 2% |
| Heterodyne comb, 4 oscillators | 4 × the above | 5 to 8% |
| Frequency division (zero-crossing ÷ 10) | about 20 cycles per sample | under 1% |
| Pitch shift, phase vocoder | 1,024-point FFT, hop 256: 750 frames per second, about 50,000 cycles per frame | about 6% |
| Time expansion | copy and interpolate at the output rate | under 1% |
| Spectrogram for the waterfall | 512-point FFT, 100 columns per second | under 1%, plus drawing |
| Call detection | energy in a frequency band, per spectrogram column | under 1% |

The cycle counts can be measured before touching the X8: a Teensy 4.1 uses the
i.MX RT1062, the same chip family at the same clock speed.

**How much CPU is free in the stock firmware is unknown.** At 192 kHz the manual
disables EQ. At 96 and 192 kHz it disables reverb and MP3 dual recording, and VSA
and KEY playback do not work with 96 or 192 kHz files. USB audio does not support
192 kHz. These may be CPU limits or product decisions. A bat mode would record 2 or
3 tracks instead of up to 8, which frees processing time.

### Memory for the look-back buffer

| Buffer | Size |
|---|---|
| 10 s, stereo, 192 kHz, 32-bit float | 15.4 MB |
| 10 s, stereo, 192 kHz, 24-bit | 11.5 MB |
| 5 s, stereo, 192 kHz, 32-bit float | 7.7 MB |

The SDRAM size is unknown and the main application already uses about 24 MB of it,
so a 10 s buffer may not fit in RAM. The stock 8 s EXPANDED PRE REC may keep its
buffer on the SD card. Testing it at 192 kHz is the first step.

## Ways to build it

| Route | What it means | Effort | Risk |
|---|---|---|---|
| **A. Built in with Tascam** | Tascam adds a bat app to their firmware and links our DSP library into it. The library is Rust compiled as a static library with a C interface (`staticlib`, target `thumbv7em-none-eabihf`). Tascam integrates the UI in Qt for MCUs; we provide the DSP and the bat-specific screen logic. | Low for us; needs Tascam engineering time | Low |
| **B. Homebrew patch** | We patch the official firmware binary: add our code to the image and redirect a few calls in the audio path and the UI. See [HOMEBREW.md](HOMEBREW.md). | Weeks of reverse engineering before the first sound | Bricking without a recovery path |
| **C. Replacement firmware in Rust** | We rewrite all of it: display, touch, SD card and exFAT, USB, battery, ADC and FPGA setup, UI. | Months | High |

**Recommendation:** aim for route A for anything released to users. Use route B
for a working prototype, ideally with Tascam's symbols and a debug unit (see the
next section), which would shorten it from weeks to days. Route C only makes sense
later, for a dedicated unattended recorder.

The DSP library is the same code in routes A and B, so it can be written and tested
on a desktop now. Oversample's `oversample-core` already has heterodyne, pitch
shift, phase vocoder, zero-crossing, frequency division and notch code, but it uses
the standard library (`Vec`, `f64`, `realfft`). The streaming parts (such as
`RealtimeHet`) need porting to a `no_std`, `f32`, fixed-buffer crate. That crate
could also serve Oversample's own live microphone mode.

## What to ask Tascam for

In order of value:

1. **The linker map or ELF symbol file for the current firmware build.** This turns
   most of the reverse engineering into reading function names.
2. **An audio hook.** A documented callback in the path from the inputs to the
   headphone output, at the recording sample rate, with the block size and sample
   format. Also: whether the stereo mix (master file) can carry the processed audio.
3. **A UI slot.** A screen in their Qt for MCUs app, or permission to draw directly
   into a region of the framebuffer while bat mode is active, plus button, wheel and
   touch events.
4. **A debug unit and a recovery procedure.** SWD and debug UART pads brought out,
   and how to force the i.MX RT ROM into USB serial download mode (the BOOT_MODE
   pins).
5. **Hardware details.** The analog input path for inputs 1 to 4 (preamp bandwidth,
   anti-alias filtering), which ADC channels feed each input, what the FPGA does and
   how it is controlled, the headphone DAC part and its clocking, the SDRAM size, and
   free flash and RAM.
6. **The ultrasonic hum.** Its cause and frequency, and whether a hardware revision
   fixed it.
7. **Higher sample rates.** Whether the clock tree, FPGA and DAC can run at 384 kHz
   for an experiment.
8. **Build integration.** Their compiler (IAR, GCC or Arm Compiler) and whether they
   would link a Rust static library. Any toolchain that follows the Arm hard-float
   calling convention can link it.

## Unknowns and how to settle them

| Question | Why it matters | How to find out |
|---|---|---|
| Frequency and level of the ultrasonic hum | Could mask calls or trigger the detector | Record at 192 kHz with inputs 3/4 terminated, on battery and on USB power; view in Oversample |
| Analog response from 60 to 96 kHz | Sets the real upper limit | Feed a sine sweep at line level into input 3, record at 192 kHz, compare with the datasheet filter |
| Free CPU at 192 kHz in the stock firmware | Sets how much DSP fits | Ask Tascam, or measure the RTOS idle time once the firmware is mapped |
| SDRAM size and free RAM | Sets the look-back buffer length | Ask Tascam, or read the SDRAM setup in the bootloader |
| Whether EXPANDED PRE REC works at 192 kHz | May provide the look-back buffer for free | Try it on a unit |
| Where the two gain paths are combined (FPGA or processor) | Decides where the bat code reads samples | Firmware analysis, or ask Tascam |
| Display resolution and framebuffer format | Needed for the waterfall | Firmware analysis, or ask Tascam |
| A recovery path for a bricked unit | Required before any patched code is flashed | Open a unit and look for SWD pads and BOOT_MODE resistors; or ask Tascam |

## Milestones

1. **Bench measurements with stock firmware.** Hum, frequency response to 96 kHz,
   noise floor with ultrasonic mics on inputs 3/4, and EXPANDED PRE REC at 192 kHz.
   Analyse the recordings in Oversample.
2. **DSP library.** A `no_std` Rust crate with block-based heterodyne, heterodyne
   comb, frequency division, phase vocoder pitch shift, time expansion, spectrogram
   columns and a call detector. Test it on a desktop against `oversample-core` and
   against X8 recordings, and measure cycle counts on a Teensy 4.1.
3. **Integration on the X8.** Route A with Tascam, or route B.
4. **Display.** Waterfall, zero-crossing plot and call snapshots.
5. **Later.** Unattended recording, GUANO metadata, phone control by audio pulses or
   Bluetooth, experiments above 192 kHz.
