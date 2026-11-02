//! Pitch shift: bat calls moved down by a factor into the audible range,
//! keeping their duration, so they are heard in real time.
//!
//! The method is a phase vocoder with peak shifting (Laroche and Dolson, "New
//! phase-vocoder techniques for real-time pitch shifting, chorusing,
//! harmonizing and other exotic audio modifications", JAES 47(11), 1999):
//!
//! 1. Every 256 samples, the last 1024 are windowed (Hann) and transformed.
//! 2. Each local maximum of the magnitude spectrum is a peak. The bins within 4
//!    of a peak (and nearer to it than to the next peak) move together, by the
//!    whole number of bins that puts the peak nearest to its frequency divided
//!    by the factor. Moving them together keeps the shape of the peak, so a
//!    tone keeps its level.
//! 3. The other bins carry broadband sound, such as a fast FM call. They move
//!    in fixed blocks of 5 bins, each block by the shift of its centre.
//! 4. Moving a bin by `d` bins changes its frequency within the frame but not
//!    its phase at the centre of the frame. To keep the output continuous from
//!    one frame to the next, the moved bins are also rotated by `d × 256 / 1024`
//!    turns more each hop. For peaks this `d` is not rounded, so a tone comes
//!    out at exactly its frequency divided by the factor. Each peak or block
//!    continues the rotation stored at its bin in the previous frame.
//! 5. The result is inverse transformed, windowed again and overlap-added.
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
//! An optional 15 kHz high-pass (on by default) removes audible sound before
//! the shift, so voices and wind do not come out as low rumble.

use core::f32::consts::FRAC_PI_2;

use microfft::Complex32;

use crate::biquad::Butterworth4;

/// Samples in one analysis frame.
pub const FRAME_LEN: usize = 1024;
/// Samples between frames: 75% overlap.
pub const HOP: usize = FRAME_LEN / 4;

pub const DEFAULT_FACTOR: f32 = 10.0;
pub const MIN_FACTOR: f32 = 2.0;
pub const MAX_FACTOR: f32 = 32.0;
pub const DEFAULT_HIGHPASS_HZ: f32 = 15_000.0;

/// Complex values from microfft's 1024-point real FFT. Bin 0 holds DC in its
/// real part and the Nyquist bin in its imaginary part.
const HALF: usize = FRAME_LEN / 2;

/// 1 / (sum of the squared Hann window over the overlapping frames), which is
/// 1 / 1.5 at 75% overlap.
const OLA_GAIN: f32 = 2.0 / 3.0;

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
/// About 29 KB. On a small stack, build it in place (for example in a static)
/// rather than as a local.
#[derive(Clone, Debug)]
pub struct PitchShifter {
    stft: Stft,
    shift: PeakShift,
    highpass: Butterworth4,
    highpass_on: bool,
}

impl PitchShifter {
    /// A shifter with factor [`DEFAULT_FACTOR`] and the high-pass on. The
    /// sample rate only sets the high-pass cutoff.
    ///
    /// This builds the shifter (about 23 KB) on the stack and returns it. On a
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
        Self {
            stft: Stft::new(),
            shift: PeakShift::new(DEFAULT_FACTOR),
            highpass: Butterworth4::identity(),
            highpass_on: true,
        }
    }

    /// Set the sample rate, restore the default settings and clear the
    /// history, in place.
    pub fn init(&mut self, sample_rate: f32) {
        self.highpass.set_coefficients(&Butterworth4::highpass(sample_rate, DEFAULT_HIGHPASS_HZ));
        self.shift.set_factor(DEFAULT_FACTOR);
        self.highpass_on = true;
        self.reset();
    }

    /// Divide frequencies by `factor`, clamped to [`MIN_FACTOR`] to
    /// [`MAX_FACTOR`]. Takes effect from the next frame (within 256 samples).
    /// A value that is not finite is ignored.
    pub fn set_factor(&mut self, factor: f32) {
        if factor.is_finite() {
            self.shift.set_factor(factor.clamp(MIN_FACTOR, MAX_FACTOR));
        }
    }

    pub fn factor(&self) -> f32 {
        self.shift.factor
    }

    /// Switch the 15 kHz high-pass before the shift on or off. The filter keeps
    /// running while off, so switching it back on does not click.
    pub fn set_highpass(&mut self, on: bool) {
        self.highpass_on = on;
    }

    pub fn highpass(&self) -> bool {
        self.highpass_on
    }

    /// Delay from input to output, in samples: one frame. The high-pass adds
    /// a few samples of group delay on top.
    pub fn latency(&self) -> usize {
        FRAME_LEN
    }

    pub fn reset(&mut self) {
        self.stft.reset();
        self.shift.reset();
        self.highpass.reset();
    }

    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        debug_assert_eq!(input.len(), output.len());
        for (y, &x) in output.iter_mut().zip(input) {
            let filtered = self.highpass.process_sample(x);
            *y = self.stft.push(if self.highpass_on { filtered } else { x });
            if self.stft.frame_due() {
                let shift = &mut self.shift;
                self.stft.frame(|src, dst| shift.apply(src, dst));
            }
        }
    }
}

/// Hann-windowed short-time Fourier transform and overlap-add resynthesis:
/// 1024-point frames every 256 samples, output delayed by one frame.
#[derive(Clone, Debug)]
struct Stft {
    /// The last FRAME_LEN input samples, oldest first. The newest HOP are
    /// filled in during the current hop.
    input: [f32; FRAME_LEN],
    /// Overlap-add sums, oldest first. The first HOP are complete and are
    /// output during the current hop.
    acc: [f32; FRAME_LEN],
    /// The windowed frame, then its spectrum.
    fft: [f32; FRAME_LEN],
    /// The modified spectrum, then the inverse transform.
    spec: [Complex32; HALF],
    /// Samples taken since the last frame.
    fill: usize,
}

/// Periodic Hann window: sin²(πn / FRAME_LEN).
static WINDOW: [f32; FRAME_LEN] = {
    let mut w = [0.0; FRAME_LEN];
    let mut n = 0;
    while n < FRAME_LEN {
        let s = cos_sin_turns(n as f32 / (2 * FRAME_LEN) as f32).1;
        w[n] = s * s;
        n += 1;
    }
    w
};

/// e^(+2πik / FRAME_LEN) for k in 0..=HALF / 2, for the inverse transform.
static TWIDDLE: [Complex32; HALF / 2 + 1] = {
    let mut t = [Complex32::new(0.0, 0.0); HALF / 2 + 1];
    let mut k = 0;
    while k <= HALF / 2 {
        let (c, s) = cos_sin_turns(k as f32 / FRAME_LEN as f32);
        t[k] = Complex32::new(c, s);
        k += 1;
    }
    t
};

impl Stft {
    const fn new() -> Self {
        Self {
            input: [0.0; FRAME_LEN],
            acc: [0.0; FRAME_LEN],
            fft: [0.0; FRAME_LEN],
            spec: [Complex32::new(0.0, 0.0); HALF],
            fill: 0,
        }
    }

    fn reset(&mut self) {
        self.input.fill(0.0);
        self.acc.fill(0.0);
        self.fill = 0;
    }

    /// Store one input sample and return the output sample one frame older.
    #[inline]
    fn push(&mut self, x: f32) -> f32 {
        self.input[FRAME_LEN - HOP + self.fill] = x;
        let y = self.acc[self.fill];
        self.fill += 1;
        y
    }

    fn frame_due(&self) -> bool {
        self.fill == HOP
    }

    /// Transform the last frame, let `modify` write a new spectrum from it
    /// (both in microfft's packed layout), and overlap-add the result.
    fn frame(&mut self, modify: impl FnOnce(&[Complex32; HALF], &mut [Complex32; HALF])) {
        self.fill = 0;
        for ((f, &x), &w) in self.fft.iter_mut().zip(&self.input).zip(&WINDOW) {
            *f = x * w;
        }
        let src = microfft::real::rfft_1024(&mut self.fft);
        modify(src, &mut self.spec);

        let frame = inverse_rfft(&mut self.spec, &TWIDDLE);
        self.acc.copy_within(HOP.., 0);
        self.acc[FRAME_LEN - HOP..].fill(0.0);
        let gain = OLA_GAIN / FRAME_LEN as f32;
        let pairs = self.acc.as_chunks_mut::<2>().0.iter_mut().zip(WINDOW.as_chunks::<2>().0);
        for ((a, w), z) in pairs.zip(frame) {
            a[0] += z.re * w[0] * gain;
            a[1] -= z.im * w[1] * gain;
        }
        self.input.copy_within(HOP.., 0);
    }
}

/// Inverse of microfft's 1024-point real FFT, using one 512-point complex FFT.
///
/// `spec` is in microfft's packed layout and is overwritten. In the result,
/// frame sample `2m` is `re / FRAME_LEN` of element `m` and sample `2m + 1` is
/// `-im / FRAME_LEN`.
///
/// The even and odd samples of the frame are packed as one complex signal
/// `z[m] = x[2m] + i x[2m+1]`, whose spectrum is rebuilt from the real
/// spectrum. The forward FFT of its conjugate gives the conjugate of the
/// inverse, which skips the reordering and the 1024 divisions of microfft's
/// inverse.
fn inverse_rfft<'a>(
    spec: &'a mut [Complex32; HALF],
    twiddle: &[Complex32; HALF / 2 + 1],
) -> &'a mut [Complex32; HALF] {
    let (dc, nyquist) = (spec[0].re, spec[0].im);
    spec[0] = Complex32::new(dc + nyquist, nyquist - dc);
    for (k, t) in twiddle.iter().enumerate().skip(1) {
        let a = spec[k];
        let b = spec[HALF - k];
        // Twice the spectra of the even and odd samples: e = a + conj(b),
        // o = (a - conj(b)) × twiddle.
        let e = Complex32::new(a.re + b.re, a.im - b.im);
        let o = Complex32::new(a.re - b.re, a.im + b.im) * t;
        // Conjugates of z's spectrum, e + i o, at k and HALF - k.
        spec[k] = Complex32::new(e.re - o.im, -e.im - o.re);
        spec[HALF - k] = Complex32::new(e.re + o.im, e.im - o.re);
    }
    microfft::complex::cfft_512(spec)
}

/// Spectral peak shifting for [`Stft`].
#[derive(Clone, Debug)]
struct PeakShift {
    factor: f32,
    /// Shift in bins per bin of frequency: 1 / factor - 1.
    shift_per_bin: f32,
    /// Power of bins 0 to HALF (Nyquist).
    power: [f32; HALF + 1],
    /// Rotation of each bin's peak in the last frame, in turns.
    rotation: [f32; HALF + 1],
    /// Shift of each bin's peak in the last frame, in bins.
    last_shift: [f32; HALF + 1],
    /// Bins of this frame's peaks, in order. Peaks are at least 2 bins apart,
    /// so there are at most HALF / 2.
    peaks: [u16; HALF / 2],
}

impl PeakShift {
    const fn new(factor: f32) -> Self {
        Self {
            factor,
            shift_per_bin: 1.0 / factor - 1.0,
            power: [0.0; HALF + 1],
            rotation: [0.0; HALF + 1],
            last_shift: [0.0; HALF + 1],
            peaks: [0; HALF / 2],
        }
    }

    fn set_factor(&mut self, factor: f32) {
        self.factor = factor;
        self.shift_per_bin = 1.0 / factor - 1.0;
    }

    fn reset(&mut self) {
        self.rotation.fill(0.0);
        self.last_shift.fill(0.0);
    }

    /// Write the shifted spectrum of `src` to `dst`, both packed.
    fn apply(&mut self, src: &[Complex32; HALF], dst: &mut [Complex32; HALF]) {
        dst.fill(Complex32::new(0.0, 0.0));

        self.power[0] = src[0].re * src[0].re;
        self.power[HALF] = src[0].im * src[0].im;
        let mut strongest = 0.0f32;
        for (p, c) in self.power[1..HALF].iter_mut().zip(&src[1..]) {
            *p = c.norm_sqr();
            strongest = strongest.max(*p);
        }

        let threshold = strongest * PEAK_THRESHOLD;
        let mut count = 0;
        for (k, w) in self.power.windows(3).enumerate() {
            if w[1] > threshold && w[1] > w[0] && w[1] >= w[2] {
                self.peaks[count] = (k + 1) as u16;
                count += 1;
            }
        }

        // Bins 1 to HALF - 1 (not DC or Nyquist). Bins within reach of a peak
        // move with it; the others move in fixed blocks.
        let mut next = 1;
        for i in 0..count {
            let k = self.peaks[i] as usize;
            let hi = if i + 1 == count { HALF - 1 } else { (k + self.peaks[i + 1] as usize) / 2 };
            let core_from = next.max(k.saturating_sub(PEAK_REACH));
            let core_to = hi.min(k + PEAK_REACH);
            self.move_blocks(src, dst, next, core_from);
            let freq = k as f32 + peak_offset(self.power[k - 1], self.power[k], self.power[k + 1]);
            let (whole, rot) = self.plan_move(core_from, core_to, k, freq, true);
            add_shifted(src, dst, core_from, core_to, whole, rot);
            self.move_blocks(src, dst, core_to + 1, hi + 1);
            next = hi + 1;
        }
        self.move_blocks(src, dst, next, HALF);
    }

    /// Move bins `from..end` in blocks of [`BLOCK`] bins, aligned to multiples
    /// of [`BLOCK`] so the same bins are grouped in every frame.
    fn move_blocks(&mut self, src: &[Complex32; HALF], dst: &mut [Complex32; HALF], from: usize, end: usize) {
        let mut a = from;
        while a < end {
            let b = ((a / BLOCK + 1) * BLOCK).min(end);
            let (whole, rot) = self.plan_move(a, b - 1, (a + b - 1) / 2, 0.5 * (a + b - 1) as f32, false);
            add_shifted(src, dst, a, b - 1, whole, rot);
            a = b;
        }
    }

    /// Plan moving bins `from..=to` together to where `freq` (in bins) lands:
    /// returns the whole-bin shift and the rotation to multiply them by. The
    /// rotation continues from the one stored at bin `key` in the last frame,
    /// and is stored for the moved bins for the next frame.
    ///
    /// For a peak (`exact`), the rotation advances by the exact shift, so a
    /// tone comes out at exactly `freq / factor`. Other bins carry broadband
    /// sound, such as a fast FM call, and advance by the whole shift, which
    /// makes overlapping frames agree exactly while the shift holds.
    fn plan_move(&mut self, from: usize, to: usize, key: usize, freq: f32, exact: bool) -> (isize, Complex32) {
        let shift = self.shift_per_bin * freq;
        let whole = round_to_int(shift);
        let rate = if exact { shift } else { whole as f32 };
        // Advance by the mean of this and the last frame's rate, so a peak
        // whose shift changes (a chirp) gets the rate from halfway through the
        // hop.
        let turns = wrap_turns(self.rotation[key] + 0.5 * (self.last_shift[key] + rate) * TURNS_PER_BIN);
        self.rotation[from..=to].fill(turns);
        self.last_shift[from..=to].fill(rate);

        // `turns` is the rotation at the centre of the frame. Moving by
        // `whole` bins turns the centre by `whole / 2` turns, which this takes
        // back, so frames with different whole shifts agree there.
        let (c, s) = cos_sin_turns(turns + 0.5 * whole as f32);
        (whole as isize, Complex32::new(c, s))
    }
}

/// Add `src[from..=to] × rot` into `dst`, `shift` bins along. Bins that would
/// land outside 1..HALF (on DC, Nyquist or beyond) are dropped.
fn add_shifted(
    src: &[Complex32; HALF],
    dst: &mut [Complex32; HALF],
    from: usize,
    to: usize,
    shift: isize,
    rot: Complex32,
) {
    let from = (from as isize).max(1 - shift);
    let to = (to as isize).min(HALF as isize - 1 - shift);
    if from > to {
        return;
    }
    let (src_from, src_to) = (from as usize, to as usize);
    let (dst_from, dst_to) = ((from + shift) as usize, (to + shift) as usize);
    for (d, &x) in dst[dst_from..=dst_to].iter_mut().zip(&src[src_from..=src_to]) {
        *d += x * rot;
    }
}

/// Offset in bins (-0.5 to 0.5) of a tone from the centre of its peak bin,
/// from the powers of the peak bin and its two neighbours. Exact for a single
/// tone under a Hann window (Grandke, 1983).
fn peak_offset(left: f32, centre: f32, right: f32) -> f32 {
    if centre <= 0.0 {
        return 0.0;
    }
    let (side, sign) = if right > left { (right, 1.0) } else { (left, -1.0) };
    // Magnitude ratio a = (1 + d) / (2 - d) for a tone d bins from the centre.
    let a = libm::sqrtf(side / centre);
    sign * ((2.0 * a - 1.0) / (a + 1.0)).clamp(0.0, 0.5)
}

/// Nearest integer, halves away from zero. For values well inside the i32
/// range.
#[inline]
const fn round_to_int(x: f32) -> i32 {
    if x >= 0.0 { (x + 0.5) as i32 } else { (x - 0.5) as i32 }
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
    use core::f32::consts::PI;
    use crate::test_util::{chirp, noise, rms, run_blocks, sine, tone_power, zero_crossing_freq};
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
    fn inverse_rfft_undoes_the_forward_fft() {
        let x = noise(1.0, FRAME_LEN, 7);
        let mut buf: [f32; FRAME_LEN] = x.clone().try_into().unwrap();
        let mut spec = *microfft::real::rfft_1024(&mut buf);
        let z = inverse_rfft(&mut spec, &TWIDDLE);
        for (m, c) in z.iter().enumerate() {
            let even = c.re / FRAME_LEN as f32;
            let odd = -c.im / FRAME_LEN as f32;
            assert!((even - x[2 * m]).abs() < 1e-5, "sample {}: {even} vs {}", 2 * m, x[2 * m]);
            assert!((odd - x[2 * m + 1]).abs() < 1e-5, "sample {}: {odd} vs {}", 2 * m + 1, x[2 * m + 1]);
        }
    }

    #[test]
    fn stft_without_a_change_returns_the_input_one_frame_later() {
        let x = noise(0.5, 20_000, 3);
        let mut stft = Stft::new();
        let y: Vec<f32> = x
            .iter()
            .map(|&v| {
                let out = stft.push(v);
                if stft.frame_due() {
                    stft.frame(|src, dst| dst.copy_from_slice(src));
                }
                out
            })
            .collect();
        assert!(y[..FRAME_LEN].iter().all(|v| v.abs() < 1e-6));
        for t in FRAME_LEN..x.len() {
            assert!((y[t] - x[t - FRAME_LEN]).abs() < 1e-5, "sample {t}: {} vs {}", y[t], x[t - FRAME_LEN]);
        }
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
        for (f_in, factor) in [(41_000.0, 7.3), (47_123.0, 10.0), (88_800.0, 31.5), (23_456.0, 2.0)] {
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
    fn latency_is_one_frame() {
        let p = PitchShifter::new(FS);
        assert_eq!(p.latency(), FRAME_LEN);
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
