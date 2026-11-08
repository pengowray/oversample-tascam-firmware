# Portacapture X8 as a bat detector: feasibility

This is the original feasibility study, kept as written on 2 November 2026. Some
of its open questions have since been answered, and some of its estimates have been
replaced by measurements on a unit. For current results, see
[HOMEBREW.md](HOMEBREW.md). Since this study was written:

- **Live listening runs on the X8.** HET, COMB, FD, TE and 1:1 run on the owner's
  unit while it records, and the CPU test measured each of them there (Step 21 in HOMEBREW.md).
  PITCH was too slow on the X8 and is hidden there. The CPU test measures it again from B251, in stereo too
  (Steps 27 and 28 in HOMEBREW.md).
- **Recording works from the bat screen.** Takes are recorded by the stock recorder
  while the bat screen is open.
- **Two launcher tiles open the bat app.** BAT DETECTOR and BAT PLAYBACK replace the
  USER SETTINGS 2 and 3 tiles (installed from B231).
- **The phone remote is installed, but not yet tested end to end.** The phone
  page, a separate project that is not yet published, plays chimes that start and
  stop a take (B239; the command format was replaced in B249, see
  [PHONE_REMOTE.md](PHONE_REMOTE.md)). The CPU cost of the X8's chime decoders was measured on the
  unit, but no take has yet been reported as started or stopped by phone. The
  AK-BT1 Bluetooth adapter has not been tested.
- **GUANO metadata and setting the clock from phone chimes are built, not yet
  installed.** B248 and later add GUANO metadata to the WAV files of bat takes, and sets
  the X8's clock from the time in a phone chime.

This project is independent, and it is not affiliated with or endorsed by TASCAM.
Route A (building the bat app into the firmware with Tascam) and the questions for
Tascam in this study are options that the study considered.

First assessment, 2 November 2026. Based on the V2.01 and V2.03 firmware update
files, the Tascam reference manual (Rev H), and the AK5704 datasheet
(019000890-E-01, 2022/01). No measurements on a unit yet. Figures marked
*estimate* are calculations, not measurements. Terms are explained at the end.

The firmware reverse-engineering notes and the patching plan are in
[HOMEBREW.md](HOMEBREW.md).

## Summary

- **The MVP is feasible.** The MVP is: record inputs 3/4 as a 192 kHz WAV while
  listening on headphones through heterodyne or pitch shift. On the 600 MHz
  Cortex-M7, heterodyne needs about 1 to 2% per channel and phase-vocoder pitch
  shift about 11 to 16% per channel (*estimates*; see `crates/bat-dsp`). The FPGA
  is not needed for any bat feature.
- **The usable range is up to about 90 kHz, with some loss above 50 kHz.** At
  192 kHz the AK5704's built-in digital filter is 1 dB below its 1 kHz level at
  51 kHz, 3 dB below at 65 kHz and 6 dB below at 78 kHz. That covers most
  Australian microbats. Calls above 96 kHz cannot be recorded, and calls between
  96 and 145 kHz partly appear as false lower-frequency signals (aliases).
- **Rust suits the Cortex-M7 code.** The i.MX RT1060 family has mature Rust support
  (`imxrt-ral`, `imxrt-hal`, the Teensy 4 crates, `probe-rs` for debugging). There
  is no practical Rust route for the FPGA, and the FPGA does not need changing.
- **A homebrew patched firmware is possible.** The updater appears to check only a
  product code, the version number and a 32-bit byte sum, and the firmware has no
  boot signature. This is based on the updater's error messages; the update code
  itself has not been read yet. `tools/x8fw.py` unpacks both official releases and
  repacks them byte for byte.
- **The bricking risk can be kept close to that of an official update.** The
  firmware chooses update mode (HOME and REC held at power-on) early in start-up,
  before the RTOS, the screens or audio start. So a bug in patched audio or screen
  code can be fixed by installing an official update file from the SD card. A unit
  is only bricked for good if an update is interrupted (true of official updates
  too), or if a patch changes the start-up or update code. A hardware recovery path
  (SWD debug pads or the processor's USB serial download mode) is the backstop for
  those cases; finding one needs the unit opened. Details are in
  [HOMEBREW.md](HOMEBREW.md#keeping-the-bricking-risk-low).
- **Open questions that need a unit or Tascam's answers:** the analog response
  from 60 to 96 kHz, how much CPU the stock firmware leaves free at 192 kHz, and
  whether the reported ultrasonic hum (from radio interference) shows up on a
  given unit.

## Hardware

| Part | Details | Source |
|---|---|---|
| Processor | NXP i.MX RT1062 (MIMXRT1062DVL6B): Cortex-M7, 600 MHz, single and double precision FPU, 1 MB on-chip RAM | teardown video; firmware |
| Program flash | 32 MiB FlexSPI NOR flash, one chip | firmware; teardown video |
| RAM | On-chip RAM (ITCM, DTCM, OCRAM) plus external SDRAM on two chips. At least about 60 MB of SDRAM is in use (the updater loads the update file at 0x822F3AD8), so probably 64 MB or 128 MB in total. The main application (25.4 MB) runs from SDRAM. | firmware; teardown video |
| FPGA | Lattice MachXO2 (LCMXO2-1200ZE-1MG132C): 1,280 LUTs, no hardware multipliers, on-chip configuration flash. The firmware programs it with Lattice's MachXO3 routines, which use the same commands. | teardown video; firmware |
| ADCs | AKM AK5704: 4 channels per chip, up to 192 kHz, with built-in mic preamps. There are two of them, serving inputs 3 to 6 according to Tascam: each dual-gain input uses two ADC channels at different gains to produce 32-bit float. Which ADC serves inputs 1/2 and EXT IN is not confirmed. | Tascam; teardown video |
| Analog and output | OPA1678 op-amps in the input stages, TI PCM1754 DAC, TI TPA4411 headphone amplifier | teardown video |
| Inputs 1/2 | 3.5 mm TRS jacks with plug-in power. The included mics connect here. | manual |
| Inputs 3/4 and 5/6 | XLR/TRS combo jacks with phantom power | manual |
| EXT IN | Stereo 3.5 mm line input. One ADC channel per side (no dual gain). Cannot be used at the same time as inputs 5/6. | manual |
| Display | 3.5-inch colour touchscreen on the NXP eLCDIF display controller, with the PXP 2D graphics engine. Resolution not yet confirmed. | manual; firmware |
| Bluetooth | Optional AK-BT1 adapter (Bluetooth LE), used by the Portacapture Control app | manual |

Plug-in power is available on inputs 1/2 only. Inputs 3 to 6 provide phantom power.
The MVP uses inputs 3/4, so the ultrasonic mics need phantom power or their own
power. Plug-in-power electret or MEMS capsules are an alternative on inputs 1/2.

**The FPGA is a small glue-logic part, not a DSP engine.** The MachXO2-1200 has
1,280 LUTs, 64 kbit of block RAM, one PLL and no hardware multipliers. The
firmware's FPGA programming routine writes 2,164 configuration pages, which matches
this part. In recorders like this, an FPGA of this size usually generates and
distributes the audio clocks, joins the serial audio from several converter chips
into one multi-channel (TDM) stream for the processor, and handles extra I/O. Here
it probably also combines each input's two gain paths into one float value. All DSP
in this plan runs on the Cortex-M7, which is far faster at it.

The bat app uses the FPGA exactly as the stock firmware sets it up: the same clocks,
the same 192 kHz sample rate, and the same samples arriving at the processor. The
FPGA's design is stored in the FPGA's own on-chip flash, programmed at the factory.
It is probably not in the published update file; the firmware only contains the
code that can reprogram it from a separate file.

## Frequency range at 192 kHz

The AK5704 at a sample rate of 192 kHz (datasheet section 8.5, "Short Delay Sharp
Roll-off Filter"):

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

This applies to inputs 3 to 6, which use the AK5704. Inputs 1/2 may use a
different ADC, so their response above 20 kHz may differ. This is one more reason
to use inputs 3/4 for the MVP.

What this means for bat calls:

- **20 to 50 kHz:** recorded within 1 dB of flat.
- **50 to 90 kHz:** recorded, but 1 to 10 dB quieter. The loss comes from a known,
  fixed digital filter, so the bat app can boost these frequencies back up for the
  headphones and the display. The boost raises the noise at those frequencies by the
  same amount. Recordings stay unprocessed.
- **96 to 145 kHz:** not recorded correctly. The filter only partly removes these
  frequencies, so a call at f kHz appears in the recording at (192 − f) kHz. For
  example, a call at 120 kHz shows up near 72 kHz. The false call is quieter than
  the real one. The higher the real call's frequency, the quieter the false call.
  Some leaf-nosed bats (Hipposideros, Rhinonicteris) call above 100 kHz, so their
  calls could show up as false calls between 50 and 90 kHz. Species identification
  needs to allow for this.
- **The analog circuit may roll off as well.** The mic preamp and any filtering on
  the board are not documented above 60 kHz. This needs a measurement.

**Above 192 kHz.** The AK5704 datasheet lists 192 kHz as the maximum sample rate.
The datasheet says the filter frequencies scale with the sample rate (note 21), so
running the clocks at 384 kHz would in theory move the −3 dB point to about
130 kHz. But 384 kHz is outside the specification, and the ADC's PLL, the FPGA's
clocking, the headphone DAC and the firmware's clock setup may not support it.
Treat 384 kHz as an experiment for a unit with a working recovery path.

**Reported ultrasonic hum: reduced, not gone.** A 2022 review and a user review
reported a steady ultrasonic hum in X8 recordings. Owner discussions (for example
on Airwiggles, not yet checked first-hand) say it comes from radio-frequency
interference picked up from nearby phones and Wi-Fi, that early production units
show it most, and that firmware updates reduced it without removing it. This
project's own X8 has not shown it. For bat work:

- Check each unit once: record at 192 kHz with inputs 3/4 terminated, with and
  without a phone next to it, and look for steady lines in Oversample.
- Keep phones away from the recorder, or in flight mode, during surveys.
- The call detector ignores steady tones, and a notch filter can remove a tone
  from the monitor path.

## Features

In this table, "monitor path" means the audio the X8 sends to the headphones.

| Feature | Feasible? | Notes |
|---|---|---|
| Record inputs 3/4 as 192 kHz, 32-bit float stereo WAV | Yes, stock | The X8's MANUAL app already records this. |
| Live heterodyne | Yes | About 1 to 2% CPU per channel (*estimate*). |
| Heterodyne comb: several heterodyne oscillators at once, mixed together, so calls at different frequencies are all audible | Yes | About 4 to 8% CPU per channel for 4 oscillators (*estimate*). |
| Frequency division (zero-crossing, the older detector method) | Yes | Under 1% CPU. |
| Pitch shift (phase vocoder) | Yes | About 11 to 16% CPU per channel (*estimate*). Listening in mono halves it. |
| Time expansion of detected calls, for listening | Yes | Under 1% CPU. Recording to the SD card continues while the expanded call plays. |
| Playback of a recorded 192 kHz file through heterodyne, pitch shift, frequency division or time expansion | Yes, same hook as live listening | The stock VSA and KEY playback do not work with 96 or 192 kHz files. The bat app would process the playback audio in the monitor path, the same way as live input. |
| Look-back buffer of 5 to 10 s | Partly stock | The FIELD app's EXPANDED PRE REC keeps about 8 s before REC is pressed. Not yet checked at 192 kHz. |
| Processed audio recorded as an extra track | Probably | The X8's multi-track apps record a master file (the stereo mix) alongside the track files. If the bat audio replaces the mix, the stock recorder may save it as the master file with no extra work. Needs checking. |
| Mono notes track from input 1 | Yes, as WAV | At 96 and 192 kHz the stock firmware does not record MP3. An MP3 notes file would need the notes channel resampled to 48 kHz first. |
| Spectrogram waterfall | Yes | Computing it is under 1% CPU. Drawing it is the real work, because the stock UI is built with Qt for MCUs. |
| Call snapshots: detect a call, then show it zoomed for a few seconds | Yes | Detection, including its spectrum, is about 6 to 8% CPU (*estimate*). The waterfall can reuse the detector's spectrum columns. |
| Zero-crossing frequency-time plot (Anabat style) | Yes | Cheaper to draw than a waterfall. |
| Unattended bat recorder | Later | The stock AUTO REC setting starts recording when the input level crosses a threshold. A recording schedule is missing. Battery life on 4 AA cells is short, so overnight use needs USB power. |
| Settings and metadata sent from a phone app as audio pulses, picked up by the X8's mics | Yes, later | Decoding the pulses is simple DSP. The work is in applying the settings inside the firmware. |
| Settings over Bluetooth | Later | Needs the AK-BT1 and an extension to its control protocol, which needs Tascam or reverse engineering. |
| GUANO metadata in WAV files | Later | Oversample already reads and writes GUANO. Writing it on the X8 means changing the WAV writer or adding the metadata when a file is closed. |

### CPU estimates

At 192 kHz, per channel, on a 600 MHz Cortex-M7 (600 million cycles per second).
All figures are *estimates*. They assume the DSP code and buffers are in the
processor's tightly coupled memory (ITCM and DTCM). Code running from SDRAM can be
several times slower when it misses the cache.

| Mode | Work | CPU share per channel |
|---|---|---|
| Heterodyne | complex mix and 4th-order low-pass filter, about 40 cycles per sample | 1 to 2% |
| Heterodyne comb, 4 oscillators | 4 × the heterodyne | 4 to 8% |
| Frequency division (zero-crossing ÷ 10) | about 20 cycles per sample | under 1% |
| Pitch shift, phase vocoder | 1,024-point FFT, 256 samples between frames: 750 frames per second, about 90,000 to 130,000 cycles per frame | 11 to 16% |
| Time expansion | copy and interpolate at the output rate | under 1% |
| Spectrogram for the waterfall | 512-point FFT, 100 columns per second | under 1%, plus drawing |
| Call detection | 512-point FFT every 128 samples (1,500 columns per second) and a per-bin noise floor | 6 to 8% |

The cycle counts can be measured before touching the X8: a Teensy 4.1 uses the
i.MX RT1062, from the same chip family, at the same clock speed.

**How much CPU is free in the stock firmware is unknown.** According to the
manual, EQ is unavailable at 192 kHz. Reverb and MP3 dual recording are unavailable
at 96 and 192 kHz, VSA and KEY playback do not work with 96 or 192 kHz files, and
USB audio does not support 192 kHz. These may be CPU limits or product decisions.
The bat app would record 2 or 3 tracks instead of up to 8 (6 input tracks plus the
stereo master file), which frees processing time.

### Memory for the look-back buffer

| Buffer | Size |
|---|---|
| 10 s, stereo, 192 kHz, 32-bit float | 15.4 MB |
| 10 s, stereo, 192 kHz, 24-bit | 11.5 MB |
| 5 s, stereo, 192 kHz, 32-bit float | 7.7 MB |

The SDRAM is probably 64 MB or 128 MB, and the main application takes about 25 MB
of it, so a 10 s float buffer (15.4 MB) should fit in RAM if the stock firmware
leaves that much free. How much it already uses is not known. Testing the stock EXPANDED PRE REC at 192 kHz
is still worth doing first, because it may already provide the buffer.

## Ways to build it

| Route | What it means | Effort | Risk |
|---|---|---|---|
| **A. Built in with Tascam** | Tascam adds a bat app to their firmware and links our DSP library into it. The library is Rust compiled as a static library with a C interface (`staticlib`, target `thumbv7em-none-eabihf`). Tascam integrates the screens in Qt for MCUs; we provide the DSP and the bat-specific screen logic. | Low for us; needs Tascam engineering time | Low |
| **B. Homebrew patch** | We patch the official firmware: add our code to the firmware image and redirect a few function calls in the audio and display code. See [HOMEBREW.md](HOMEBREW.md). | Weeks of reverse engineering before the first sound | Bricking without a recovery path |
| **C. Replacement firmware in Rust** | We rewrite all of it: display, touch, SD card and exFAT, USB, battery, ADC and FPGA setup, screens. | Months | High |

**Recommendation:** aim for route A for anything released to users. Use route B
for a working prototype. Tascam's symbol file and a debug unit (see the next
section) would shorten route B from weeks to days. Route C only makes sense later,
for a dedicated unattended recorder.

The DSP library is the same code in routes A and B, so it can be written and tested
on a desktop now. Oversample's DSP library (`oversample-core`) already has
heterodyne, heterodyne comb, pitch shift, phase vocoder, zero-crossing, frequency
division and notch filter code. It uses the Rust standard library, 64-bit floats and
growable buffers, which the firmware cannot use. The real-time parts (for example
the live heterodyne) need porting to a `no_std`, 32-bit float, fixed-buffer
library. That library could also serve Oversample's own live microphone mode.

## What we would ask Tascam

These are the questions we would ask Tascam, in order of value:

1. **The linker map or ELF symbol file for the current firmware build.** With
   symbols, most functions already have names, so far less reverse engineering is
   needed.
2. **An audio hook.** A documented callback in the monitor path, at the recording
   sample rate, with the block size and sample format. Also: whether the master
   file can carry the processed audio.
3. **A screen for the bat app.** A screen in their Qt for MCUs UI, or permission to
   draw directly into a region of the framebuffer while the bat app is open, plus
   button, wheel and touch events.
4. **A debug unit and a recovery procedure.** SWD and debug UART pads brought out,
   and how to put the processor into USB serial download mode (the BOOT_MODE pins).
5. **Hardware details.** The analog input path for inputs 1 to 4 (preamp bandwidth,
   anti-alias filtering), which ADC channels feed each input, what the FPGA does and
   how the processor controls it, how the headphone DAC is clocked, and the free
   flash and RAM.
6. **The ultrasonic hum.** Its frequency, whether it is radio interference as owners
   report, and which production batches or firmware versions reduced it.
7. **Higher sample rates.** Whether the clocks, FPGA and DAC can run at 384 kHz for
   an experiment.
8. **Build integration.** Whether they would link a Rust static library into their
   IAR Embedded Workbench build (the firmware shows IAR's start-up code). Any toolchain that follows the Arm hard-float
   calling convention can link it.

## Unknowns and how to settle them

| Question | Why it matters | How to find out |
|---|---|---|
| Whether a unit shows the ultrasonic hum | Would be audible through the heterodyne | Record at 192 kHz with inputs 3/4 terminated, with and without a phone nearby, on battery and on USB power; view in Oversample |
| Analog response from 60 to 96 kHz | Sets the real upper limit | Feed a sine sweep at line level into input 3, record at 192 kHz, compare with the datasheet filter |
| Free CPU at 192 kHz in the stock firmware | Sets how much DSP fits | Ask Tascam, or measure the RTOS idle time once the firmware is mapped |
| SDRAM size (64 or 128 MB) and how much the stock firmware leaves free | Sets the look-back buffer length | Read the SDRAM chip markings, ask Tascam, or find the heap setup once the firmware is mapped |
| Whether EXPANDED PRE REC works at 192 kHz | May provide the look-back buffer at no cost | Try it on a unit |
| Where the two gain paths are combined (FPGA or processor) | Decides where the bat code reads samples | Firmware analysis, or ask Tascam |
| Display resolution and framebuffer format | Needed for the waterfall | Firmware analysis, or ask Tascam |
| A hardware recovery path | Needed if an update is interrupted; a backstop for patch testing | Open a unit and look for SWD pads and BOOT_MODE resistors; or ask Tascam |
| Whether SD card recovery works after a patched update | Makes code patches low risk | Half done: a string-only patch installed and ran on 2026-11-02. Next, install the official file again (the updater compares only the version, so it should be accepted) |

## Milestones

1. **Bench measurements with the stock firmware.** Any remaining hum, frequency
   response to 96 kHz, noise floor with ultrasonic mics on inputs 3/4, and EXPANDED PRE REC at
   192 kHz. Analyse the recordings in Oversample.
2. **DSP library (started: `crates/bat-dsp`).** A `no_std` Rust library with block-based heterodyne,
   heterodyne comb, frequency division, phase vocoder pitch shift, time expansion,
   spectrogram columns and a call detector. Test it on a desktop against
   `oversample-core` and against X8 recordings, and measure cycle counts on a
   Teensy 4.1.
3. **Integration on the X8.** Route A with Tascam, or route B.
4. **Display (started: `crates/bat-app`, with a web emulator in `web/`).**
   Waterfall and call snapshots are done; the zero-crossing plot is not.
5. **Later.** Unattended recording, GUANO metadata, phone control by audio pulses or
   Bluetooth, experiments above 192 kHz.

## Terms

| Term | Meaning |
|---|---|
| MVP | Minimum viable product: the smallest version worth using |
| Stock firmware | Tascam's official firmware, unmodified |
| Homebrew | Firmware changes made by users, not by the manufacturer |
| Oversample | Bat call analysis app (browser, desktop, Android) whose DSP code this project reuses |
| Heterodyne | Mixing the input with a fixed tone so that calls near that tone's frequency become audible |
| Aliasing | Frequencies above half the sample rate appearing as false lower frequencies |
| GUANO | A metadata standard for bat recordings, stored inside WAV files |
| Anabat | A bat detector brand known for zero-crossing frequency-time plots |
| VSA, KEY | The X8's variable-speed and pitch-change playback functions |
| TDM | Time-division multiplexing: several audio channels sent over one serial line |
| ITCM, DTCM | Tightly coupled memory: small, fast RAM inside the Cortex-M7 for code (ITCM) and data (DTCM) |
| SWD | Serial Wire Debug, the Arm debug port used to read and write flash with a probe |
| BOOT_MODE pins | Processor pins that select whether it boots from flash or waits for a download over USB |
| `no_std` | Rust code that runs without an operating system or the standard library |
| `staticlib` | A Rust library compiled so that C or C++ firmware can link it |
| Symbol file (ELF, linker map) | A build output that lists each function's name and address |
