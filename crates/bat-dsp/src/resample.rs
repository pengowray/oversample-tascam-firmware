//! Streaming stereo resampler, so a recording at any sample rate plays at its
//! real speed through an output at a fixed rate (192 kHz on the X8).
//!
//! The method is windowed-sinc interpolation (Smith, "Digital Audio
//! Resampling", 2002): each output sample is the input around its time, each
//! input sample weighted by a low-pass kernel centred on that time. The kernel
//! is read from a table of [`PHASES`] points per frame of the lower rate,
//! interpolated linearly between points, so the output can fall anywhere
//! between input samples.
//!
//! The kernel is a sinc with a Kaiser window (beta 6.5), cut off at 0.45 of
//! the lower of the two rates. When the input rate is higher, the kernel is
//! stretched by the ratio of the rates, so sound above 0.45 of the output rate
//! (86 kHz at 192 kHz) is removed rather than folded back below it. Stretching
//! takes more input samples (taps) for each output sample:
//!
//! | Input rate into 192 kHz | Kernel's reach each side, in frames of the lower rate | Taps per output frame per channel |
//! |---|---|---|
//! | 8 to 192 kHz | 16 | 32 |
//! | 250 / 256 / 300 / 384 kHz | 16 | 42 / 44 / 50 / 64 |
//! | 441 / 500 kHz | 12 | 56 / 64 |
//! | 768 kHz | 8 | 64 |
//! | above 768 kHz | 8, cut short | 64 |
//!
//! With a reach of 16 frames of the lower rate, a tone up to 0.4 of the lower
//! rate keeps its level within 0.2 dB, and sound above 0.52 of it is at least
//! 67 dB down. Above twice the output rate, a reach of 16 would need more
//! than 64 taps, so a shorter kernel is used, with a wider band between what
//! is kept and what is removed. With a reach of 12 (441 and 500 kHz), a tone
//! at 76.8 kHz (0.4 of 192 kHz) is 0.6 dB down, and sound above 103 kHz is at
//! least 60 dB down. With 8 (768 kHz), 76.8 kHz is 1.5 dB down, and sound
//! above 110 kHz is at least 60 dB down. Above 4 times the output rate, the
//! kernel's ends are cut off, which lets more sound fold back.
//!
//! Any other sound the resampler adds, from the kernel's ripple and the table,
//! is at least 74 dB below a tone.
//!
//! When the ratio of the rates has few phases (input at 48, 96, 256, 384 or
//! 768 kHz into 192 kHz), [`Resampler::set_rates`] works out the taps for
//! each phase once, and each output is then about half the work. See
//! [`Resampler::banked`].
//!
//! Equal rates pass the samples through unchanged, with no delay.

/// Kernel table points per frame of the lower rate. Interpolating linearly
/// between them makes errors at most 83 dB below the signal. The three tables
/// take 21 KB; 64 points would halve that, with errors up to 71 dB below.
pub const PHASES: usize = 128;

/// Most taps per output sample per channel.
pub const MAX_TAPS: usize = 64;

/// Room for the kernel's taps at every output phase, for ratios with few
/// phases (see [`Resampler::banked`]).
const BANK: usize = 256;

/// Kaiser window shape: about 68 dB of stopband attenuation.
const BETA: f64 = 6.5;
/// Cutoff as a fraction of the lower rate.
const CUTOFF: f64 = 0.45;

/// Positions in a table are in 1/65536 of a point.
const POSITION_ONE: f32 = 65_536.0;

/// Points in a table for a kernel that reaches `reach` frames each side: the
/// kernel to its end, then zeros, so the outermost tap's interpolation never
/// reads past the end (see [`Resampler::set_rates`]).
const fn table_len(reach: usize) -> usize {
    (reach + 2) * PHASES
}

/// One side of the kernel, from its centre: point `i` is the kernel at `i /
/// PHASES` input samples from the centre at 1:1, including the gain of 2 ×
/// CUTOFF that makes a low tone come out at its input level.
static KERNEL_16: [f32; table_len(16)] = kernel(16);
static KERNEL_12: [f32; table_len(12)] = kernel(12);
static KERNEL_8: [f32; table_len(8)] = kernel(8);

const fn kernel<const N: usize>(reach: usize) -> [f32; N] {
    let mut t = [0.0; N];
    let i0_beta = bessel_i0(BETA);
    let mut i = 0;
    while i < reach * PHASES {
        let v = i as f64 / PHASES as f64;
        let r = v / reach as f64;
        let window = bessel_i0(BETA * sqrt(1.0 - r * r)) / i0_beta;
        t[i] = (2.0 * CUTOFF * sinc(2.0 * CUTOFF * v) * window) as f32;
        i += 1;
    }
    t
}

/// sin(πx) / (πx), for x ≥ 0.
const fn sinc(x: f64) -> f64 {
    if x == 0.0 { 1.0 } else { sin_turns(0.5 * x) / (core::f64::consts::PI * x) }
}

/// Sine of an angle in turns, for angles from 0 to about 1000 turns. Splits
/// off whole quarter turns and uses Taylor series on the rest (±π/4).
const fn sin_turns(t: f64) -> f64 {
    let q = 4.0 * t;
    let n = (q + 0.5) as i64;
    let x = (q - n as f64) * core::f64::consts::FRAC_PI_2;
    let x2 = x * x;
    let mut s = 0.0;
    let mut c = 0.0;
    let (mut ts, mut tc) = (x, 1.0);
    let mut k = 1;
    while k <= 10 {
        s += ts;
        c += tc;
        ts *= -x2 / ((2 * k) * (2 * k + 1)) as f64;
        tc *= -x2 / ((2 * k - 1) * (2 * k)) as f64;
        k += 1;
    }
    match n & 3 {
        0 => s,
        1 => c,
        2 => -s,
        _ => -c,
    }
}

/// Modified Bessel function of the first kind, order 0, by its power series.
/// Within 1e-15 for x up to 10.
const fn bessel_i0(x: f64) -> f64 {
    let q = 0.25 * x * x;
    let mut sum = 1.0;
    let mut term = 1.0;
    let mut k = 1;
    while k <= 30 {
        term *= q / (k * k) as f64;
        sum += term;
        k += 1;
    }
    sum
}

/// Square root of `y` from 0 to 1 by Newton's method.
const fn sqrt(y: f64) -> f64 {
    if y <= 0.0 {
        return 0.0;
    }
    let mut x = 1.0;
    let mut k = 0;
    while k < 60 {
        x = 0.5 * (x + y / x);
        k += 1;
    }
    x
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// Stereo resampler from any input rate to a fixed output rate.
///
/// Output frame `j` is the input at time `j × in_rate / out_rate` input
/// frames, counted from the first frame after [`reset`](Self::reset), with
/// silence before that frame. So a file of `n` frames gives
/// [`out_frames(n)`](Self::out_frames) output frames. The last of them need
/// input from up to [`taps() / 2`](Self::taps) frames after the file: to play
/// the end of a file, feed that much silence after it and stop at
/// `out_frames(n)`.
///
/// About 2.1 KB, so it can sit in a static. Build it in place with
/// [`new`](Self::new), then call [`set_rates`](Self::set_rates).
#[derive(Clone, Debug)]
pub struct Resampler {
    /// The last [`MAX_TAPS`] input samples, written twice (at `write` and
    /// `write + MAX_TAPS`) so the newest `MAX_TAPS` are always in one run,
    /// oldest first, at `write..write + MAX_TAPS`.
    left: [f32; 2 * MAX_TAPS],
    right: [f32; 2 * MAX_TAPS],
    write: usize,
    /// Input frames received after the one at or before the next output's
    /// time: the next output is due when this reaches `half`. -1 after a
    /// reset, when the next output is at frame 0 and nothing has arrived.
    ahead: i32,
    /// Fraction of an input frame from the frame at or before the next
    /// output's time to that time, as `frac / den`.
    frac: u32,
    /// Input frames per output frame: `step_whole + step_frac / den`, the
    /// ratio of the rates reduced to lowest terms.
    step_whole: u32,
    step_frac: u32,
    den: u32,
    inv_den: f32,
    /// Taps on each side of the output's time.
    half: usize,
    kernel: &'static [f32],
    /// Table points per input frame: PHASES divided by the stretch.
    step: usize,
    /// 1 divided by the stretch, which keeps the kernel's gain at 1.
    gain: f32,
    /// Output frames per input frame.
    out_per_in: f32,
    pass_through: bool,
    /// Whether `bank` holds the taps for every value of `frac`.
    banked: bool,
    /// The kernel's taps for each value of `frac`, `2 × half` of them each:
    /// the side before the output's time nearest first, then the side after.
    bank: [f32; BANK],
}

impl Default for Resampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Resampler {
    /// A resampler with equal rates, passing samples through.
    pub const fn new() -> Self {
        Self {
            left: [0.0; 2 * MAX_TAPS],
            right: [0.0; 2 * MAX_TAPS],
            write: 0,
            ahead: -1,
            frac: 0,
            step_whole: 1,
            step_frac: 0,
            den: 1,
            inv_den: 1.0,
            half: 0,
            kernel: &KERNEL_16,
            step: PHASES,
            gain: 1.0,
            out_per_in: 1.0,
            pass_through: true,
            banked: false,
            bank: [0.0; BANK],
        }
    }

    /// Set the input and output rates in Hz, and [`reset`](Self::reset).
    /// Equal rates, or a rate of 0, pass samples through.
    pub fn set_rates(&mut self, in_rate: u32, out_rate: u32) {
        self.pass_through = in_rate == out_rate || in_rate == 0 || out_rate == 0;
        if !self.pass_through {
            let g = gcd(in_rate, out_rate);
            let (num, den) = (in_rate / g, out_rate / g);
            self.step_whole = num / den;
            self.step_frac = num % den;
            self.den = den;
            self.inv_den = 1.0 / den as f32;
            self.out_per_in = (f64::from(out_rate) / f64::from(in_rate)) as f32;

            // The kernel is stretched when downsampling, to cut off at 0.45
            // of the output rate. The stretch is rounded so that one input
            // frame is a whole number of table points: then every tap of an
            // output is the same fraction of a point past its point, which
            // saves work in the inner loop. This moves the cutoff by up to
            // 0.4% at 250 kHz, 1% at 500 kHz and 1.6% at 768 kHz.
            let stretch = (f64::from(in_rate) / f64::from(out_rate)).max(1.0);
            let (kernel, reach): (&'static [f32], usize) = if stretch <= 2.0 {
                (&KERNEL_16, 16)
            } else if stretch <= 8.0 / 3.0 {
                (&KERNEL_12, 12)
            } else {
                (&KERNEL_8, 8)
            };
            self.kernel = kernel;
            self.step = (libm::round(PHASES as f64 / stretch) as usize).max(1);
            // The outermost tap is less than `half` frames from the output's
            // time, which is less than one more frame past the kernel's end:
            // inside its table.
            self.half = (reach * PHASES).div_ceil(self.step).min(MAX_TAPS / 2);
            self.gain = self.step as f32 / PHASES as f32;

            let taps = 2 * self.half;
            self.banked = den as usize * taps <= BANK;
            if self.banked {
                for (frac, row) in self.bank.chunks_exact_mut(taps).take(den as usize).enumerate() {
                    let (before, after) = positions(frac as u32, self.inv_den, self.step);
                    let (past, future) = row.split_at_mut(self.half);
                    taps_at(self.kernel, before, self.step, past);
                    taps_at(self.kernel, after, self.step, future);
                }
            }
        }
        self.reset();
    }

    /// Whether the kernel's taps are worked out once for every phase of the
    /// output, which makes each output about half the work. That is when the
    /// output rate divided by the greatest common divisor of the two rates,
    /// times the taps, is at most 256: for 192 kHz output, input at 48, 96,
    /// 256, 384 or 768 kHz, but not 44.1, 250, 300 or 500 kHz.
    pub fn banked(&self) -> bool {
        !self.pass_through && self.banked
    }

    /// Clear the history: the next input frame is frame 0.
    pub fn reset(&mut self) {
        self.left.fill(0.0);
        self.right.fill(0.0);
        self.write = 0;
        self.ahead = -1;
        self.frac = 0;
    }

    /// Multiply-adds per output frame per channel: 0 when passing through.
    pub fn taps(&self) -> usize {
        if self.pass_through { 0 } else { 2 * self.half }
    }

    /// Output frames that cover `in_frames` input frames: those whose time is
    /// before the end of the input.
    pub fn out_frames(&self, in_frames: u64) -> u64 {
        if self.pass_through {
            return in_frames;
        }
        // Output frames per input frame is den / (step_whole × den + step_frac).
        let num = u64::from(self.step_whole) * u64::from(self.den) + u64::from(self.step_frac);
        (in_frames * u64::from(self.den)).div_ceil(num)
    }

    /// Output frames of delay: the input frames taken but not yet heard, in
    /// output frames. The next output frame is the input at the time of the
    /// frames taken so far less this (in output frames). 0 when passing
    /// through.
    pub fn latency_out(&self) -> f32 {
        if self.pass_through {
            return 0.0;
        }
        ((self.ahead + 1) as f32 - self.frac as f32 * self.inv_den) * self.out_per_in
    }

    /// Resample from `in_l` and `in_r` into `out_l` and `out_r`, until the
    /// input is used up or the output is full. Returns (input frames taken,
    /// output frames written). The output is the same however the input and
    /// output are split into calls.
    pub fn process(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) -> (usize, usize) {
        let n_in = in_l.len().min(in_r.len());
        let n_out = out_l.len().min(out_r.len());
        if self.pass_through {
            let n = n_in.min(n_out);
            out_l[..n].copy_from_slice(&in_l[..n]);
            out_r[..n].copy_from_slice(&in_r[..n]);
            return (n, n);
        }
        let half = self.half as i32;
        let (mut taken, mut written) = (0, 0);
        loop {
            while self.ahead < half {
                if taken == n_in {
                    return (taken, written);
                }
                self.push(in_l[taken], in_r[taken]);
                taken += 1;
            }
            if written == n_out {
                return (taken, written);
            }
            let (l, r) = self.convolve();
            out_l[written] = l;
            out_r[written] = r;
            written += 1;
            self.advance();
        }
    }

    #[inline]
    fn push(&mut self, l: f32, r: f32) {
        let w = self.write;
        self.left[w] = l;
        self.left[w + MAX_TAPS] = l;
        self.right[w] = r;
        self.right[w + MAX_TAPS] = r;
        self.write = if w + 1 == MAX_TAPS { 0 } else { w + 1 };
        self.ahead += 1;
    }

    /// Move the next output's time on by one output frame.
    #[inline]
    fn advance(&mut self) {
        let mut frames = self.step_whole;
        self.frac += self.step_frac;
        if self.frac >= self.den {
            self.frac -= self.den;
            frames += 1;
        }
        self.ahead -= frames as i32;
    }

    /// The output at the next output's time. The newest input frame is
    /// `half` frames after the one at or before that time.
    ///
    /// The banked and table versions are separate functions, not inlined into
    /// `process`, so the compiler can give each inner loop all the registers.
    /// Inlined, the table version ran about 20% slower on a PC.
    #[inline]
    fn convolve(&self) -> (f32, f32) {
        if self.banked { self.convolve_banked() } else { self.convolve_table() }
    }

    /// The input frames each side of the output's time, nearest first.
    #[inline(always)]
    fn sides(&self) -> (impl ExactSizeIterator<Item = (&f32, &f32)>, impl ExactSizeIterator<Item = (&f32, &f32)>) {
        let h = self.half;
        let start = self.write + MAX_TAPS - 2 * h;
        let (l_past, l_future) = self.left[start..start + 2 * h].split_at(h);
        let (r_past, r_future) = self.right[start..start + 2 * h].split_at(h);
        (l_past.iter().rev().zip(r_past.iter().rev()), l_future.iter().zip(r_future))
    }

    #[inline(never)]
    fn convolve_banked(&self) -> (f32, f32) {
        let (past, future) = self.sides();
        let h = self.half;
        let (k_past, k_future) = self.bank[self.frac as usize * 2 * h..][..2 * h].split_at(h);
        let ((pl, pr), (fl, fr)) = (dot(k_past, past), dot(k_future, future));
        ((pl + fl) * self.gain, (pr + fr) * self.gain)
    }

    #[inline(never)]
    fn convolve_table(&self) -> (f32, f32) {
        let (past, future) = self.sides();
        let (before, after) = positions(self.frac, self.inv_den, self.step);
        let (pl, pr) = wing(self.kernel, past, before, self.step);
        let (fl, fr) = wing(self.kernel, future, after, self.step);
        ((pl + fl) * self.gain, (pr + fr) * self.gain)
    }
}

/// Table positions, in 1/65536 of a point, of the nearest input frame on each
/// side of an output whose time is `frac × inv_den` frames after the frame at
/// or before it: that frame, then the one after it.
#[inline]
fn positions(frac: u32, inv_den: f32, step: usize) -> (u32, u32) {
    let one = (step as f32 * POSITION_ONE) as u32;
    let before = (frac as f32 * inv_den * one as f32) as u32;
    (before, one - before)
}

/// The kernel at table position `position` (in 1/65536 of a point), `step`
/// points further on, and so on, into `taps`. The same values [`wing`] uses.
fn taps_at(kernel: &[f32], position: u32, step: usize, taps: &mut [f32]) {
    let i = (position >> 16) as usize;
    let f = (position & 0xFFFF) as f32 * (1.0 / POSITION_ONE);
    for (j, k) in taps.iter_mut().enumerate() {
        let (a, b) = (kernel[i + j * step], kernel[i + j * step + 1]);
        *k = a + (b - a) * f;
    }
}

/// Sum of the samples weighted by `taps`, for both channels, in the same
/// order as [`wing`].
#[inline(always)]
fn dot<'a>(taps: &[f32], samples: impl Iterator<Item = (&'a f32, &'a f32)>) -> (f32, f32) {
    let (mut sum_l, mut sum_r) = (0.0, 0.0);
    for (&k, (&l, &r)) in taps.iter().zip(samples) {
        sum_l += k * l;
        sum_r += k * r;
    }
    (sum_l, sum_r)
}

/// Sum of the samples weighted by the kernel, for both channels: the first
/// sample by the kernel at table position `position` (in 1/65536 of a point),
/// the next `step` points further on, and so on. Every tap is the same
/// fraction of a point past its point.
#[inline(always)]
fn wing<'a>(
    kernel: &[f32],
    samples: impl ExactSizeIterator<Item = (&'a f32, &'a f32)>,
    position: u32,
    step: usize,
) -> (f32, f32) {
    let i = (position >> 16) as usize;
    let f = (position & 0xFFFF) as f32 * (1.0 / POSITION_ONE);
    debug_assert!(i + (samples.len() - 1) * step + 1 < kernel.len());
    let points = kernel[i..].iter().step_by(step).zip(kernel[i + 1..].iter().step_by(step));
    let (mut sum_l, mut sum_r) = (0.0, 0.0);
    for ((&l, &r), (&a, &b)) in samples.zip(points) {
        let k = a + (b - a) * f;
        sum_l += k * l;
        sum_r += k * r;
    }
    (sum_l, sum_r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{noise, rms, sine, zero_crossing_freq};
    use std::vec::Vec;

    const OUT: u32 = 192_000;

    fn resampler(in_rate: u32, out_rate: u32) -> Resampler {
        let mut r = Resampler::new();
        r.set_rates(in_rate, out_rate);
        r
    }

    /// Run `l` and `r` through `rs`, taking input and output blocks in turn
    /// from the two lists, then drain what the output had no room for.
    fn run_stereo(
        rs: &mut Resampler,
        l: &[f32],
        r: &[f32],
        in_blocks: &[usize],
        out_blocks: &[usize],
    ) -> [Vec<f32>; 2] {
        let (mut yl, mut yr) = (Vec::new(), Vec::new());
        let mut buf = [std::vec![0.0; 5000], std::vec![0.0; 5000]];
        let (mut i, mut k) = (0, 0);
        loop {
            let end = (i + in_blocks[k % in_blocks.len()]).min(l.len());
            let ob = out_blocks[k % out_blocks.len()];
            k += 1;
            let [bl, br] = &mut buf;
            let (taken, written) = rs.process(&l[i..end], &r[i..end], &mut bl[..ob], &mut br[..ob]);
            i += taken;
            yl.extend_from_slice(&bl[..written]);
            yr.extend_from_slice(&br[..written]);
            if i == l.len() && written < ob {
                return [yl, yr];
            }
        }
    }

    fn run(rs: &mut Resampler, x: &[f32]) -> Vec<f32> {
        let [l, r] = run_stereo(rs, x, x, &[1000], &[1000]);
        assert_eq!(l, r);
        l
    }

    /// Amplitude of the tone at `freq` in `x`, and the RMS of what is left
    /// when it is taken out, by least squares.
    pub(super) fn fit_tone(x: &[f32], fs: f64, freq: f64) -> (f64, f64) {
        let w = core::f64::consts::TAU * freq / fs;
        let (mut cc, mut ss, mut cs, mut xc, mut xs) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for (n, &v) in x.iter().enumerate() {
            let (s, c) = (w * n as f64).sin_cos();
            cc += c * c;
            ss += s * s;
            cs += c * s;
            xc += v as f64 * c;
            xs += v as f64 * s;
        }
        let det = cc * ss - cs * cs;
        let a = (xc * ss - xs * cs) / det;
        let b = (xs * cc - xc * cs) / det;
        let resid: f64 = x
            .iter()
            .enumerate()
            .map(|(n, &v)| {
                let (s, c) = (w * n as f64).sin_cos();
                (v as f64 - a * c - b * s).powi(2)
            })
            .sum();
        ((a * a + b * b).sqrt(), (resid / x.len() as f64).sqrt())
    }

    fn db(r: f64) -> f64 {
        20.0 * r.log10()
    }

    /// The output once the kernel no longer reaches back before the input:
    /// after 16 input frames at 8 kHz, 384 output frames.
    fn steady(y: &[f32]) -> &[f32] {
        &y[500..y.len() - 100]
    }

    /// Rates for which the kernel reaches 16 frames each side, which the quality
    /// targets apply to.
    const RATES: [u32; 9] = [44_100, 48_000, 96_000, 125_000, 250_000, 256_000, 300_000, 352_800, 384_000];

    #[test]
    fn equal_rates_pass_through_exactly() {
        let x = noise(1.0, 10_000, 3);
        let x2 = noise(1.0, 10_000, 4);
        let mut rs = resampler(OUT, OUT);
        let [l, r] = run_stereo(&mut rs, &x, &x2, &[1, 77, 4096], &[300, 5, 1]);
        assert_eq!((l, r), (x.clone(), x2.clone()));
        assert_eq!((rs.latency_out(), rs.taps(), rs.out_frames(12_345)), (0.0, 0, 12_345));

        // Back to equal rates after resampling.
        rs.set_rates(384_000, OUT);
        run(&mut rs, &x);
        assert!(rs.latency_out() > 0.0);
        rs.set_rates(OUT, OUT);
        assert_eq!(rs.latency_out(), 0.0);
        assert_eq!(run(&mut rs, &x), x);
        assert_eq!(Resampler::new().process(&x, &x, &mut [0.0; 3], &mut [0.0; 2]), (2, 2));
    }

    #[test]
    fn passband_is_flat_and_clean() {
        for in_rate in RATES {
            let mut rs = resampler(in_rate, OUT);
            let lower = in_rate.min(OUT) as f64;
            let mut f = 0.02 * lower;
            while f <= 0.4 * lower + 1.0 {
                let x = sine(in_rate as f32, f as f32, 0.5, in_rate as usize / 20);
                rs.reset();
                let y = run(&mut rs, &x);
                let (amp, resid) = fit_tone(steady(&y), OUT as f64, f);
                let gain = db(amp / 0.5);
                let noise = db(resid / 0.5);
                assert!(gain.abs() < 0.2, "{in_rate} Hz in, tone at {f:.0} Hz: gain {gain:.3} dB");
                assert!(noise < -60.0, "{in_rate} Hz in, tone at {f:.0} Hz: residual {noise:.1} dB");
                f += 0.0237 * lower;
            }
        }
    }

    /// Where a tone at `f` lands after sampling at `fs`.
    fn folded(f: f64, fs: f64) -> f64 {
        let r = f % fs;
        if r > fs / 2.0 { fs - r } else { r }
    }

    #[test]
    fn downsampling_removes_sound_above_the_output_nyquist() {
        // The kernel's stopband: at least 60 dB down from 0.52 of the output
        // rate (99.8 kHz) with a reach of 16 frames.
        for in_rate in [250_000u32, 256_000, 300_000, 384_000] {
            let mut rs = resampler(in_rate, OUT);
            let mut f = 100_000.0;
            while f < 0.5 * in_rate as f64 {
                let x = sine(in_rate as f32, f as f32, 0.5, in_rate as usize / 20);
                rs.reset();
                let y = run(&mut rs, &x);
                let alias = folded(f, OUT as f64);
                let (amp, _) = fit_tone(steady(&y), OUT as f64, alias);
                let level = db(amp / 0.5);
                let total = db(rms(steady(&y)) as f64 / rms(&x) as f64);
                assert!(level < -60.0, "{in_rate} Hz in, {f:.0} Hz: alias at {alias:.0} Hz is {level:.1} dB");
                assert!(total < -60.0, "{in_rate} Hz in, {f:.0} Hz: output {total:.1} dB");
                f += 4_321.0;
            }
        }
    }

    #[test]
    fn upsampling_leaves_no_images() {
        // Images of a tone at f fall at the input rate less f, and above.
        for (in_rate, f) in [(96_000u32, 38_000.0f32), (48_000, 19_000.0), (44_100, 17_000.0), (8_000, 3_000.0)] {
            let x = sine(in_rate as f32, f, 0.5, in_rate as usize / 10);
            let y = run(&mut resampler(in_rate, OUT), &x);
            let (amp, resid) = fit_tone(steady(&y), OUT as f64, f as f64);
            assert!(db(amp / 0.5).abs() < 0.2);
            assert!(db(resid / 0.5) < -60.0, "{in_rate} Hz in, {f} Hz: residual {:.1} dB", db(resid / 0.5));
        }
    }

    #[test]
    fn tones_keep_their_frequency() {
        for (in_rate, f) in
            [(96_000u32, 40_000.0f32), (250_000, 70_000.0), (384_000, 75_000.0), (48_000, 18_000.0), (44_100, 15_000.0)]
        {
            let x = sine(in_rate as f32, f, 0.5, in_rate as usize / 5);
            let y = run(&mut resampler(in_rate, OUT), &x);
            let got = zero_crossing_freq(steady(&y), OUT as f32);
            assert!((got - f).abs() < f * 1e-4, "{in_rate} Hz in: {f} Hz came out at {got} Hz");
        }
    }

    #[test]
    fn output_frame_j_is_the_input_at_time_j_in_over_out() {
        for in_rate in [44_100u32, 96_000, 250_000, 384_000, 500_000] {
            let f = 0.3 * in_rate.min(OUT) as f32;
            let x = sine(in_rate as f32, f, 0.5, 20_000);
            let mut rs = resampler(in_rate, OUT);
            let y = run(&mut rs, &x);
            // The same tone sampled at the output rate from time 0.
            let expect = sine(OUT as f32, f, 0.5, y.len());
            let diff: Vec<f32> = y.iter().zip(&expect).map(|(a, b)| a - b).collect();
            let error = db(rms(steady(&diff)) as f64 / 0.5);
            assert!(error < -45.0, "{in_rate} Hz in: error {error:.1} dB");

            // What was taken but not yet heard.
            let lag = (x.len() as f64 - y.len() as f64 * in_rate as f64 / OUT as f64) * OUT as f64 / in_rate as f64;
            assert!((rs.latency_out() as f64 - lag).abs() < 1e-3, "{in_rate}: {} vs {lag}", rs.latency_out());
            assert!(lag > 0.0 && lag <= rs.taps() as f64 * OUT as f64 / in_rate as f64);
        }
    }

    #[test]
    fn the_end_of_a_file_comes_out_after_silence() {
        for in_rate in [44_100u32, 250_000, 768_000] {
            let mut rs = resampler(in_rate, OUT);
            let mut x = noise(0.5, 10_001, 9);
            let n = x.len() as u64;
            x.extend(std::vec![0.0; rs.taps() / 2]);
            let y = run(&mut rs, &x);
            assert!(y.len() as u64 >= rs.out_frames(n), "{in_rate}: {} of {}", y.len(), rs.out_frames(n));
        }
    }

    #[test]
    fn chunk_sizes_do_not_change_the_output() {
        let l = noise(0.8, 30_000, 17);
        let r = sine(250_000.0, 51_234.0, 0.7, 30_000);
        for in_rate in [44_100u32, 96_000, 250_000, 384_000, 500_000, 768_000] {
            let mut rs = resampler(in_rate, OUT);
            let whole = run_stereo(&mut rs, &l, &r, &[30_000], &[5000]);
            assert!(whole[0].len() > 7000);
            for (ins, outs) in [
                (&[1usize][..], &[5000usize][..]),
                (&[5000], &[1]),
                (&[7, 300, 1, 64, 4093], &[3, 1, 500, 17, 4999]),
                (&[333], &[2, 1]),
            ] {
                rs.reset();
                assert!(run_stereo(&mut rs, &l, &r, ins, outs) == whole, "{in_rate} Hz in, blocks {ins:?} {outs:?}");
            }
        }
    }

    #[test]
    fn silence_gives_silence_and_full_scale_stays_finite() {
        for in_rate in [8_000u32, 44_100, 96_000, 250_000, 384_000, 500_000, 768_000, 1_000_000] {
            let mut rs = resampler(in_rate, OUT);
            assert!(run(&mut rs, &[0.0; 5000]).iter().all(|&v| v == 0.0));
            let square: Vec<f32> = (0..5000).map(|i| if (i / 8) % 2 == 0 { 1.0 } else { -1.0 }).collect();
            let nyquist: Vec<f32> = (0..5000).map(|i| if i % 2 == 0 { 1.0 } else { -1.0 }).collect();
            let full = noise(1.0, 5000, 5);
            for x in [&square, &nyquist, &full] {
                rs.reset();
                let y = run(&mut rs, x);
                let peak = y.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                assert!(y.iter().all(|v| v.is_finite()) && peak < 2.0, "{in_rate} Hz in: peak {peak}");
            }
        }
    }

    #[test]
    fn any_rate_runs_with_the_kernel_inside_its_table() {
        // Indexing past a table panics; every phase is visited at rates
        // whose ratio has a large denominator.
        let x = noise(1.0, 20_000, 23);
        let mut rates: Vec<u32> = (0..60).map(|k| 8_000 + k * 16_661).collect();
        rates.extend([
            44_100, 47_999, 191_999, 192_001, 383_999, 384_001, 511_999, 512_001, 767_999, 768_000, 3_000_000,
        ]);
        for in_rate in rates {
            let mut rs = resampler(in_rate, OUT);
            assert!(rs.taps() <= MAX_TAPS);
            assert!(run(&mut rs, &x).iter().all(|v| v.is_finite()), "{in_rate}");
        }
    }

    #[test]
    fn taps_and_size() {
        let taps = |r| resampler(r, OUT).taps();
        assert_eq!(
            [44_100, 96_000, 192_000, 250_000, 256_000, 300_000, 384_000, 441_000, 500_000, 768_000, 1_000_000]
                .map(taps),
            [32, 32, 0, 42, 44, 50, 64, 56, 64, 64, 64]
        );
        assert!(core::mem::size_of::<Resampler>() <= 2_200, "{}", core::mem::size_of::<Resampler>());
        let banked = |r| resampler(r, OUT).banked();
        assert_eq!(
            [48_000, 96_000, 256_000, 384_000, 768_000, 44_100, 192_000, 250_000, 300_000, 500_000].map(banked),
            [true, true, true, true, true, false, false, false, false, false]
        );
    }

    #[test]
    fn banked_taps_give_the_same_output_as_the_table() {
        let l = noise(0.8, 20_000, 31);
        let r = sine(96_000.0, 31_234.0, 0.7, 20_000);
        for in_rate in [32_000u32, 48_000, 96_000, 128_000, 256_000, 384_000, 768_000] {
            let mut rs = resampler(in_rate, OUT);
            assert!(rs.banked(), "{in_rate}");
            let banked = run_stereo(&mut rs, &l, &r, &[777], &[555]);
            rs.reset();
            rs.banked = false;
            assert!(run_stereo(&mut rs, &l, &r, &[777], &[555]) == banked, "{in_rate}");
        }
    }
}

/// Figures for comparing changes to the resampler, not pass or fail tests.
/// Run with `cargo test -p bat-dsp --release resample::measurements --
/// --ignored --nocapture`. For timings closer to the X8's, which has no SIMD,
/// add `RUSTFLAGS="-C no-vectorize-loops -C no-vectorize-slp"`.
#[cfg(test)]
mod measurements {
    use super::tests::fit_tone;
    use super::*;
    use crate::test_util::{rms, sine};
    use std::time::Instant;

    const OUT: u32 = 192_000;

    fn run(rs: &mut Resampler, x: &[f32]) -> std::vec::Vec<f32> {
        let mut y = std::vec::Vec::new();
        let mut buf = [0.0; 1024];
        let mut buf_r = [0.0; 1024];
        let mut i = 0;
        loop {
            let (t, w) = rs.process(&x[i..], &x[i..], &mut buf, &mut buf_r);
            i += t;
            y.extend_from_slice(&buf[..w]);
            if w < buf.len() {
                return y;
            }
        }
    }

    #[test]
    #[ignore]
    /// For each input rate: the taps, the worst level error of tones up to
    /// 0.4 of the lower rate, the worst residual (images and table error)
    /// with those tones, and the loudest sound left of tones from 0.52 of the
    /// output rate (99.8 kHz) up to the input's Nyquist frequency.
    fn quality_figures() {
        let db = |r: f64| 20.0 * r.log10();
        for in_rate in [8_000u32, 44_100, 48_000, 96_000, 250_000, 256_000, 300_000, 384_000, 441_000, 500_000, 768_000]
        {
            let mut rs = Resampler::new();
            rs.set_rates(in_rate, OUT);
            let lower = in_rate.min(OUT) as f64;
            let (mut worst_gain, mut worst_resid) = (0.0f64, f64::NEG_INFINITY);
            let mut f = 0.01 * lower;
            while f <= 0.4 * lower {
                let x = sine(in_rate as f32, f as f32, 0.5, in_rate as usize / 20);
                rs.reset();
                let y = run(&mut rs, &x);
                let (amp, resid) = fit_tone(&y[500..y.len() - 100], OUT as f64, f);
                if db(amp / 0.5).abs() > worst_gain.abs() {
                    worst_gain = db(amp / 0.5);
                }
                worst_resid = worst_resid.max(db(resid / 0.5));
                f += 0.0113 * lower;
            }
            let level_at = |f: f64, rs: &mut Resampler| {
                let x = sine(in_rate as f32, f as f32, 0.5, in_rate as usize / 20);
                rs.reset();
                let y = run(rs, &x);
                db(rms(&y[500..y.len() - 100]) as f64 / rms(&x) as f64)
            };
            std::print!(
                "{in_rate} Hz: {} taps, passband worst {worst_gain:+.3} dB, worst residual {worst_resid:.1} dB",
                rs.taps()
            );
            if in_rate > OUT {
                // The highest tone left louder than -60 dB, and the loudest
                // from 0.52 of the output rate up.
                let (mut edge, mut stop) = (0.0, f64::NEG_INFINITY);
                let mut f = 0.5 * OUT as f64;
                while f < 0.5 * in_rate as f64 {
                    let level = level_at(f, &mut rs);
                    if level > -60.0 {
                        edge = f;
                    }
                    if f >= 0.52 * OUT as f64 {
                        stop = stop.max(level);
                    }
                    f += 677.0;
                }
                let level_04 = level_at(0.4 * OUT as f64, &mut rs);
                std::print!(
                    ", 76.8 kHz {level_04:+.2} dB, -60 dB from {:.1} kHz, loudest above 99.8 kHz {stop:.1} dB",
                    edge / 1000.0
                );
            }
            std::println!();
        }
    }

    #[test]
    #[ignore]
    /// Milliseconds per second of 192 kHz stereo output.
    fn time_per_second() {
        for in_rate in [44_100u32, 48_000, 96_000, 250_000, 256_000, 384_000, 500_000, 768_000] {
            let mut rs = Resampler::new();
            rs.set_rates(in_rate, OUT);
            let seconds = 5;
            let x: std::vec::Vec<f32> =
                (0..in_rate as usize * seconds).map(|i| ((i * 7919) % 1000) as f32 / 1000.0 - 0.5).collect();
            let (mut yl, mut yr) = ([0.0; 256], [0.0; 256]);
            let t = Instant::now();
            let mut i = 0;
            let mut out = 0;
            while i < x.len() {
                let (taken, written) = rs.process(&x[i..], &x[i..], &mut yl, &mut yr);
                std::hint::black_box((&yl, &yr));
                i += taken;
                out += written;
            }
            let ms = t.elapsed().as_secs_f64() * 1000.0 / (out as f64 / OUT as f64);
            let banked = if rs.banked() { ", taps worked out in advance" } else { "" };
            std::println!("{in_rate} Hz in, {} taps{banked}: {ms:.2} ms per second of output", rs.taps());
        }
        let mut rs = Resampler::new();
        rs.set_rates(OUT, OUT);
        std::println!("size_of::<Resampler>() = {} bytes", core::mem::size_of::<Resampler>());
    }
}
