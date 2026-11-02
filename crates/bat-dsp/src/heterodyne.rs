//! Heterodyne and heterodyne comb.
//!
//! A heterodyne detector multiplies the input by a local oscillator (LO) and
//! low-passes the result, so a call at `f` is heard at `|f - lo|`. Like an
//! analogue detector, sounds above and below the LO land on the same audible
//! frequency.
//!
//! A comb runs several heterodynes on the same input, with LOs spread across
//! the band, and mixes them, so calls anywhere in that band are audible without
//! retuning. With the LOs spaced about 2 × the cutoff apart, the audible bands
//! meet with little overlap.
//!
//! Defaults match Oversample: 15 kHz cutoff, 30 kHz comb spacing.

use crate::biquad::Butterworth4;
use crate::osc::Nco;

pub const DEFAULT_CUTOFF_HZ: f32 = 15_000.0;
pub const DEFAULT_COMB_SPACING_HZ: f32 = 30_000.0;

#[derive(Clone, Debug)]
pub struct Heterodyne {
    sample_rate: f32,
    nco: Nco,
    lowpass: Butterworth4,
    cutoff_hz: f32,
    /// 2 × the user gain: mixing halves the amplitude of the difference tone.
    out_gain: f32,
}

impl Heterodyne {
    pub fn new(sample_rate: f32, lo_hz: f32) -> Self {
        Self {
            sample_rate,
            nco: Nco::new(sample_rate, lo_hz),
            lowpass: Butterworth4::lowpass(sample_rate, DEFAULT_CUTOFF_HZ),
            cutoff_hz: DEFAULT_CUTOFF_HZ,
            out_gain: 2.0,
        }
    }

    /// Retune the LO without a click.
    pub fn set_lo(&mut self, lo_hz: f32) {
        self.nco.set_freq(lo_hz);
    }

    pub fn lo(&self) -> f32 {
        self.nco.freq()
    }

    /// Cutoff of the low-pass after the mixer: the highest audible output
    /// frequency, so calls within this distance of the LO are heard.
    pub fn set_cutoff(&mut self, cutoff_hz: f32) {
        self.cutoff_hz = cutoff_hz;
        self.lowpass.set_coefficients(&Butterworth4::lowpass(self.sample_rate, cutoff_hz));
    }

    pub fn cutoff(&self) -> f32 {
        self.cutoff_hz
    }

    /// Linear output gain. 1.0 means a call comes out at its input amplitude.
    pub fn set_gain(&mut self, gain: f32) {
        self.out_gain = 2.0 * gain;
    }

    pub fn reset(&mut self) {
        self.nco.reset_phase();
        self.lowpass.reset();
    }

    #[inline]
    pub fn process_sample(&mut self, x: f32) -> f32 {
        let (lo, _) = self.nco.tick();
        self.lowpass.process_sample(x * lo) * self.out_gain
    }

    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        debug_assert_eq!(input.len(), output.len());
        for (o, &x) in output.iter_mut().zip(input) {
            *o = self.process_sample(x);
        }
    }

    /// Add this heterodyne's output, times `scale`, to `output`.
    fn process_add(&mut self, input: &[f32], output: &mut [f32], scale: f32) {
        for (o, &x) in output.iter_mut().zip(input) {
            *o += self.process_sample(x) * scale;
        }
    }
}

/// Up to `N` heterodynes on one input, mixed. Output is scaled by 1 / (number
/// of active LOs), so changing the count does not change the loudness.
#[derive(Clone, Debug)]
pub struct HetComb<const N: usize> {
    voices: [Heterodyne; N],
    active: usize,
}

impl<const N: usize> HetComb<N> {
    /// LOs at `first_lo_hz`, `first_lo_hz + spacing_hz`, ... for `count` LOs
    /// (at most `N`), with the cutoff set to half the spacing.
    pub fn new(sample_rate: f32, first_lo_hz: f32, spacing_hz: f32, count: usize) -> Self {
        let mut comb = Self {
            voices: core::array::from_fn(|_| Heterodyne::new(sample_rate, first_lo_hz)),
            active: 0,
        };
        comb.set_evenly_spaced(first_lo_hz, spacing_hz, count);
        comb
    }

    /// Set the LO frequencies. Uses the first `N` if more are given. Voices
    /// that stay active keep their phase, so the change does not click.
    pub fn set_los(&mut self, los_hz: &[f32]) {
        self.active = los_hz.len().min(N);
        for (v, &lo) in self.voices.iter_mut().zip(los_hz) {
            v.set_lo(lo);
        }
    }

    /// Evenly spaced LOs, with each voice's cutoff at half the spacing.
    pub fn set_evenly_spaced(&mut self, first_lo_hz: f32, spacing_hz: f32, count: usize) {
        self.active = count.min(N);
        for (i, v) in self.voices.iter_mut().take(self.active).enumerate() {
            v.set_lo(first_lo_hz + spacing_hz * i as f32);
            v.set_cutoff(spacing_hz / 2.0);
        }
    }

    pub fn set_cutoff(&mut self, cutoff_hz: f32) {
        for v in &mut self.voices {
            v.set_cutoff(cutoff_hz);
        }
    }

    pub fn set_gain(&mut self, gain: f32) {
        for v in &mut self.voices {
            v.set_gain(gain);
        }
    }

    pub fn active(&self) -> usize {
        self.active
    }

    pub fn reset(&mut self) {
        for v in &mut self.voices {
            v.reset();
        }
    }

    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        debug_assert_eq!(input.len(), output.len());
        output.fill(0.0);
        if self.active == 0 {
            return;
        }
        let scale = 1.0 / self.active as f32;
        for v in &mut self.voices[..self.active] {
            v.process_add(input, output, scale);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{rms, run_blocks, sine, tone_power, zero_crossing_freq};

    const FS: f32 = 192_000.0;

    #[test]
    fn shifts_a_45khz_call_to_5khz_with_a_40khz_lo() {
        let x = sine(FS, 45_000.0, 0.5, 38_400);
        let mut het = Heterodyne::new(FS, 40_000.0);
        let y = run_blocks(&x, 256, |i, o| het.process(i, o));
        let tail = &y[9_600..];
        let f = zero_crossing_freq(tail, FS);
        assert!((f - 5_000.0).abs() < 20.0, "output at {f} Hz");
        // Gain 1: the difference tone comes out at the input amplitude.
        let level = rms(tail) * core::f32::consts::SQRT_2;
        assert!((level - 0.5).abs() < 0.02, "output amplitude {level}");
        // The 85 kHz sum tone is gone.
        assert!(tone_power(tail, FS, 85_000.0) < 1e-6);
    }

    #[test]
    fn calls_far_from_the_lo_are_silent() {
        let x = sine(FS, 80_000.0, 0.5, 38_400);
        let mut het = Heterodyne::new(FS, 40_000.0);
        let mut y = std::vec![0.0; x.len()];
        het.process(&x, &mut y);
        assert!(rms(&y[9_600..]) < 0.5 * 0.01, "leak {}", rms(&y[9_600..]));
    }

    #[test]
    fn block_size_does_not_change_the_output() {
        let x = sine(FS, 47_000.0, 0.5, 10_000);
        let mut a = Heterodyne::new(FS, 40_000.0);
        let mut b = Heterodyne::new(FS, 40_000.0);
        let ya = run_blocks(&x, 10_000, |i, o| a.process(i, o));
        let yb = run_blocks(&x, 37, |i, o| b.process(i, o));
        assert_eq!(ya, yb);
    }

    #[test]
    fn comb_hears_calls_across_the_band_at_equal_level() {
        // LOs at 25, 55 and 85 kHz with 15 kHz cutoffs.
        let mut levels = std::vec::Vec::new();
        for f in [30_000.0, 60_000.0, 80_000.0] {
            let mut comb: HetComb<4> = HetComb::new(FS, 25_000.0, DEFAULT_COMB_SPACING_HZ, 3);
            let x = sine(FS, f, 0.5, 38_400);
            let mut y = std::vec![0.0; x.len()];
            comb.process(&x, &mut y);
            levels.push(rms(&y[9_600..]));
        }
        // Each call is 5 kHz from an LO, so each comes out at 1/3 of the
        // single-heterodyne level.
        for l in &levels {
            assert!((l - levels[0]).abs() / levels[0] < 0.05, "levels {levels:?}");
        }
        assert!(levels[0] > 0.5 / core::f32::consts::SQRT_2 / 3.0 * 0.9);
    }

    #[test]
    fn comb_count_is_capped_at_n() {
        let comb: HetComb<2> = HetComb::new(FS, 20_000.0, 20_000.0, 5);
        assert_eq!(comb.active(), 2);
    }
}
