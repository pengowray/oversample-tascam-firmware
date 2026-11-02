//! Numerically controlled oscillator.
//!
//! A unit phasor rotated by a fixed step each sample, so a sample costs four
//! multiplies and two adds instead of a `sin` and a `cos`. Rounding makes the
//! phasor's length drift, so it is pulled back to 1 every [`RENORM_INTERVAL`]
//! samples.

use core::f32::consts::TAU;

/// Samples between length corrections. Drift over this many f32 rotations is
/// far below the 24-bit noise floor.
const RENORM_INTERVAL: u32 = 256;

#[derive(Clone, Debug)]
pub struct Nco {
    sample_rate: f32,
    freq_hz: f32,
    /// Current phasor (cos, sin).
    re: f32,
    im: f32,
    /// Per-sample rotation (cos, sin).
    step_re: f32,
    step_im: f32,
    since_renorm: u32,
}

impl Nco {
    pub fn new(sample_rate: f32, freq_hz: f32) -> Self {
        let mut nco = Self {
            sample_rate,
            freq_hz: 0.0,
            re: 1.0,
            im: 0.0,
            step_re: 1.0,
            step_im: 0.0,
            since_renorm: 0,
        };
        nco.set_freq(freq_hz);
        nco
    }

    /// Change frequency without a phase jump.
    pub fn set_freq(&mut self, freq_hz: f32) {
        self.freq_hz = freq_hz;
        let w = TAU * freq_hz / self.sample_rate;
        self.step_re = libm::cosf(w);
        self.step_im = libm::sinf(w);
    }

    pub fn freq(&self) -> f32 {
        self.freq_hz
    }

    pub fn reset_phase(&mut self) {
        self.re = 1.0;
        self.im = 0.0;
        self.since_renorm = 0;
    }

    /// Returns (cos, sin) of the current phase, then advances one sample.
    #[inline]
    pub fn tick(&mut self) -> (f32, f32) {
        let out = (self.re, self.im);
        let re = self.re * self.step_re - self.im * self.step_im;
        let im = self.re * self.step_im + self.im * self.step_re;
        self.re = re;
        self.im = im;
        self.since_renorm += 1;
        if self.since_renorm >= RENORM_INTERVAL {
            self.since_renorm = 0;
            // One Newton step towards length 1: g = (3 - |z|^2) / 2.
            let g = 1.5 - 0.5 * (re * re + im * im);
            self.re *= g;
            self.im *= g;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phasor_length_stays_one_for_a_minute_at_192k() {
        let mut nco = Nco::new(192_000.0, 41_234.5);
        let mut worst = 0.0f32;
        for _ in 0..(192_000 * 60) {
            let (c, s) = nco.tick();
            worst = worst.max(((c * c + s * s).sqrt() - 1.0).abs());
        }
        assert!(worst < 1e-5, "phasor length drifted by {worst}");
    }

    #[test]
    fn frequency_is_right() {
        let fs = 192_000.0;
        let f = 45_000.0;
        let mut nco = Nco::new(fs, f);
        let x: std::vec::Vec<f32> = (0..19_200).map(|_| nco.tick().0).collect();
        let est = crate::test_util::zero_crossing_freq(&x, fs);
        assert!((est - f).abs() < 5.0, "estimated {est} Hz");
    }

    #[test]
    fn retuning_keeps_phase_continuous() {
        let fs = 192_000.0;
        let mut nco = Nco::new(fs, 30_000.0);
        let mut last = nco.tick();
        for _ in 0..100 {
            last = nco.tick();
        }
        nco.set_freq(31_000.0);
        // The first sample after retuning is the old phasor advanced by the
        // old step, so it continues the old waveform.
        let next = nco.tick();
        let w = TAU * 30_000.0 / fs;
        let expect = (
            last.0 * libm::cosf(w) - last.1 * libm::sinf(w),
            last.0 * libm::sinf(w) + last.1 * libm::cosf(w),
        );
        assert!((next.0 - expect.0).abs() < 1e-4 && (next.1 - expect.1).abs() < 1e-4);
    }
}
