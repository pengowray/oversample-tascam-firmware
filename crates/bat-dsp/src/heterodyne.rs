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
//! The voices of a comb all use one low-pass. A filter is linear, so filtering each
//! voice and adding the results gives the same sound as adding the oscillators and
//! filtering once, apart from rounding. The comb does the second, which costs about one
//! filter per channel, however many voices there are.
//!
//! Stereo runs two channels against one oscillator, each with its own low-pass.
//! With a shared oscillator, the phase difference between the channels at the output
//! is the phase difference at the input, so a sound equally loud and in phase on both
//! stays centred. Two oscillators would drift apart, and would cost more.
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
    /// The low-pass of each channel. The mono path uses only the first.
    lowpass: [Butterworth4; 2],
    cutoff_hz: f32,
    /// 2 × the user gain: mixing halves the amplitude of the difference tone.
    out_gain: f32,
}

impl Heterodyne {
    pub fn new(sample_rate: f32, lo_hz: f32) -> Self {
        Self {
            sample_rate,
            nco: Nco::new(sample_rate, lo_hz),
            lowpass: [Butterworth4::lowpass(sample_rate, DEFAULT_CUTOFF_HZ); 2],
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
        let filter = Butterworth4::lowpass(self.sample_rate, cutoff_hz);
        for lowpass in &mut self.lowpass {
            lowpass.set_coefficients(&filter);
        }
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
        for lowpass in &mut self.lowpass {
            lowpass.reset();
        }
    }

    /// Start the second channel's low-pass from the state of the first. Call this
    /// when going from mono to stereo: the mono path leaves the second low-pass
    /// holding whatever it held when stereo last stopped, and that would come out as
    /// a click. After the copy both channels carry on from what was being heard.
    pub fn copy_filter_to_second_channel(&mut self) {
        self.lowpass[1] = self.lowpass[0];
    }

    #[inline]
    pub fn process_sample(&mut self, x: f32) -> f32 {
        let (lo, _) = self.nco.tick();
        self.lowpass[0].process_sample(x * lo) * self.out_gain
    }

    /// One sample of each channel, against one tick of the oscillator. Always inlined,
    /// because a call for every sample would cost as much as the oscillator.
    #[inline(always)]
    pub fn process_sample_stereo(&mut self, left: f32, right: f32) -> (f32, f32) {
        let (lo, _) = self.nco.tick();
        let [filter_left, filter_right] = &mut self.lowpass;
        (filter_left.process_sample(left * lo) * self.out_gain, filter_right.process_sample(right * lo) * self.out_gain)
    }

    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        debug_assert_eq!(input.len(), output.len());
        for (o, &x) in output.iter_mut().zip(input) {
            *o = self.process_sample(x);
        }
    }

    /// Both channels against one oscillator. All four slices must have the same length.
    pub fn process_stereo(&mut self, left: &[f32], right: &[f32], out_left: &mut [f32], out_right: &mut [f32]) {
        debug_assert_eq!(left.len(), out_left.len());
        debug_assert_eq!(right.len(), out_right.len());
        for (((ol, or), &l), &r) in out_left.iter_mut().zip(out_right.iter_mut()).zip(left).zip(right) {
            (*ol, *or) = self.process_sample_stereo(l, r);
        }
    }
}

/// Up to `N` heterodynes on one input, mixed. Output is scaled by 1 / (number
/// of active LOs), so changing the count does not change the loudness.
#[derive(Clone, Debug)]
pub struct HetComb<const N: usize> {
    sample_rate: f32,
    oscillators: [Nco; N],
    active: usize,
    /// The low-pass of each channel. The mono path uses only the first.
    lowpass: [Butterworth4; 2],
    /// 2 × the user gain: mixing halves the amplitude of the difference tone.
    out_gain: f32,
}

impl<const N: usize> HetComb<N> {
    /// LOs at `first_lo_hz`, `first_lo_hz + spacing_hz`, ... for `count` LOs
    /// (at most `N`), with the cutoff set to half the spacing.
    pub fn new(sample_rate: f32, first_lo_hz: f32, spacing_hz: f32, count: usize) -> Self {
        let mut comb = Self {
            sample_rate,
            oscillators: core::array::from_fn(|_| Nco::new(sample_rate, first_lo_hz)),
            active: 0,
            lowpass: [Butterworth4::lowpass(sample_rate, DEFAULT_CUTOFF_HZ); 2],
            out_gain: 2.0,
        };
        comb.set_evenly_spaced(first_lo_hz, spacing_hz, count);
        comb
    }

    /// Set the LO frequencies. Uses the first `N` if more are given. Voices
    /// that stay active keep their phase, so the change does not click.
    pub fn set_los(&mut self, los_hz: &[f32]) {
        self.active = los_hz.len().min(N);
        for (oscillator, &lo) in self.oscillators.iter_mut().zip(los_hz) {
            oscillator.set_freq(lo);
        }
    }

    /// Evenly spaced LOs, with the cutoff at half the spacing.
    pub fn set_evenly_spaced(&mut self, first_lo_hz: f32, spacing_hz: f32, count: usize) {
        self.active = count.min(N);
        for (i, oscillator) in self.oscillators.iter_mut().take(self.active).enumerate() {
            oscillator.set_freq(first_lo_hz + spacing_hz * i as f32);
        }
        self.set_cutoff(spacing_hz / 2.0);
    }

    pub fn set_cutoff(&mut self, cutoff_hz: f32) {
        let filter = Butterworth4::lowpass(self.sample_rate, cutoff_hz);
        for lowpass in &mut self.lowpass {
            lowpass.set_coefficients(&filter);
        }
    }

    pub fn set_gain(&mut self, gain: f32) {
        self.out_gain = 2.0 * gain;
    }

    pub fn active(&self) -> usize {
        self.active
    }

    pub fn reset(&mut self) {
        for oscillator in &mut self.oscillators {
            oscillator.reset_phase();
        }
        for lowpass in &mut self.lowpass {
            lowpass.reset();
        }
    }

    /// See [`Heterodyne::copy_filter_to_second_channel`].
    pub fn copy_filter_to_second_channel(&mut self) {
        self.lowpass[1] = self.lowpass[0];
    }

    /// The active oscillators added together, for the next sample.
    #[inline(always)]
    fn next_lo(oscillators: &mut [Nco]) -> f32 {
        let mut sum = 0.0;
        for oscillator in oscillators {
            sum += oscillator.tick().0;
        }
        sum
    }

    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        debug_assert_eq!(input.len(), output.len());
        if self.active == 0 {
            output.fill(0.0);
            return;
        }
        let gain = self.out_gain / self.active as f32;
        let oscillators = &mut self.oscillators[..self.active];
        let lowpass = &mut self.lowpass[0];
        for (o, &x) in output.iter_mut().zip(input) {
            *o = lowpass.process_sample(x * Self::next_lo(oscillators)) * gain;
        }
    }

    /// Both channels, with one set of oscillators. All four slices must have the same
    /// length.
    pub fn process_stereo(&mut self, left: &[f32], right: &[f32], out_left: &mut [f32], out_right: &mut [f32]) {
        debug_assert_eq!(left.len(), out_left.len());
        debug_assert_eq!(right.len(), out_right.len());
        if self.active == 0 {
            out_left.fill(0.0);
            out_right.fill(0.0);
            return;
        }
        let gain = self.out_gain / self.active as f32;
        let oscillators = &mut self.oscillators[..self.active];
        let [filter_left, filter_right] = &mut self.lowpass;
        for (((ol, or), &l), &r) in out_left.iter_mut().zip(out_right.iter_mut()).zip(left).zip(right) {
            let lo = Self::next_lo(oscillators);
            *ol = filter_left.process_sample(l * lo) * gain;
            *or = filter_right.process_sample(r * lo) * gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{rms, run_blocks, sine, tone_power, zero_crossing_freq};
    use std::vec::Vec;

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
    fn comb_sounds_the_same_as_a_filter_for_each_voice() {
        let x = noise_for_comb();
        let y: Vec<f32> = x.iter().rev().copied().collect();
        let mut comb: HetComb<3> = HetComb::new(FS, 15_000.0, DEFAULT_COMB_SPACING_HZ, 3);
        comb.set_gain(3.0);
        let (l, r) = stereo(&x, &y, |a, b, ol, or| comb.process_stereo(a, b, ol, or));
        let mut mono: HetComb<3> = HetComb::new(FS, 15_000.0, DEFAULT_COMB_SPACING_HZ, 3);
        mono.set_gain(3.0);
        let mono_out = run_blocks(&x, 256, |i, o| mono.process(i, o));

        // The same three LOs and filters, one chain for each voice.
        let mut voices: Vec<Heterodyne> = [15_000.0, 45_000.0, 75_000.0]
            .iter()
            .map(|&lo| {
                let mut voice = Heterodyne::new(FS, lo);
                voice.set_cutoff(DEFAULT_COMB_SPACING_HZ / 2.0);
                voice.set_gain(3.0);
                voice
            })
            .collect();
        let (mut want_l, mut want_r) = (std::vec![0.0f32; x.len()], std::vec![0.0f32; x.len()]);
        for voice in &mut voices {
            let (a, b) = stereo(&x, &y, |p, q, ol, or| voice.process_stereo(p, q, ol, or));
            for i in 0..x.len() {
                want_l[i] += a[i] / 3.0;
                want_r[i] += b[i] / 3.0;
            }
        }
        // More than 100 dB below the sound itself, on either ear and on the mono path.
        for (got, want) in [(&l, &want_l), (&r, &want_r), (&mono_out, &want_l)] {
            let difference: Vec<f32> = got.iter().zip(want.iter()).map(|(a, b)| a - b).collect();
            let db = 20.0 * (rms(&difference) / rms(want)).log10();
            assert!(db < -100.0, "difference {db} dB");
        }
    }

    /// Noise, for a signal with something in every voice's band.
    fn noise_for_comb() -> Vec<f32> {
        crate::test_util::noise(0.3, 96_000, 7)
    }

    #[test]
    fn comb_count_is_capped_at_n() {
        let comb: HetComb<2> = HetComb::new(FS, 20_000.0, 20_000.0, 5);
        assert_eq!(comb.active(), 2);
    }

    /// Run two channels through `process`, as (left, right).
    fn stereo(
        x: &[f32],
        y: &[f32],
        mut process: impl FnMut(&[f32], &[f32], &mut [f32], &mut [f32]),
    ) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (std::vec![0.0; x.len()], std::vec![0.0; x.len()]);
        for (((a, b), ol), or) in x.chunks(256).zip(y.chunks(256)).zip(l.chunks_mut(256)).zip(r.chunks_mut(256)) {
            process(a, b, ol, or);
        }
        (l, r)
    }

    #[test]
    fn a_tone_in_one_channel_comes_out_of_that_channel_only() {
        // Isolation: no leak at all. The silent channel's mixer and low-pass see only
        // zeros, so its output is exactly zero, whatever the other channel carries.
        let tone = sine(FS, 45_000.0, 0.5, 38_400);
        let silence = std::vec![0.0; tone.len()];
        for tone_in_left in [true, false] {
            let (x, y) = if tone_in_left { (&tone, &silence) } else { (&silence, &tone) };
            let mut het = Heterodyne::new(FS, 40_000.0);
            let (l, r) = stereo(x, y, |a, b, ol, or| het.process_stereo(a, b, ol, or));
            let (heard, quiet) = if tone_in_left { (l, r) } else { (r, l) };
            let f = zero_crossing_freq(&heard[9_600..], FS);
            assert!((f - 5_000.0).abs() < 20.0, "output at {f} Hz");
            assert!((rms(&heard[9_600..]) * core::f32::consts::SQRT_2 - 0.5).abs() < 0.02);
            assert!(quiet.iter().all(|&v| v == 0.0));
        }
    }

    #[test]
    fn identical_channels_match_the_mono_path_bit_for_bit() {
        let x = sine(FS, 47_000.0, 0.5, 20_000);
        let mut mono = Heterodyne::new(FS, 40_000.0);
        let expected = run_blocks(&x, 256, |i, o| mono.process(i, o));
        let mut het = Heterodyne::new(FS, 40_000.0);
        let (l, r) = stereo(&x, &x, |a, b, ol, or| het.process_stereo(a, b, ol, or));
        assert_eq!(l, expected);
        assert_eq!(r, expected);

        let mut comb: HetComb<3> = HetComb::new(FS, 25_000.0, DEFAULT_COMB_SPACING_HZ, 3);
        let expected = run_blocks(&x, 256, |i, o| comb.process(i, o));
        let mut comb: HetComb<3> = HetComb::new(FS, 25_000.0, DEFAULT_COMB_SPACING_HZ, 3);
        let (l, r) = stereo(&x, &x, |a, b, ol, or| comb.process_stereo(a, b, ol, or));
        assert_eq!(l, expected);
        assert_eq!(r, expected);
    }

    #[test]
    fn the_comb_keeps_each_channel_apart() {
        // A call in the left channel near one LO and a call in the right near another.
        let x = sine(FS, 30_000.0, 0.5, 38_400);
        let y = sine(FS, 60_000.0, 0.25, 38_400);
        let mut comb: HetComb<3> = HetComb::new(FS, 25_000.0, DEFAULT_COMB_SPACING_HZ, 3);
        comb.set_gain(3.0);
        let (l, r) = stereo(&x, &y, |a, b, ol, or| comb.process_stereo(a, b, ol, or));
        let level = |v: &[f32]| rms(&v[9_600..]) * core::f32::consts::SQRT_2;
        assert!((level(&l) - 0.5).abs() < 0.03, "{}", level(&l));
        assert!((level(&r) - 0.25).abs() < 0.02, "{}", level(&r));
    }

    #[test]
    fn reset_and_cutoff_cover_both_channels() {
        let x = sine(FS, 41_000.0, 0.5, 4_000);
        let silence = std::vec![0.0; 256];
        let mut het = Heterodyne::new(FS, 40_000.0);
        stereo(&x, &x, |a, b, ol, or| het.process_stereo(a, b, ol, or));
        het.reset();
        let (l, r) = stereo(&silence, &silence, |a, b, ol, or| het.process_stereo(a, b, ol, or));
        assert!(l.iter().chain(&r).all(|&v| v == 0.0), "a reset must clear both low-passes");

        // A tone 8 kHz from the LO, with the cutoff moved below it: both channels lose it.
        let x = sine(FS, 48_000.0, 0.5, 38_400);
        let mut het = Heterodyne::new(FS, 40_000.0);
        het.set_cutoff(1_000.0);
        let (l, r) = stereo(&x, &x, |a, b, ol, or| het.process_stereo(a, b, ol, or));
        assert_eq!(l, r);
        assert!(rms(&r[9_600..]) < 0.5 * 0.001, "{}", rms(&r[9_600..]));
    }

    #[test]
    fn copying_the_filter_lets_stereo_carry_on_from_mono() {
        // Mono for a while, with loud sound, then stereo on the same signal: the second
        // channel follows the first from the first sample, with no stale state.
        let x = sine(FS, 47_000.0, 0.5, 2_001);
        let mut y = std::vec![0.0; x.len()];
        let mut het = Heterodyne::new(FS, 40_000.0);
        het.process(&x, &mut y);
        het.copy_filter_to_second_channel();
        let (l, r) = stereo(&x, &x, |a, b, ol, or| het.process_stereo(a, b, ol, or));
        assert_eq!(l, r);
        // The first channel carries on as if stereo had not started.
        let mut plain = Heterodyne::new(FS, 40_000.0);
        let mut first = std::vec![0.0; x.len()];
        plain.process(&x, &mut first);
        let mut rest = std::vec![0.0; x.len()];
        plain.process(&x, &mut rest);
        assert_eq!(l, rest);
    }

    #[test]
    fn retuning_moves_both_channels_together() {
        let x = sine(FS, 47_000.0, 0.5, 20_000);
        let mut het = Heterodyne::new(FS, 40_000.0);
        let (l1, r1) = stereo(&x[..10_001], &x[..10_001], |a, b, ol, or| het.process_stereo(a, b, ol, or));
        assert_eq!(l1, r1);
        het.set_lo(44_000.0);
        let (l2, r2) = stereo(&x[10_001..], &x[10_001..], |a, b, ol, or| het.process_stereo(a, b, ol, or));
        assert_eq!(l2, r2);
        let f = zero_crossing_freq(&l2[4_000..], FS);
        assert!((f - 3_000.0).abs() < 20.0, "output at {f} Hz");
    }
}
