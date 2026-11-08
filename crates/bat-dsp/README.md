# bat-dsp

Real-time bat detector DSP for microcontrollers, written for the TASCAM
Portacapture X8 (NXP i.MX RT1062, Cortex-M7 at 600 MHz, recording at 192 kHz).
`no_std`, `f32`, no allocation. Dependencies: `libm` and `microfft`.

## Modules

CPU figures are per channel at 192 kHz on a 600 MHz Cortex-M7. They are
estimates from the generated code and operation counts, not measurements.

| Module | Type | What it does | CPU (estimate) |
|---|---|---|---|
| `heterodyne` | `Heterodyne` | Mixes the input with a local oscillator and low-passes it, so a call at f is heard at \|f − LO\|. Default cutoff 15 kHz. | 1 to 2% |
| `heterodyne` | `HetComb<N>` | Up to N heterodynes mixed, LOs spaced across the band (default spacing 30 kHz). | 1 to 2% per LO |
| `freq_div` | `FrequencyDivider` | Zero-crossing frequency division: a click (or square-wave step) every `division` cycles of the ultrasonic signal, so a call at f is heard at f / division. Also reports zero-crossing frequency points for an Anabat-style plot. | about 3% |
| `time_expansion` | `TimeExpander` | Keeps a ring buffer in caller-provided storage; on a trigger, captures a window (default 50 ms before, 250 ms after) and plays it back slowed by a factor (default 10). | under 2% while playing |
| `pitch_shift` | `PitchShifter` | Phase vocoder that divides frequencies by a factor (default 10, range 2 to 32) at real-time speed. 1024-point frames, hop 256, latency 1024 samples. | 11 to 16% |
| `spectrum` | `SpectrumAnalyzer<N>` | Hann-windowed FFT columns in dB (N = 256, 512 or 1024), plus mapping a column to display pixels. | with the detector: 6 to 8% for N = 512, hop 128 |
| `detector` | `CallDetector<N>` | Finds calls in spectrum columns against a per-bin noise floor, so steady tones and insect noise are ignored. Reports start, end, peak, frequency range and the audible band's level during each call. | (included above) |
| `classify` | `CallClassifier` | Sorts detections into bat calls and other sounds: steady tones longer than a bat call (such as a rat's 22 kHz alarm call), broadband clicks, sounds below 16 kHz (settable), sounds with a loud audible part, and singles; a bat call is one in a train of similar calls. | negligible |
| `biquad`, `osc` | `Biquad`, `Butterworth4`, `Nco` | Building blocks: RBJ biquads (low-pass, high-pass, notch), 4th-order Butterworth, and a sine/cosine oscillator. | |

## Conventions

- `process(&mut self, input, output)` with equal lengths. The output is the same
  whichever way the input is split into blocks; every module has a test for this.
- The sample rate is fixed in the constructor. Settings that a user changes while
  listening have setters that do not click.
- Buffers whose size depends on time (the time expander's storage) are passed in
  by the caller, so they can live in a static or in SDRAM.
- Three structs are large: `PitchShifter` (about 17 KB), `SpectrumAnalyzer<1024>`
  (about 14 KB) and `CallDetector<1024>` (about 8 KB). `new()` builds them on the
  stack, which is fine on a PC. On the target, put `unconfigured()` (a `const fn`)
  in a `static` and call `init(sample_rate)` on it at start-up. `init` works in
  place: on the Cortex-M7 build, `PitchShifter::init` uses a 56-byte stack frame,
  against over 16 KB for `new`. Calling `init` again restores the defaults and
  clears the history, for example after a sample-rate change.
- The pitch shifter's window and twiddle tables (6 KB) are computed at compile
  time into statics, so they sit in flash and are shared by all instances.

## Build and test

```bash
cargo test -p bat-dsp
```

```bash
cargo clippy -p bat-dsp --all-targets
```

```bash
cargo build -p bat-dsp --release --target thumbv7em-none-eabihf
```

`.cargo/config.toml` at the repository root sets `target-cpu=cortex-m7`, so f64
maths uses the hardware FPU. Add the target once with
`rustup target add thumbv7em-none-eabihf`.

## Known limits

- **Pitch shift:** fast FM calls (2 to 7 ms sweeps) keep their level on average,
  but a single call can come out 6 dB quieter to 3 dB louder, and the sweep is
  smeared by about ±2 bins. This is probably a limit of 1024-point frames.
- **Time expansion:** no fade at the window edges, so a window that ends in the
  middle of a call can click.
- **Call detector:** a call is reported 2 ms (the merge gap) after it ends. A steady
  sound that switches on keeps `is_active()` true for up to 100 ms before it
  counts as background. Digital silence followed by sound gives one false event.
  A call's frequency range covers the bins within 20 dB of the call's peak
  (settable, in 3 dB steps), so a sweep whose ends fade out reports a narrower
  range than its full sweep.
- **Frequency division:** the noise floor starts high, so the divider is silent
  for about 0.1 s after construction or reset. A steady tone lasting several
  seconds eventually counts as noise.

## Tests

Each module has its own tests, and `pipeline_tests.rs` chains the spectrum
analyzer, the call detector and the time expander as the X8 app will: a 5 ms
sweep in noise is detected once, triggers the expander within a few
milliseconds, and plays back 10 times slower and lower.

## Next

- Measure real cycle counts on a Teensy 4.1 (same chip family) with the DWT cycle
  counter, and compare the outputs with Oversample's `oversample-core` on recorded
  X8 files.
- Fades for the time expander window.
- A C interface (`staticlib`) so TASCAM's firmware, or a patch, can call it.
