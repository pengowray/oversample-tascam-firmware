//! Pitch shift: bat calls moved down by a factor into the audible range,
//! keeping their duration, so they are heard in real time.
//!
//! The method is a phase vocoder with peak shifting (Laroche and Dolson, "New
//! phase-vocoder techniques for real-time pitch shifting, chorusing,
//! harmonizing and other exotic audio modifications", JAES 47(11), 1999):
//!
//! 1. Every 288 samples (1.5 ms), the last 1024 are windowed (Hann) and
//!    transformed.
//! 2. Each local maximum of the magnitude spectrum is a peak. The bins within 4
//!    of a peak (and nearer to it than to the next peak) move together, by the
//!    whole number of bins that puts the peak nearest to its frequency divided
//!    by the factor. Moving them together keeps the shape of the peak, so a
//!    tone keeps its level.
//! 3. The other bins carry broadband sound, such as a fast FM call. They move
//!    in fixed blocks of 5 bins, each block by the shift of its centre.
//! 4. Moving a bin by `d` bins changes its frequency within the frame but not
//!    its phase at the centre of the frame. To keep the output continuous from
//!    one frame to the next, the moved bins are also rotated by `d × 288 / 1024`
//!    turns more each hop. For peaks this `d` is not rounded, so a tone comes
//!    out at exactly its frequency divided by the factor. Each peak or block
//!    continues the rotation stored at its bin in the previous frame.
//! 5. The shifted spectrum has nothing above 24 kHz, because the factor is at
//!    least 4. Its bins below 24 kHz are inverse transformed at a quarter of
//!    the sample rate (256 points), windowed again and overlap-added there. The
//!    result is interpolated linearly back to the full rate. Each bin is first
//!    boosted to undo the level the interpolation loses at high frequencies
//!    (1.3 dB at 10 kHz).
//!
//! The output phase of each bin is its input phase plus a rotation, so noise
//! keeps its random phase and still sounds like noise. A classic phase vocoder
//! replaces the phase with a running sum of estimated frequencies divided by
//! the factor. That makes every output bin a steady tone at its centre
//! frequency, and turns noise into a buzz at multiples of fs / 1024 (Oversample
//! found this).
//!
//! Oversample's offline vocoder fills output bin `j` from input bin
//! `j × factor`. That misses tones between those bins: with 1024-point frames a
//! 50 kHz tone is at bin 266.7, and factor 10 reads bins 260 and 270.
//!
//! Limits: a tone comes out up to 0.9 dB quieter, depending on where its
//! shifted frequency falls between bins. A fast FM call (a sweep of 30 kHz or
//! more within a few ms) comes out at about its input level on average, but a
//! single call can be several dB louder or quieter, and its sweep is smeared
//! by up to about ±2 bins (±375 Hz at 187.5 Hz per bin).
//!
//! The interpolation also leaves images of the output around 48 kHz, which
//! are above hearing: 34 dB below a 6 kHz output tone, and nearly as loud as
//! the tone when it is close to 24 kHz.
//!
//! An optional 15 kHz high-pass (on by default) leaves out the bins below
//! 15 kHz before the shift, so voices and wind do not come out as low rumble.

use core::f32::consts::FRAC_PI_2;

use microfft::Complex32;

mod fft;

use fft::{PHASOR_STEPS, PHASORS};

/// Samples in one analysis frame.
pub const FRAME_LEN: usize = 1024;
/// Samples between frames. 288 costs 11% less than 256 (a quarter frame)
/// with the same figures for steady tones in `measurements`; at 320 their
/// median tone to residual falls from 49 to 41 dB. Unlike 256, 288 does not
/// divide the frame, so noise comes out with its level rising and falling
/// once a hop: 0.6 dB from top to bottom at factor 4, 0.25 dB at factor 10
/// (0.05 dB, the measuring limit, at 256). A multiple of [`DECIMATION`].
pub const HOP: usize = 288;

pub const DEFAULT_FACTOR: f32 = 10.0;
/// The smallest factor: the shifted sound must fit below 24 kHz (see [`Stft`]).
pub const MIN_FACTOR: f32 = 4.0;
pub const MAX_FACTOR: f32 = 32.0;
pub const DEFAULT_HIGHPASS_HZ: f32 = 15_000.0;

/// Complex values from microfft's 1024-point real FFT. Bin 0 holds DC in its
/// real part and the Nyquist bin in its imaginary part.
const HALF: usize = FRAME_LEN / 2;

/// Peaks below this fraction of the frame's strongest bin, in power, are
/// ignored: -60 dB.
const PEAK_THRESHOLD: f32 = 1e-6;

/// Bins up to this far from a peak move with it. This covers a Hann window's
/// main lobe (2 bins) and its strongest sidelobes.
const PEAK_REACH: usize = 4;

/// Bins away from any peak move in blocks of this many. Wider blocks keep more
/// of a fast FM call's level but smear its sweep more; 5 was best over a set of
/// test calls.
const BLOCK: usize = 5;

/// Turns of rotation per hop for a shift of one bin.
const TURNS_PER_BIN: f32 = HOP as f32 / FRAME_LEN as f32;

/// Pitch shifter for one channel: frequencies divided by a factor, duration
/// kept.
///
/// About 17 KB. On a small stack, build it in place (for example in a static)
/// rather than as a local.
#[derive(Clone, Debug)]
pub struct PitchShifter {
    stft: Stft,
    shift: PeakShift,
    /// The first bin at or above the high-pass cutoff.
    cutoff_bin: usize,
    highpass_on: bool,
}

impl PitchShifter {
    /// A shifter with factor [`DEFAULT_FACTOR`] and the high-pass on. The
    /// sample rate only sets which bin the high-pass starts at.
    ///
    /// This builds the shifter (about 17 KB) on the stack and returns it. On a
    /// microcontroller, use [`unconfigured`](Self::unconfigured) in a `static`
    /// and call [`init`](Self::init) on it instead.
    pub fn new(sample_rate: f32) -> Self {
        let mut shifter = Self::unconfigured();
        shifter.init(sample_rate);
        shifter
    }

    /// A shifter with no sample rate yet, for a `static`, so it is never
    /// copied through the stack. Call [`init`](Self::init) before use; until
    /// then the high-pass passes everything.
    pub const fn unconfigured() -> Self {
        Self { stft: Stft::new(), shift: PeakShift::new(DEFAULT_FACTOR), cutoff_bin: 1, highpass_on: true }
    }

    /// Set the sample rate, restore the default settings and clear the
    /// history, in place.
    pub fn init(&mut self, sample_rate: f32) {
        let bin_hz = sample_rate / FRAME_LEN as f32;
        self.cutoff_bin = (libm::ceilf(DEFAULT_HIGHPASS_HZ / bin_hz) as usize).clamp(1, MAX_INPUT_BIN);
        self.shift.set_factor(DEFAULT_FACTOR);
        self.highpass_on = true;
        self.reset();
    }

    /// Divide frequencies by `factor`, clamped to [`MIN_FACTOR`] to
    /// [`MAX_FACTOR`]. Takes effect from the next frame (within [`HOP`]
    /// samples).
    /// A value that is not finite is ignored.
    pub fn set_factor(&mut self, factor: f32) {
        if factor.is_finite() {
            self.shift.set_factor(factor.clamp(MIN_FACTOR, MAX_FACTOR));
        }
    }

    pub fn factor(&self) -> f32 {
        self.shift.factor
    }

    /// Switch the 15 kHz high-pass on or off. It drops the bins below 15 kHz
    /// before the shift, from the next frame.
    pub fn set_highpass(&mut self, on: bool) {
        self.highpass_on = on;
    }

    pub fn highpass(&self) -> bool {
        self.highpass_on
    }

    /// Delay from input to output, in samples: one frame, and one sample at
    /// the quarter rate for the interpolation.
    pub fn latency(&self) -> usize {
        FRAME_LEN + DECIMATION
    }

    pub fn reset(&mut self) {
        self.stft.reset();
        self.shift.reset();
    }

    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        debug_assert_eq!(input.len(), output.len());
        let mut done = 0;
        while done < input.len() {
            let n = self.stft.room().min(input.len() - done);
            self.stft.push(&input[done..done + n], &mut output[done..done + n]);
            done += n;
            if self.stft.frame_due() {
                let shift = &mut self.shift;
                let lowest = if self.highpass_on { self.cutoff_bin } else { 1 };
                self.stft.frame(|src, dst| shift.apply(src, dst, lowest));
            }
        }
    }
}

/// Hann-windowed short-time Fourier transform and overlap-add resynthesis:
/// 1024-point frames every HOP samples. The shifted spectrum has nothing above
/// 24 kHz (96 kHz divided by [`MIN_FACTOR`]), so each frame is resynthesised at
/// a quarter of the sample rate, from bins 0 to [`OUT_BINS`] - 1, with a
/// 256-point inverse transform, and overlap-added there. The output is that
/// signal interpolated linearly back to the full rate, delayed by one frame
/// and [`DECIMATION`] samples.
#[derive(Clone, Debug)]
struct Stft {
    /// The last FRAME_LEN input samples, in a circular buffer that starts at
    /// `start`. The newest HOP, just before `start`, are filled in during the
    /// current hop.
    input: [f32; FRAME_LEN],
    /// Where the oldest input sample is: always even.
    start: usize,
    /// Overlap-add sums at the quarter rate, oldest first. The first
    /// LOW_HOP + 1 are complete; the output during the current hop is
    /// interpolated between them.
    acc: [f32; LOW_LEN + 1],
    /// The spectrum of the last frame.
    spectrum: [Complex32; HALF],
    /// The modified spectrum (see [`Shifted`]), then the inverse transform.
    spec: Shifted,
    /// Samples taken since the last frame.
    fill: usize,
}

/// The output is resynthesised at the sample rate divided by this.
pub const DECIMATION: usize = 4;
/// Samples in a frame at the quarter rate.
const LOW_LEN: usize = FRAME_LEN / DECIMATION;
/// Samples between frames at the quarter rate.
const LOW_HOP: usize = HOP / DECIMATION;
/// Bins of the shifted spectrum: those below the quarter rate's Nyquist bin.
const OUT_BINS: usize = LOW_LEN / 2;
const _: () = assert!(MAX_INPUT_BIN as f32 / MIN_FACTOR <= OUT_BINS as f32);
/// Highest input bin that carries sound: just below Nyquist.
const MAX_INPUT_BIN: usize = HALF - 1;

/// The shifted spectrum: bin `j` (0 to OUT_BINS - 1) is at `j + GUARD`, in
/// microfft's packed layout. Bins moved below 1 or to OUT_BINS and above land
/// in the GUARD places either side, which the inverse transform leaves out
/// with bin 0, so the bins are moved without checking where they land.
type Shifted = [Complex32; OUT_BINS + 2 * GUARD];
/// A bin moves with a peak or block whose frequency is within 4.5 bins of it,
/// so it lands within 5 bins of that frequency divided by the factor: from -5
/// to 133.
const GUARD: usize = 8;
const _: () = assert!(HALF as f32 / MIN_FACTOR + PEAK_REACH as f32 + 1.0 < (OUT_BINS + GUARD) as f32);

/// Periodic Hann window: sin²(πn / FRAME_LEN). Only for building
/// [`SYNTHESIS`] and for tests: [`fft::windowed_rfft_1024`] applies the window
/// in the spectrum.
const WINDOW: [f32; FRAME_LEN] = {
    let mut w = [0.0; FRAME_LEN];
    let mut n = 0;
    while n < FRAME_LEN {
        let s = cos_sin_turns(n as f32 / (2 * FRAME_LEN) as f32).1;
        w[n] = s * s;
        n += 1;
    }
    w
};

/// Synthesis window at the quarter rate: every 4th sample of the Hann
/// window, divided by the sum of the squared Hann window over the frames that
/// overlap each sample (1.5 if HOP were a quarter frame; at other hops it
/// varies along the frame), by FRAME_LEN for the inverse transform and
/// by the 8 that [`fft::windowed_rfft_1024`] multiplies its spectrum by.
/// Frames analysed with [`WINDOW`] and overlap-added with this one give back
/// the input's band below 24 kHz.
static SYNTHESIS: [f32; LOW_LEN] = {
    let mut w = [0.0; LOW_LEN];
    let mut m = 0;
    while m < LOW_LEN {
        let n = m * DECIMATION;
        let mut sum = 0.0;
        let mut k = n % HOP;
        while k < FRAME_LEN {
            sum += WINDOW[k] * WINDOW[k];
            k += HOP;
        }
        w[m] = WINDOW[n] / (sum * (8 * FRAME_LEN) as f32);
        m += 1;
    }
    w
};

/// Linear interpolation from the quarter rate turns a tone at bin k down by
/// sinc²(k / LOW_LEN). This undoes that, bin by bin: 1.3 dB at 10 kHz, 7.8 dB
/// at 24 kHz.
static DROOP: [f32; OUT_BINS] = {
    let mut g = [1.0; OUT_BINS];
    let mut k = 1;
    while k < OUT_BINS {
        let x = k as f32 / LOW_LEN as f32;
        let sinc = cos_sin_turns(0.5 * x).1 / (core::f32::consts::PI * x);
        g[k] = 1.0 / (sinc * sinc);
        k += 1;
    }
    g
};

const _: () = assert!(fft::INV == OUT_BINS && HOP.is_multiple_of(DECIMATION));

impl Stft {
    const fn new() -> Self {
        Self {
            input: [0.0; FRAME_LEN],
            start: 0,
            acc: [0.0; LOW_LEN + 1],
            spectrum: [Complex32::new(0.0, 0.0); HALF],
            spec: [Complex32::new(0.0, 0.0); OUT_BINS + 2 * GUARD],
            fill: 0,
        }
    }

    fn reset(&mut self) {
        self.input.fill(0.0);
        self.start = 0;
        self.acc.fill(0.0);
        self.fill = 0;
    }

    /// Input samples that can be taken before the next frame is due.
    fn room(&self) -> usize {
        HOP - self.fill
    }

    /// Store input samples (at most [`room`](Self::room)) and write the output
    /// samples one frame and DECIMATION samples older.
    #[inline]
    fn push(&mut self, x: &[f32], y: &mut [f32]) {
        let at = (self.start + FRAME_LEN - HOP + self.fill) % FRAME_LEN;
        let (first, wrapped) = x.split_at(x.len().min(FRAME_LEN - at));
        self.input[at..at + first.len()].copy_from_slice(first);
        self.input[..wrapped.len()].copy_from_slice(wrapped);
        for (j, out) in y.iter_mut().enumerate() {
            let f = self.fill + j;
            let (m, r) = (f / DECIMATION, f % DECIMATION);
            let (from, to) = (self.acc[m], self.acc[m + 1]);
            *out = from + (to - from) * (r as f32 / DECIMATION as f32);
        }
        self.fill += x.len();
    }

    fn frame_due(&self) -> bool {
        self.fill == HOP
    }

    /// Transform the last frame, let `modify` write the shifted spectrum from
    /// it (microfft's packed layout, with DC and Nyquist in bin 0; see
    /// [`Shifted`]), and overlap-add the result.
    fn frame(&mut self, modify: impl FnOnce(&[Complex32; HALF], &mut Shifted)) {
        self.fill = 0;
        fft::windowed_rfft_1024(&self.input, self.start, &mut self.spectrum);
        self.start = (self.start + HOP) % FRAME_LEN;
        modify(&self.spectrum, &mut self.spec);

        self.acc.copy_within(LOW_HOP.., 0);
        self.acc[LOW_LEN + 1 - LOW_HOP..].fill(0.0);
        let acc = &mut self.acc;
        // DC and the quarter rate's Nyquist bin carry no shifted sound, and
        // the inverse transform leaves them out.
        let bins = (&mut self.spec[GUARD..GUARD + OUT_BINS]).try_into().unwrap();
        fft::inverse_rfft_256(bins, &DROOP, |k, z| {
            acc[2 * k + 1] += z.re * SYNTHESIS[2 * k];
            acc[2 * k + 2] -= z.im * SYNTHESIS[2 * k + 1];
        });
    }
}

/// Spectral peak shifting for [`Stft`].
#[derive(Clone, Debug)]
struct PeakShift {
    factor: f32,
    /// Shift in bins per bin of frequency: 1 / factor - 1.
    shift_per_bin: f32,
    /// Power of bins 0 to HALF (Nyquist).
    power: [f32; HALF + 1],
    /// Rotation and shift of each bin's peak in the last frame.
    track: [Track; HALF + 1],
    /// Bins of this frame's peaks, in order. Peaks are at least 2 bins apart,
    /// so there are at most HALF / 2, and one more place is written to.
    peaks: [u16; HALF / 2 + 1],
}

impl PeakShift {
    const fn new(factor: f32) -> Self {
        Self {
            factor,
            shift_per_bin: 1.0 / factor - 1.0,
            power: [0.0; HALF + 1],
            track: [Track::ZERO; HALF + 1],
            peaks: [0; HALF / 2 + 1],
        }
    }

    fn set_factor(&mut self, factor: f32) {
        self.factor = factor;
        self.shift_per_bin = 1.0 / factor - 1.0;
    }

    fn reset(&mut self) {
        self.track.fill(Track::ZERO);
    }

    /// Write the shifted spectrum of `src` to `dst`, both packed, from the
    /// bins of `src` from `lowest` up (at least 1). The bins below are left
    /// out, which is the high-pass.
    fn apply(&mut self, src: &[Complex32; HALF], dst: &mut Shifted, lowest: usize) {
        dst.fill(Complex32::new(0.0, 0.0));

        // Powers from the bin below `lowest`, which the peak search compares
        // with, to Nyquist. Peaks are searched for from `lowest`, so loud sound
        // below the cutoff does not raise the threshold.
        self.power[HALF] = src[0].im * src[0].im;
        let below = lowest - 1;
        self.power[below] = if below == 0 { src[0].re * src[0].re } else { src[below].norm_sqr() };
        let mut strongest = 0.0f32;
        for (p, c) in self.power[lowest..HALF].iter_mut().zip(&src[lowest..]) {
            *p = c.norm_sqr();
            strongest = strongest.max(*p);
        }

        // Without a branch, which on noise goes one way or the other at random.
        let threshold = strongest * PEAK_THRESHOLD;
        let mut count = 0;
        for (k, w) in self.power[below..].windows(3).enumerate() {
            self.peaks[count] = (below + k + 1) as u16;
            count += usize::from((w[1] > threshold) & (w[1] > w[0]) & (w[1] >= w[2]));
        }

        // Each peak's frequency, in place of its power: peaks are at least 2
        // bins apart, so no later peak reads that power.
        for &k in &self.peaks[..count] {
            let k = k as usize;
            self.power[k] = k as f32 + peak_offset(self.power[k - 1], self.power[k], self.power[k + 1]);
        }

        // Bins `lowest` to HALF - 1 (not DC or Nyquist). Bins within reach of
        // a peak move with it; the others move in fixed blocks.
        let mut next = lowest;
        for i in 0..count {
            let k = self.peaks[i] as usize;
            let hi = if i + 1 == count { HALF - 1 } else { (k + self.peaks[i + 1] as usize) / 2 };
            let core_from = next.max(k.saturating_sub(PEAK_REACH));
            let core_to = hi.min(k + PEAK_REACH);
            self.move_blocks(src, dst, next, core_from);
            let freq = self.power[k];
            self.move_bins(src, dst, core_from, core_to, k, freq, true);
            self.move_blocks(src, dst, core_to + 1, hi + 1);
            next = hi + 1;
        }
        self.move_blocks(src, dst, next, HALF);
    }

    /// Move bins `from..end` in blocks of [`BLOCK`] bins, aligned to multiples
    /// of [`BLOCK`] so the same bins are grouped in every frame.
    fn move_blocks(&mut self, src: &[Complex32; HALF], dst: &mut Shifted, from: usize, end: usize) {
        let mut a = from;
        while a < end {
            let b = ((a / BLOCK + 1) * BLOCK).min(end);
            self.move_bins(src, dst, a, b - 1, (a + b - 1) / 2, 0.5 * (a + b - 1) as f32, false);
            a = b;
        }
    }

    /// Add bins `from..=to` of `src` into `dst`, moved together by the whole
    /// number of bins nearest to where `freq` (in bins) lands, and rotated.
    /// The rotation continues from the one stored at bin `key` in the last
    /// frame, and is stored for the moved bins for the next frame.
    ///
    /// For a peak (`exact`), the rotation advances by the exact shift, so a
    /// tone comes out at exactly `freq / factor`. Other bins carry broadband
    /// sound, such as a fast FM call, and advance by the whole shift, which
    /// makes overlapping frames agree exactly while the shift holds.
    #[allow(clippy::too_many_arguments)]
    fn move_bins(
        &mut self,
        src: &[Complex32; HALF],
        dst: &mut Shifted,
        from: usize,
        to: usize,
        key: usize,
        freq: f32,
        exact: bool,
    ) {
        let shift = self.shift_per_bin * freq;
        let whole = round_to_int(shift);
        let rate = if exact { shift } else { whole as f32 };
        // Advance by the mean of this and the last frame's rate, so a peak
        // whose shift changes (a chirp) gets the rate from halfway through the
        // hop.
        let last = self.track[key];
        let turns = wrap_turns(last.turns + 0.5 * (last.rate + rate) * TURNS_PER_BIN);

        // `turns` is the rotation at the centre of the frame. Moving by
        // `whole` bins turns the centre by `whole / 2` turns, which this takes
        // back, so frames with different whole shifts agree there: half a
        // turn more when `whole` is odd.
        let rot = unit_phasor(turns);
        let rot = if whole % 2 == 0 { rot } else { -rot };
        let at = (GUARD as isize + whole as isize) as usize;
        let moved = dst[at.wrapping_add(from)..=at.wrapping_add(to)].iter_mut();
        for ((d, &x), t) in moved.zip(&src[from..=to]).zip(&mut self.track[from..=to]) {
            *d += x * rot;
            *t = Track { turns, rate };
        }
    }
}

/// The rotation of a peak or block, stored at each of its bins for the next
/// frame.
#[derive(Clone, Copy, Debug)]
struct Track {
    /// Rotation at the centre of the frame, in turns (-0.5 to 0.5).
    turns: f32,
    /// Shift, in bins.
    rate: f32,
}

impl Track {
    const ZERO: Self = Self { turns: 0.0, rate: 0.0 };
}

/// Offset in bins (-0.5 to 0.5) of a tone from the centre of its peak bin,
/// from the powers of the peak bin and its two neighbours. Exact for a single
/// tone under a Hann window (Grandke, 1983).
#[allow(clippy::manual_clamp)] // `clamp` would keep a NaN.
fn peak_offset(left: f32, centre: f32, right: f32) -> f32 {
    if centre <= 0.0 {
        return 0.0;
    }
    let (side, sign) = if right > left { (right, 1.0) } else { (left, -1.0) };
    // Magnitude ratio a = (1 + d) / (2 - d) for a tone d bins from the centre,
    // so d = (2a - 1) / (a + 1). With a = sqrt(side / centre), that is
    // (2g - centre) / (g + centre) for g = sqrt(side × centre), which takes
    // one division instead of two.
    let g = sqrt(side * centre);
    // `max` before `min` turns a NaN (from an overflowing product) into 0, so
    // the peak's bins always land inside [`Shifted`].
    sign * ((2.0 * g - centre) / (g + centre)).max(0.0).min(0.5)
}

/// Square root, within 5 parts in a million, for numbers from 1e-30 to 1e30:
/// a peak's offset comes out within 4e-6 bins. libm's `sqrtf` is a slow
/// routine in software on the X8, and Rust has no hardware square root
/// without the standard library.
///
/// Starts from an estimate of 1 / sqrt(x) made from the bits of `x`
/// (within 3.5%), refined by two Newton steps.
#[inline]
fn sqrt(x: f32) -> f32 {
    let mut y = f32::from_bits(0x5f37_5a86 - (x.to_bits() >> 1));
    for _ in 0..2 {
        y *= 1.5 - 0.5 * x * y * y;
    }
    x * y
}

/// e^(2πi t) for `t` from -0.5 to 0.5 turns, interpolated linearly from
/// [`PHASORS`]: within 6e-6 of the exact value. It is faster than
/// [`cos_sin_turns`]: on a PC the peak shifting took 4.3 ms per second of audio
/// with it, 5.7 ms without.
#[inline]
fn unit_phasor(t: f32) -> Complex32 {
    // Looked up half a turn on, so the index is not negative, then negated.
    let x = t * PHASOR_STEPS as f32 + (PHASOR_STEPS / 2) as f32;
    let i = (x as usize).min(PHASOR_STEPS - 1);
    let f = x - i as f32;
    let (a, b) = (PHASORS[i], PHASORS[i + 1]);
    Complex32::new((a.re - b.re) * f - a.re, (a.im - b.im) * f - a.im)
}

/// Nearest integer, halves away from zero. For values well inside the i32
/// range. Without a branch, since the sign of a rotation is random.
#[inline]
const fn round_to_int(x: f32) -> i32 {
    (x + 0.5f32.copysign(x)) as i32
}

/// The same angle in -0.5 to 0.5 turns.
#[inline]
fn wrap_turns(t: f32) -> f32 {
    t - round_to_int(t) as f32
}

/// Cosine and sine of an angle in turns, for angles within about ±1000 turns.
/// Splits off whole quarter turns and uses Taylor series on the rest
/// (±π/4), so it needs no f64.
#[inline]
const fn cos_sin_turns(t: f32) -> (f32, f32) {
    let q = 4.0 * t;
    let n = round_to_int(q);
    let x = (q - n as f32) * FRAC_PI_2;
    let x2 = x * x;
    let s = x * (1.0 + x2 * (-1.0 / 6.0 + x2 * (1.0 / 120.0 + x2 * (-1.0 / 5040.0 + x2 / 362_880.0))));
    let c = 1.0 + x2 * (-0.5 + x2 * (1.0 / 24.0 + x2 * (-1.0 / 720.0 + x2 / 40_320.0)));
    match n & 3 {
        0 => (c, s),
        1 => (-s, c),
        2 => (-c, -s),
        _ => (s, -c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{chirp, noise, rms, run_blocks, sine, tone_power, zero_crossing_freq};
    use core::f32::consts::PI;
    use std::vec::Vec;

    const FS: f32 = 192_000.0;

    fn shift(x: &[f32], factor: f32, highpass: bool) -> Vec<f32> {
        let mut p = PitchShifter::new(FS);
        p.set_factor(factor);
        p.set_highpass(highpass);
        run_blocks(x, 256, |i, o| p.process(i, o))
    }

    fn db(ratio: f32) -> f32 {
        20.0 * libm::log10f(ratio)
    }

    #[test]
    fn unit_phasor_matches_f64() {
        let mut worst = 0.0f64;
        for i in -40_000..=40_000 {
            let t = i as f32 / 80_000.0;
            let z = unit_phasor(t);
            let a = core::f64::consts::TAU * t as f64;
            worst = worst.max((z.re as f64 - a.cos()).abs()).max((z.im as f64 - a.sin()).abs());
        }
        assert!(worst < 6e-6, "worst error {worst}");
    }

    #[test]
    fn sqrt_matches_f64() {
        let mut worst = 0.0f64;
        for i in -3000..=3000 {
            let x = libm::powf(10.0, i as f32 / 100.0) * 1.000_123;
            let r = sqrt(x) as f64 / (x as f64).sqrt() - 1.0;
            worst = worst.max(r.abs());
        }
        assert!(worst < 5e-6, "worst relative error {worst}");
        assert_eq!(sqrt(0.0), 0.0);
    }

    #[test]
    fn cos_sin_turns_matches_f64() {
        let mut worst = 0.0f64;
        for i in -40_000..=40_000 {
            let t = i as f32 * 0.000_123_7;
            let (c, s) = cos_sin_turns(t);
            let a = core::f64::consts::TAU * t as f64;
            worst = worst.max((c as f64 - a.cos()).abs()).max((s as f64 - a.sin()).abs());
        }
        assert!(worst < 5e-7, "worst error {worst}");
    }

    #[test]
    fn stft_without_a_change_returns_sound_below_24khz_one_frame_later() {
        // Tones well below 24 kHz come through, delayed by the latency. The
        // linear interpolation leaves images around 48 kHz, 34 dB down for the
        // 6 kHz tone.
        let n = 20_000;
        let mut x = sine(FS, 1_000.0, 0.3, n);
        for (f, a) in [(3_100.0, 0.2), (6_000.0, 0.2)] {
            for (v, t) in x.iter_mut().zip(sine(FS, f, a, n)) {
                *v += t;
            }
        }
        let mut stft = Stft::new();
        let y: Vec<f32> = x
            .iter()
            .map(|&v| {
                let mut out = [0.0];
                stft.push(&[v], &mut out);
                let out = out[0];
                if stft.frame_due() {
                    stft.frame(|src, dst| dst[GUARD..GUARD + OUT_BINS].copy_from_slice(&src[..OUT_BINS]));
                }
                out
            })
            .collect();
        let delay = FRAME_LEN + DECIMATION;
        let early = rms(&y[..FRAME_LEN - HOP]) / rms(&x);
        assert!(early < 0.01, "output before the latency: {early} of the input");
        let error: Vec<f32> = (2 * FRAME_LEN..n).map(|t| y[t] - x[t - delay]).collect();
        let relative = rms(&error) / rms(&x);
        assert!(relative < 0.03, "error {relative} of the input");
    }

    #[test]
    fn peak_offset_finds_a_tone_between_bins() {
        for bin in [100.0f32, 100.2, 100.5, 200.7, 300.9, 411.35] {
            let x = sine(FS, bin * FS / FRAME_LEN as f32, 1.0, FRAME_LEN);
            let mut buf: [f32; FRAME_LEN] = core::array::from_fn(|i| x[i] * WINDOW[i]);
            let spec = microfft::real::rfft_1024(&mut buf);
            let power: Vec<f32> = spec.iter().map(|c| c.norm_sqr()).collect();
            let k = (1..HALF - 1).max_by(|&a, &b| power[a].total_cmp(&power[b])).unwrap();
            let est = k as f32 + peak_offset(power[k - 1], power[k], power[k + 1]);
            assert!((est - bin).abs() < 0.02, "tone at bin {bin}, estimated {est}");
        }
    }

    #[test]
    fn shifts_a_50khz_tone_to_5khz_at_the_same_level() {
        let x = sine(FS, 50_000.0, 0.5, 38_400);
        let y = shift(&x, 10.0, true);
        let tail = &y[4 * FRAME_LEN..];
        let f = zero_crossing_freq(tail, FS);
        assert!((f - 5_000.0).abs() < 10.0, "output at {f} Hz");
        let gain = db(rms(tail) / rms(&x));
        assert!(gain.abs() < 0.5, "gain {gain} dB");
        assert!(tone_power(tail, FS, 50_000.0) < 1e-6);
    }

    #[test]
    fn shifts_tones_between_bins_by_non_integer_factors() {
        for (f_in, factor) in [(41_000.0, 7.3), (47_123.0, 10.0), (88_800.0, 31.5), (23_456.0, 4.0)] {
            let x = sine(FS, f_in, 0.5, 38_400);
            let y = shift(&x, factor, true);
            let tail = &y[4 * FRAME_LEN..];
            let expect = f_in / factor;
            let f = zero_crossing_freq(tail, FS);
            assert!((f - expect).abs() < expect * 0.002, "{f_in} Hz / {factor}: {f} Hz, expected {expect}");
            let gain = db(rms(tail) / rms(&x));
            assert!(gain.abs() < 1.0, "{f_in} Hz / {factor}: gain {gain} dB");
        }
    }

    #[test]
    fn shifts_a_chirp_from_80_to_30khz_down_to_8_to_3khz() {
        let n = (0.3 * FS) as usize;
        let x = chirp(FS, 80_000.0, 30_000.0, 0.5, n);
        let mut padded = x.clone();
        padded.resize(n + FRAME_LEN, 0.0);
        let y = shift(&padded, 10.0, true);
        let win = 2048;
        let mut last = f32::INFINITY;
        for step in 1..10 {
            let centre = n * step / 10;
            let f_in = 80_000.0 - 50_000.0 * centre as f32 / n as f32;
            let out = &y[centre + FRAME_LEN - win / 2..centre + FRAME_LEN + win / 2];
            let f = zero_crossing_freq(out, FS);
            let expect = f_in / 10.0;
            assert!((f - expect).abs() < expect * 0.005, "at {centre}: {f} Hz, expected {expect}");
            assert!(f < last);
            last = f;
            let gain = db(rms(out) / 0.5 * core::f32::consts::SQRT_2);
            assert!(gain.abs() < 1.5, "at {centre}: gain {gain} dB");
        }
    }

    #[test]
    fn block_size_does_not_change_the_output() {
        let n = 20_000;
        let mut x = chirp(FS, 90_000.0, 25_000.0, 0.4, n);
        for (v, r) in x.iter_mut().zip(noise(0.05, n, 11)) {
            *v += r;
        }
        let run = |block: usize| {
            let mut p = PitchShifter::new(FS);
            p.set_factor(9.3);
            run_blocks(&x, block, |i, o| p.process(i, o))
        };
        let y1 = run(1);
        assert_eq!(y1, run(100));
        assert_eq!(y1, run(4096));
    }

    #[test]
    fn changing_the_factor_mid_stream_stays_finite_and_bounded() {
        let n = 96_000;
        let mut x = sine(FS, 45_000.0, 0.5, n);
        for (v, r) in x.iter_mut().zip(noise(0.3, n, 5)) {
            *v += r;
        }
        let factors = [2.0, 32.0, 1.0, 100.0, 7.7, f32::NAN, 10.0, 2.01, 31.99, 13.0];
        let mut p = PitchShifter::new(FS);
        let mut y = std::vec![0.0; n];
        for (i, (xi, yi)) in x.chunks(333).zip(y.chunks_mut(333)).enumerate() {
            p.set_factor(factors[i % factors.len()]);
            assert!((MIN_FACTOR..=MAX_FACTOR).contains(&p.factor()));
            p.process(xi, yi);
        }
        let peak = y.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(y.iter().all(|v| v.is_finite()));
        assert!(peak < 3.0, "peak {peak}");
        assert!(rms(&y[FRAME_LEN..]) > 0.05, "output almost silent");
    }

    #[test]
    fn huge_nan_and_infinite_input_does_not_panic() {
        // Powers this large overflow when multiplied in `peak_offset`.
        let huge = sine(FS, 50_000.0, 1e7, 20_000);
        let y = shift(&huge, 10.0, true);
        assert!(y[4 * FRAME_LEN..].iter().any(|v| v.is_finite() && v.abs() > 1e6));
        let mut x = sine(FS, 50_000.0, 0.5, 40_000);
        x[10_000] = f32::NAN;
        x[20_000] = f32::INFINITY;
        x[20_001] = f32::NEG_INFINITY;
        let y = shift(&x, 7.0, true);
        // The shifter recovers once the bad samples have left its frames.
        assert!(y[30_000..].iter().all(|v| v.is_finite()));
    }

    /// Average power at `freqs` over 2048-sample Hann-windowed segments, half
    /// overlapping.
    fn power_spectrum(x: &[f32], freqs: &[f32]) -> Vec<f64> {
        const N: usize = 2048;
        let mut sum = std::vec![0.0f64; freqs.len()];
        for start in (0..x.len() - N).step_by(N / 2) {
            let seg: Vec<f32> = (0..N)
                .map(|i| {
                    let w = libm::sinf(PI * i as f32 / N as f32);
                    x[start + i] * w * w
                })
                .collect();
            for (s, &f) in sum.iter_mut().zip(freqs) {
                *s += tone_power(&seg, FS, f) as f64;
            }
        }
        sum
    }

    #[test]
    fn white_noise_comes_out_as_noise_without_tones() {
        let x = noise(0.5, 288_000, 21);
        let y = shift(&x, 10.0, true);
        let y = &y[4 * FRAME_LEN..];

        // The occupied band is 1.5 kHz (15 kHz high-pass) to 9.6 kHz (Nyquist
        // over 10). Inside it, no bin stands out from the rest.
        let hz_per_bin = FS / 2048.0;
        let freqs: Vec<f32> = (27..96).map(|k| k as f32 * hz_per_bin).collect();
        let mut band = power_spectrum(y, &freqs);
        band.sort_by(|a, b| a.total_cmp(b));
        let median = band[band.len() / 2];
        let top = band[band.len() - 1];
        let spread_db = 10.0 * (top / median).log10();
        assert!(spread_db < 3.0, "strongest bin {spread_db:.1} dB above the median");

        // No repetition at the frame or hop length. The buzz of a classic
        // phase vocoder repeats every frame: its output bins are steady tones
        // whose amplitudes have a nonzero mean, which gives a correlation of
        // about 0.8 at a lag of one frame.
        let energy: f64 = y.iter().map(|v| (*v as f64).powi(2)).sum();
        for lag in [HOP, 2 * HOP, 3 * HOP, FRAME_LEN] {
            let r: f64 = y.iter().zip(&y[lag..]).map(|(a, b)| *a as f64 * *b as f64).sum::<f64>() / energy;
            assert!(r.abs() < 0.05, "correlation {r:.3} at lag {lag}");
        }

        // Most of the noise power above 15 kHz comes through.
        let gain = db(rms(y) / rms(&x));
        assert!(gain > -6.0 && gain < 1.0, "noise gain {gain} dB");
    }

    #[test]
    fn highpass_removes_audible_input() {
        let x = sine(FS, 5_000.0, 0.5, 38_400);
        let on = shift(&x, 10.0, true);
        let off = shift(&x, 10.0, false);
        let tail = 4 * FRAME_LEN..;
        let removed = db(rms(&on[tail.clone()]) / rms(&off[tail.clone()]));
        assert!(removed < -30.0, "high-pass removes {removed} dB");
        assert!(db(rms(&off[tail]) / rms(&x)) > -3.0);
    }

    #[test]
    fn latency_is_one_frame_and_a_little() {
        let p = PitchShifter::new(FS);
        assert_eq!(p.latency(), FRAME_LEN + DECIMATION);
        // A tone that starts at sample s comes out from s + latency.
        let s = 5_000;
        let mut x = std::vec![0.0; s];
        x.extend(sine(FS, 50_000.0, 0.5, 10_000));
        let y = shift(&x, 10.0, true);
        let before = rms(&y[s + FRAME_LEN - 600..s + FRAME_LEN - 400]);
        let after = rms(&y[s + FRAME_LEN + 600..s + FRAME_LEN + 800]);
        assert!(before < 0.01 && after > 0.3, "before {before}, after {after}");
    }

    /// A bat-like FM call: a linear sweep under a Hann envelope.
    fn fm_call(f0: f32, f1: f32, dur_s: f32, amp: f32) -> Vec<f32> {
        let n = (dur_s * FS) as usize;
        let mut c = chirp(FS, f0, f1, amp, n);
        for (i, v) in c.iter_mut().enumerate() {
            let w = libm::sinf(PI * i as f32 / n as f32);
            *v *= w * w;
        }
        c
    }

    #[test]
    fn fast_fm_calls_keep_their_level_on_average() {
        // 24 calls: 2 to 7 ms, from 55-95 kHz down to 22-42 kHz, starting at
        // different points in the hop.
        let r = noise(1.0, 4 * 24, 12_345);
        let mut gains = Vec::new();
        let mut freq_errors = Vec::new();
        for c in r.chunks(4) {
            let u = |v: f32| 0.5 * (v + 1.0);
            let (dur, f0, f1) = (0.002 + 0.005 * u(c[0]), 55_000.0 + 40_000.0 * u(c[1]), 22_000.0 + 20_000.0 * u(c[2]));
            let mut x = std::vec![0.0; 3_000 + (256.0 * u(c[3])) as usize];
            let start = x.len();
            x.extend(fm_call(f0, f1, dur, 0.5));
            let end = x.len();
            x.extend(std::vec![0.0; 5_000]);
            let y = shift(&x, 10.0, true);

            let energy = |v: &[f32]| v.iter().map(|s| s * s).sum::<f32>();
            gains.push(10.0 * libm::log10f(energy(&y) / energy(&x)));
            let (a, b) = (start + FRAME_LEN, end + FRAME_LEN);
            let expect = zero_crossing_freq(&x[start..end], FS) / 10.0;
            freq_errors.push((zero_crossing_freq(&y[a..b], FS) / expect - 1.0).abs());
            // The call stays where it was, a frame later.
            assert!(energy(&y[a - 256..b + 256]) > 0.98 * energy(&y), "call smeared in time");
        }
        let mean = gains.iter().sum::<f32>() / gains.len() as f32;
        assert!(mean.abs() < 2.0, "mean gain {mean} dB, gains {gains:?}");
        assert!(gains.iter().all(|g| g.abs() < 8.0), "gains {gains:?}");
        let mean_error = freq_errors.iter().sum::<f32>() / freq_errors.len() as f32;
        assert!(mean_error < 0.05, "mean frequency error {mean_error}");
    }
}

/// Figures for comparing changes to the shifter, not pass or fail tests. Run
/// with `cargo test -p bat-dsp --release pitch_shift::measurements -- --ignored
/// --nocapture`. For timings closer to the X8's shares, which has no SIMD, add
/// `RUSTFLAGS="-C no-vectorize-loops -C no-vectorize-slp"`.
#[cfg(test)]
mod measurements {
    use super::*;
    use std::time::Instant;

    #[test]
    #[ignore]
    /// Steady tones, chirps and noise through the shifter, measured below 21 kHz.
    fn quality_figures() {
        use crate::test_util::{chirp, noise, rms, run_blocks, sine, tone_power};
        const FS: f32 = 192_000.0;
        // Only the audible band counts: two 4th-order low-passes at 21 kHz.
        let shift = |x: &[f32], f: f32| {
            let mut p = PitchShifter::new(FS);
            p.set_factor(f);
            let mut y = run_blocks(x, 256, |i, o| p.process(i, o));
            let mut lp = [
                crate::biquad::Butterworth4::lowpass(FS, 21_000.0),
                crate::biquad::Butterworth4::lowpass(FS, 21_000.0),
            ];
            for v in y.iter_mut() {
                let a = lp[0].process_sample(*v);
                *v = lp[1].process_sample(a);
            }
            y
        };
        let db = |r: f32| 20.0 * libm::log10f(r);
        // Steady tones: level error, envelope ripple, and power off the expected frequency.
        let (mut worst_gain, mut worst_ripple) = (0.0f32, 0.0f32);
        let mut snrs = std::vec::Vec::new();
        for factor in [4.0, 8.0, 10.0, 16.0, 32.0] {
            let mut f_in = 20_000.0;
            while f_in < 92_000.0 {
                let x = sine(FS, f_in, 0.5, 48_000);
                let y = shift(&x, factor);
                let tail = &y[4 * FRAME_LEN..];
                worst_gain = worst_gain.max(db(rms(tail) / rms(&x)).abs());
                let env: std::vec::Vec<f32> = tail.chunks(128).map(rms).collect();
                let (lo, hi) = env.iter().fold((f32::INFINITY, 0.0f32), |(a, b), &v| (a.min(v), b.max(v)));
                worst_ripple = worst_ripple.max(db(hi / lo));
                let total = rms(tail).powi(2);
                let tone = tone_power(tail, FS, f_in / factor);
                let snr = 10.0 * libm::log10f(tone / (total - tone).max(1e-12));
                if snr < 25.0 {
                    std::println!("  {f_in} Hz / {factor}: tone to residual {snr:.1} dB, ripple {:.2}", db(hi / lo));
                }
                snrs.push(snr);
                f_in += 3_137.0;
            }
        }
        snrs.sort_by(f32::total_cmp);
        std::println!(
            "tones: worst level error {worst_gain:.2} dB, worst ripple {worst_ripple:.2} dB, tone to residual: worst {:.1} dB, \
             10th percentile {:.1} dB, median {:.1} dB",
            snrs[0],
            snrs[snrs.len() / 10],
            snrs[snrs.len() / 2]
        );
        // Chirp 80 to 30 kHz over 20 ms (fast) and 300 ms (slow): level ripple along it.
        for ms in [5.0, 20.0, 300.0] {
            let n = (ms / 1000.0 * FS) as usize;
            let mut x = chirp(FS, 80_000.0, 30_000.0, 0.5, n);
            x.resize(n + 2 * FRAME_LEN, 0.0);
            let y = shift(&x, 10.0);
            let seg = &y[FRAME_LEN + n / 10..FRAME_LEN + n * 9 / 10];
            let env: std::vec::Vec<f32> = seg.chunks(64).map(rms).collect();
            let (lo, hi) = env.iter().fold((f32::INFINITY, 0.0f32), |(a, b), &v| (a.min(v), b.max(v)));
            std::println!(
                "chirp {ms} ms: level {:.2} dB, ripple {:.2} dB",
                db(rms(seg) / (0.5 / core::f32::consts::SQRT_2)),
                db(hi / lo)
            );
        }
        let x = noise(0.5, 288_000, 21);
        let y = shift(&x, 10.0);
        let y = &y[4 * FRAME_LEN..];
        let e: f64 = y.iter().map(|v| (*v as f64).powi(2)).sum();
        let mut worst_r = 0.0f64;
        for lag in [HOP, 2 * HOP, 3 * HOP, FRAME_LEN] {
            let r: f64 = y.iter().zip(&y[lag..]).map(|(a, b)| *a as f64 * *b as f64).sum::<f64>() / e;
            worst_r = worst_r.max(r.abs());
        }
        std::println!(
            "noise: gain {:.2} dB, worst correlation at 1 to 3 hops or 1 frame {worst_r:.3}",
            db(rms(y) / rms(&x))
        );
    }
    #[test]
    #[ignore]
    /// Milliseconds per second of 192 kHz audio for each part of a frame, and
    /// for the whole shifter, on white noise. Each part is timed on one
    /// second's frames, each different, so that the processor cannot learn
    /// the branches of one frame. Each figure is the fastest of 7 runs.
    fn time_parts() {
        fn best(mut f: impl FnMut()) -> f64 {
            (0..7)
                .map(|_| {
                    let t = Instant::now();
                    f();
                    t.elapsed().as_secs_f64() * 1000.0
                })
                .fold(f64::MAX, f64::min)
        }
        const FS: usize = 192_000;
        let x = crate::test_util::noise(0.5, FS + FRAME_LEN, 2);
        let mut st = Stft::new();
        let frames = FS / HOP;
        let fwd = best(|| {
            for f in 0..frames {
                // Only `start` matters for the time.
                st.start = ((f * HOP) % FRAME_LEN) & !1;
                fft::windowed_rfft_1024(&st.input, st.start, &mut st.spectrum);
                std::hint::black_box(&st.spectrum);
            }
        });
        let spectra: std::vec::Vec<[Complex32; HALF]> = (0..frames)
            .map(|f| {
                let mut ring = [0.0; FRAME_LEN];
                ring.copy_from_slice(&x[f * HOP..f * HOP + FRAME_LEN]);
                let mut s = [Complex32::new(0.0, 0.0); HALF];
                fft::windowed_rfft_1024(&ring, 0, &mut s);
                s
            })
            .collect();
        let mut ps = PeakShift::new(10.0);
        let mut spec = [Complex32::new(0.0, 0.0); OUT_BINS + 2 * GUARD];
        let apply = best(|| {
            for src in &spectra {
                ps.apply(src, &mut spec, 80);
                std::hint::black_box(&spec);
            }
        });
        let mut acc = [0.0f32; LOW_LEN + 1];
        let inv = best(|| {
            for src in &spectra {
                let mut sp: [Complex32; OUT_BINS] = src[..OUT_BINS].try_into().unwrap();
                fft::inverse_rfft_256(&mut sp, &DROOP, |k, z| {
                    acc[2 * k + 1] += z.re * SYNTHESIS[2 * k];
                    acc[2 * k + 2] -= z.im * SYNTHESIS[2 * k + 1];
                });
                std::hint::black_box(&acc);
            }
        });
        let mut p = PitchShifter::new(FS as f32);
        let mut y = std::vec![0.0; 256];
        let all = best(|| {
            for c in x[..FS].chunks(256) {
                p.process(c, &mut y[..c.len()]);
            }
        });
        std::println!("ms per s: forward {fwd:.2}, apply {apply:.2}, inverse and overlap-add {inv:.2}, whole {all:.2}");
    }
}
