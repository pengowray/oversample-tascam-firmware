//! Spectrogram columns from a sliding FFT.
//!
//! [`SpectrumAnalyzer`] keeps the last `N` samples, and every `hop` samples
//! applies a Hann window, runs a real FFT and converts each bin to power in
//! dB. The result, a [`Column`], feeds a waterfall display and the call
//! detector. `N` is 256, 512 or 1024, fixed at compile time.
//!
//! Levels are relative to a full-scale sine: a sine of amplitude 1 centred on
//! a bin reads 0 dB in that bin. A DC offset of 1, and a signal alternating
//! between 1 and -1 every sample (the Nyquist frequency), also read 0 dB.
//!
//! At 192 kHz, `N = 512` gives 375 Hz bins and a 2.7 ms window.

use core::f32::consts::TAU;

use microfft::real::{rfft_256, rfft_512, rfft_1024};

/// Level of a bin with no signal (digital silence), in dB.
pub const MIN_DB: f32 = -200.0;

/// Power that reads [`MIN_DB`].
const MIN_POWER: f32 = 1e-20;

/// 10 · log10(2): dB per doubling of power.
const DB_PER_OCTAVE: f32 = 3.010_3;

mod sealed {
    pub trait Sealed {}
}

/// Frame sizes the analyzer supports: `[f32; 256]`, `[f32; 512]` and
/// `[f32; 1024]`.
pub trait FftFrame: sealed::Sealed {
    /// One value per bin: `[f32; N / 2 + 1]`.
    type Bins: Copy + core::fmt::Debug + AsRef<[f32]> + AsMut<[f32]>;

    /// `Bins` with every value set to [`MIN_DB`].
    const SILENT_BINS: Self::Bins;

    /// Real FFT in place. Afterwards the frame holds `(re, im)` pairs for
    /// bins `0..N / 2`, except that the second value is the real part of the
    /// Nyquist bin instead of the (zero) imaginary part of bin 0.
    fn rfft_in_place(&mut self);
}

macro_rules! fft_frame {
    ($n:literal, $rfft:ident) => {
        impl sealed::Sealed for [f32; $n] {}

        impl FftFrame for [f32; $n] {
            type Bins = [f32; $n / 2 + 1];

            const SILENT_BINS: Self::Bins = [MIN_DB; $n / 2 + 1];

            #[inline]
            fn rfft_in_place(&mut self) {
                // microfft transforms in place; the returned slice is a view of `self`.
                let _ = $rfft(self);
            }
        }
    };
}

fft_frame!(256, rfft_256);
fft_frame!(512, rfft_512);
fft_frame!(1024, rfft_1024);

/// Power to dB: `10 · log10(power)`, within 0.002 dB, and [`MIN_DB`] for
/// powers at or below 1e-20 (including zero and NaN).
///
/// Uses the float's exponent and a cubic for the mantissa, which costs a few
/// multiplies instead of a `log10f`.
#[inline]
pub fn power_to_db(power: f32) -> f32 {
    let bits = power.max(MIN_POWER).to_bits();
    let exponent = (bits >> 23) as i32 - 127;
    let m = f32::from_bits((bits & 0x007f_ffff) | 0x3f80_0000);
    // log2(m) for m in [1, 2): near-minimax cubic, error under 6.4e-4.
    let log2_m = ((0.158_245_2 * m - 1.051_847_3) * m + 3.047_825_5) * m - 2.153_584_2;
    DB_PER_OCTAVE * (exponent as f32 + log2_m)
}

/// One spectrogram column: power per bin for one windowed frame.
#[derive(Clone, Copy, Debug)]
pub struct Column<'a> {
    /// Index of the sample at the centre of the frame, counted from the first
    /// sample pushed after construction or [`SpectrumAnalyzer::reset`].
    pub centre_sample: u64,
    /// Bin spacing in Hz: sample rate / N.
    pub bin_hz: f32,
    /// Power in dB for bins `0..=N / 2`. Bin `k` is centred on `k · bin_hz`.
    pub db: &'a [f32],
}

impl Column<'_> {
    /// Centre frequency of a bin, in Hz.
    pub fn bin_freq(&self, bin: usize) -> f32 {
        bin as f32 * self.bin_hz
    }

    /// The bin nearest to a frequency, clamped to `0..=N / 2`.
    pub fn freq_bin(&self, hz: f32) -> usize {
        nearest_bin(hz, self.bin_hz, self.db.len() - 1)
    }

    /// Fill `out` with display pixels covering `from_hz` to `to_hz`.
    ///
    /// `out[0]` is at the `from_hz` end. Pass `to_hz` below `from_hz` to put
    /// high frequencies first. Each pixel takes the maximum of the bins
    /// centred inside it, so a narrow call still shows when many bins share a
    /// pixel. A pixel with no bin centre inside it (more pixels than bins)
    /// takes the value interpolated at its centre.
    pub fn to_pixels(&self, from_hz: f32, to_hz: f32, out: &mut [f32]) {
        let n = out.len();
        if n == 0 || self.db.is_empty() {
            return;
        }
        let last = self.db.len() - 1;
        let to_bins = 1.0 / self.bin_hz;
        let step = (to_hz - from_hz) / n as f32;
        // Pixel i spans edge(i) to edge(i + 1), in bins.
        let edge = |i: usize| (from_hz + step * i as f32) * to_bins;
        let mut a = edge(0);
        for (i, px) in out.iter_mut().enumerate() {
            let b = edge(i + 1);
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            // Bins centred in [lo, hi).
            let first = ceil_index(lo).min(last + 1);
            let end = ceil_index(hi).min(last + 1);
            *px = if first < end {
                self.db[first..end].iter().fold(f32::MIN, |m, &v| m.max(v))
            } else {
                self.interpolate(0.5 * (lo + hi), last)
            };
            a = b;
        }
    }

    /// Value at a fractional bin position, linear between bins.
    fn interpolate(&self, pos: f32, last: usize) -> f32 {
        if pos <= 0.0 {
            return self.db[0];
        }
        let k = pos as usize;
        if k >= last {
            return self.db[last];
        }
        let t = pos - k as f32;
        self.db[k] + t * (self.db[k + 1] - self.db[k])
    }
}

/// Smallest index at or above `x`; 0 for negative `x`.
fn ceil_index(x: f32) -> usize {
    if x <= 0.0 { 0 } else { libm::ceilf(x) as usize }
}

fn nearest_bin(hz: f32, bin_hz: f32, last: usize) -> usize {
    // `as usize` saturates: negative and NaN give 0.
    ((hz / bin_hz + 0.5) as usize).min(last)
}

/// Sliding-window spectrum analyzer with an `N`-point FFT (256, 512 or 1024).
#[derive(Clone, Debug)]
pub struct SpectrumAnalyzer<const N: usize>
where
    [f32; N]: FftFrame,
{
    sample_rate: f32,
    hop: usize,
    /// Periodic Hann window, scaled so a full-scale sine centred on a bin
    /// gives a magnitude of 1.
    window: [f32; N],
    /// The last `N` samples. Once full, `pos` is the oldest.
    ring: [f32; N],
    /// Where the next sample goes in `ring`.
    pos: usize,
    /// Samples still needed before the next column.
    until_column: usize,
    /// Samples pushed since construction or reset.
    pushed: u64,
    /// FFT working buffer.
    frame: [f32; N],
    db: <[f32; N] as FftFrame>::Bins,
}

impl<const N: usize> SpectrumAnalyzer<N>
where
    [f32; N]: FftFrame,
{
    /// Analyzer with the default hop of `N / 4` samples.
    pub fn new(sample_rate: f32) -> Self {
        let mut window = [0.0; N];
        let mut sum = 0.0;
        for (i, w) in window.iter_mut().enumerate() {
            *w = 0.5 - 0.5 * libm::cosf(TAU * i as f32 / N as f32);
            sum += *w;
        }
        // A sine of amplitude A centred on a bin gives |X| = A · sum / 2.
        let scale = 2.0 / sum;
        for w in &mut window {
            *w *= scale;
        }
        Self {
            sample_rate,
            hop: N / 4,
            window,
            ring: [0.0; N],
            pos: 0,
            until_column: N,
            pushed: 0,
            frame: [0.0; N],
            db: <[f32; N] as FftFrame>::SILENT_BINS,
        }
    }

    /// FFT size `N`.
    pub const fn fft_size(&self) -> usize {
        N
    }

    /// Number of bins in each column: `N / 2 + 1`.
    pub const fn bins(&self) -> usize {
        N / 2 + 1
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Samples between columns, clamped to `1..=N`. The column already being
    /// filled keeps the old spacing; the ones after it use the new one.
    pub fn set_hop(&mut self, hop: usize) {
        self.hop = hop.clamp(1, N);
    }

    pub fn hop(&self) -> usize {
        self.hop
    }

    /// Bin spacing in Hz: sample rate / N.
    pub fn bin_hz(&self) -> f32 {
        self.sample_rate / N as f32
    }

    /// Centre frequency of a bin, in Hz.
    pub fn bin_freq(&self, bin: usize) -> f32 {
        bin as f32 * self.bin_hz()
    }

    /// The bin nearest to a frequency, clamped to `0..=N / 2`.
    pub fn freq_bin(&self, hz: f32) -> usize {
        nearest_bin(hz, self.bin_hz(), N / 2)
    }

    /// Clear the sample history and restart the sample count at 0.
    pub fn reset(&mut self) {
        self.ring = [0.0; N];
        self.pos = 0;
        self.until_column = N;
        self.pushed = 0;
    }

    /// Add samples, in a block of any size. Calls `on_column` for each frame
    /// completed by these samples, in order, before returning.
    ///
    /// The first column comes once `N` samples have arrived, then one every
    /// `hop` samples.
    pub fn push(&mut self, mut input: &[f32], mut on_column: impl FnMut(&Column<'_>)) {
        while !input.is_empty() {
            let take = self.until_column.min(input.len());
            let (now, rest) = input.split_at(take);
            self.write_ring(now);
            self.pushed += take as u64;
            self.until_column -= take;
            input = rest;
            if self.until_column == 0 {
                self.until_column = self.hop;
                self.compute_column();
                on_column(&Column {
                    centre_sample: self.pushed - (N / 2) as u64,
                    bin_hz: self.bin_hz(),
                    db: self.db.as_ref(),
                });
            }
        }
    }

    /// Copy up to `N` samples into the ring.
    fn write_ring(&mut self, x: &[f32]) {
        let first = (N - self.pos).min(x.len());
        self.ring[self.pos..self.pos + first].copy_from_slice(&x[..first]);
        let wrapped = x.len() - first;
        self.ring[..wrapped].copy_from_slice(&x[first..]);
        self.pos = (self.pos + x.len()) % N;
    }

    fn compute_column(&mut self) {
        // Oldest samples are ring[pos..], newest ring[..pos].
        let older = N - self.pos;
        let (newest, oldest) = self.ring.split_at(self.pos);
        let (w_old, w_new) = self.window.split_at(older);
        let (f_old, f_new) = self.frame.split_at_mut(older);
        for ((f, &x), &w) in f_old.iter_mut().zip(oldest).zip(w_old) {
            *f = x * w;
        }
        for ((f, &x), &w) in f_new.iter_mut().zip(newest).zip(w_new) {
            *f = x * w;
        }

        self.frame.rfft_in_place();

        let bins = self.db.as_mut();
        let (dc, nyquist) = (self.frame[0], self.frame[1]);
        // DC and Nyquist have no negative-frequency twin, so they get a
        // quarter of the power scale of the other bins.
        bins[0] = power_to_db(0.25 * dc * dc);
        bins[N / 2] = power_to_db(0.25 * nyquist * nyquist);
        let (pairs, _) = self.frame[2..].as_chunks::<2>();
        for (b, &[re, im]) in bins[1..N / 2].iter_mut().zip(pairs) {
            *b = power_to_db(re * re + im * im);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::sine;
    use std::vec::Vec;

    const FS: f32 = 192_000.0;

    /// Every column from `x`, pushed in blocks of `block`, as (centre, dB).
    fn columns<const N: usize>(
        an: &mut SpectrumAnalyzer<N>,
        x: &[f32],
        block: usize,
    ) -> Vec<(u64, Vec<f32>)>
    where
        [f32; N]: FftFrame,
    {
        let mut out = Vec::new();
        for b in x.chunks(block) {
            an.push(b, |c| out.push((c.centre_sample, c.db.to_vec())));
        }
        out
    }

    fn sine_on_bin_reads_0_db<const N: usize>()
    where
        [f32; N]: FftFrame,
    {
        let mut an = SpectrumAnalyzer::<N>::new(FS);
        let bin = N / 5;
        let x = sine(FS, an.bin_freq(bin), 1.0, 4 * N);
        let cols = columns(&mut an, &x, 1000);
        let (_, db) = cols.last().unwrap();
        assert_eq!(db.len(), N / 2 + 1);
        assert!(db[bin].abs() < 0.1, "N = {N}: {} dB on the bin", db[bin]);
        // Hann: neighbours are 6 dB down, and bins further out far lower.
        assert!((db[bin + 1] + 6.02).abs() < 0.1, "N = {N}: {} dB next bin", db[bin + 1]);
        assert!(db[bin + 4] < -80.0, "N = {N}: {} dB 4 bins away", db[bin + 4]);
    }

    #[test]
    fn full_scale_sine_on_a_bin_reads_0_db_for_each_size() {
        sine_on_bin_reads_0_db::<256>();
        sine_on_bin_reads_0_db::<512>();
        sine_on_bin_reads_0_db::<1024>();
    }

    #[test]
    fn dc_and_nyquist_are_unpacked_into_the_end_bins() {
        let mut an = SpectrumAnalyzer::<512>::new(FS);
        // Alternating +0.5 / -0.5: the Nyquist frequency at -6 dB.
        let x: Vec<f32> = (0..2048).map(|i| if i % 2 == 0 { 0.5 } else { -0.5 }).collect();
        let cols = columns(&mut an, &x, 512);
        let (_, db) = cols.last().unwrap();
        assert!((db[256] + 6.02).abs() < 0.05, "Nyquist bin {} dB", db[256]);
        assert!(db[0] < -100.0, "DC bin {} dB", db[0]);

        let mut an = SpectrumAnalyzer::<512>::new(FS);
        let cols = columns(&mut an, &[1.0; 2048], 512);
        let (_, db) = cols.last().unwrap();
        assert!(db[0].abs() < 0.05, "DC bin {} dB", db[0]);
        assert!(db[256] < -100.0, "Nyquist bin {} dB", db[256]);
    }

    #[test]
    fn silence_reads_min_db() {
        let mut an = SpectrumAnalyzer::<256>::new(FS);
        let cols = columns(&mut an, &[0.0; 256], 256);
        assert!(cols[0].1.iter().all(|&v| (v - MIN_DB).abs() < 0.01));
    }

    #[test]
    fn columns_come_every_hop_with_frame_centres() {
        let mut an = SpectrumAnalyzer::<512>::new(FS);
        assert_eq!(an.hop(), 128);
        let cols = columns(&mut an, &[0.0; 10_000], 333);
        assert_eq!(cols.len(), (10_000 - 512) / 128 + 1);
        for (i, (centre, _)) in cols.iter().enumerate() {
            assert_eq!(*centre, 256 + 128 * i as u64);
        }

        an.reset();
        an.set_hop(300);
        let cols = columns(&mut an, &[0.0; 2_000], 777);
        let centres: Vec<u64> = cols.iter().map(|c| c.0).collect();
        assert_eq!(centres, [256, 556, 856, 1156, 1456]);
    }

    #[test]
    fn block_size_does_not_change_the_columns() {
        let x = crate::test_util::chirp(FS, 80_000.0, 30_000.0, 0.5, 20_000);
        let a = columns(&mut SpectrumAnalyzer::<512>::new(FS), &x, 20_000);
        let b = columns(&mut SpectrumAnalyzer::<512>::new(FS), &x, 37);
        let c = columns(&mut SpectrumAnalyzer::<512>::new(FS), &x, 1);
        assert_eq!(a, b);
        assert_eq!(a, c);
    }

    #[test]
    fn fast_db_matches_log10f_within_0_01_db() {
        let mut worst = 0.0f32;
        let mut p = 1e-19f32;
        while p < 1e6 {
            let exact = 10.0 * libm::log10f(p);
            worst = worst.max((power_to_db(p) - exact).abs());
            p *= 1.0137;
        }
        assert!(worst < 0.01, "worst error {worst} dB");
        assert_eq!(power_to_db(0.0), power_to_db(MIN_POWER));
        assert!((power_to_db(f32::NAN) - MIN_DB).abs() < 0.01);
    }

    #[test]
    fn bin_and_frequency_round_trip() {
        let an = SpectrumAnalyzer::<1024>::new(FS);
        assert_eq!(an.bin_hz(), 187.5);
        for bin in [0, 1, 200, 512] {
            assert_eq!(an.freq_bin(an.bin_freq(bin)), bin);
        }
        assert_eq!(an.freq_bin(40_000.0), 213);
        assert_eq!(an.freq_bin(-5.0), 0);
        assert_eq!(an.freq_bin(200_000.0), 512);
    }

    #[test]
    fn pixels_keep_a_narrow_tone_when_many_bins_share_a_pixel() {
        // 513 bins of 187.5 Hz into 32 pixels of 3 kHz.
        let mut an = SpectrumAnalyzer::<1024>::new(FS);
        let x = sine(FS, 50_250.0, 1.0, 4096);
        let mut px = [0.0f32; 32];
        let mut rev = [0.0f32; 32];
        an.push(&x, |c| {
            c.to_pixels(0.0, 96_000.0, &mut px);
            c.to_pixels(96_000.0, 0.0, &mut rev);
        });
        // 50.25 kHz is in pixel 16 (48 to 51 kHz), within a bin of its peak.
        assert!(px[16] > -1.5, "tone pixel {} dB", px[16]);
        assert!(px[10] < -80.0 && px[22] < -80.0, "far pixels {} {}", px[10], px[22]);
        // Reversed range: high frequencies first.
        assert!(rev[15] > -1.5, "reversed tone pixel {} dB", rev[15]);
    }

    #[test]
    fn each_bin_lands_in_exactly_one_pixel() {
        // One loud bin at a time; with fewer pixels than bins, exactly one
        // pixel shows it.
        let mut db = [-100.0f32; 257];
        for loud in 0..257 {
            db[loud] = 0.0;
            let col = Column { centre_sample: 0, bin_hz: 375.0, db: &db };
            let mut px = [0.0f32; 100];
            col.to_pixels(0.0, 96_375.0, &mut px);
            let hits = px.iter().filter(|&&v| v == 0.0).count();
            assert_eq!(hits, 1, "bin {loud} shows in {hits} pixels");
            db[loud] = -100.0;
        }
    }

    #[test]
    fn pixels_interpolate_when_there_are_more_pixels_than_bins() {
        let db: Vec<f32> = (0..129).map(|k| (k as f32 * 0.37).sin() * 30.0 - 50.0).collect();
        let col = Column { centre_sample: 0, bin_hz: 750.0, db: &db };
        // 40 to 60 kHz in 10 Hz pixels.
        let mut px = std::vec![0.0f32; 2000];
        col.to_pixels(40_000.0, 60_000.0, &mut px);
        for (i, &v) in px.iter().enumerate() {
            let centre_bin = (40_000.0 + 10.0 * (i as f32 + 0.5)) / 750.0;
            let k = centre_bin as usize;
            let (a, b) = (db[k].min(db[k + 1]), db[k].max(db[k + 1]));
            assert!(v >= a - 1e-3 && v <= b + 1e-3, "pixel {i}: {v} not in [{a}, {b}]");
        }
        // The pixel holding a bin centre (54 kHz = bin 72) shows that bin.
        assert!(px[1399] == db[72] || px[1400] == db[72]);
    }
}
