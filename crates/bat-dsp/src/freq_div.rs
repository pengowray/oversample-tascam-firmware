//! Zero-crossing frequency division (FD).
//!
//! An FD detector counts the zero crossings of the ultrasonic signal and makes
//! one output event every `division` cycles, so a call at `f` is heard at
//! `f / division`. The output keeps the timing and loudness of each call but
//! not its harmonics.
//!
//! The signal path follows Oversample's `zc_divide`:
//! 1. 4th-order Butterworth high-pass at 15 kHz.
//! 2. Envelope follower: attack about 1 ms, release about 4 ms.
//! 3. Arming with hysteresis: crossings are counted only while the envelope
//!    is well above the noise floor.
//! 4. Every `division` cycles, a 0.15 ms half-sine click whose height follows
//!    the envelope ([`DividerOutput::Clicks`]), or a flip of a square wave
//!    ([`DividerOutput::Square`]).
//! 5. 2nd-order low-pass at 12 kHz.
//!
//! `division` counts whole cycles (an upward and a downward crossing), so the
//! output is at `f / division`, as on analogue FD detectors. Oversample counts
//! each sign change as one, which puts its output at `2f / division`.
//!
//! Unlike Oversample, a crossing counts only once the signal passes a
//! threshold either side of zero: 15% of the envelope, and at least the
//! disarm level. This stops noise riding on a call from adding crossings. A
//! gap of more than one 15 kHz cycle between crossings restarts the count, so
//! the noise in the few milliseconds after a call, while the envelope decays,
//! makes no clicks.
//!
//! Each zero-crossing time is interpolated between samples. At 192 kHz, for
//! a tone well above the noise floor, the frequencies in [`ZcPoint`]s are
//! within 1% up to about 70 kHz and within 2% up to about 87 kHz.

use crate::biquad::{Biquad, Butterworth4};
use crate::osc::Nco;

pub const DEFAULT_DIVISION: u32 = 10;
pub const MAX_DIVISION: u32 = 1 << 16;
/// The envelope must be this many times the noise floor to arm (about 12 dB).
pub const DEFAULT_ARM_RATIO: f32 = 4.0;
pub const HIGHPASS_HZ: f32 = 15_000.0;
pub const OUTPUT_LOWPASS_HZ: f32 = 12_000.0;

/// Envelope attack time. The release is 4 times longer.
const ENVELOPE_MS: f32 = 1.0;
const CLICK_MS: f32 = 0.15;
/// Time constant of the noise floor when the envelope is below it.
const FLOOR_FALL_MS: f32 = 10.0;
/// Fastest rise of the noise floor.
const FLOOR_RISE_DB_PER_S: f32 = 6.0;
/// The floor starts at full scale and falls to the real noise level, so a
/// fresh or reset divider does not click on noise while it settles.
const FLOOR_START: f32 = 1.0;
/// Lowest noise floor, so digital silence never arms.
const MIN_FLOOR: f32 = 1e-5;
const MIN_ARM_RATIO: f32 = 1.1;
/// A crossing must pass this fraction of the envelope either side of zero.
/// Low enough that a sine sampled at 192 kHz passes it every half-cycle up to
/// about 87 kHz.
const CROSSING_FRACTION: f32 = 0.15;
const BUTTERWORTH2_Q: f32 = core::f32::consts::FRAC_1_SQRT_2;

/// What the divider sends to its output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DividerOutput {
    /// A 0.15 ms half-sine click every `division` cycles. For calls above
    /// `division` × 6.7 kHz (67 kHz at division 10) the clicks come faster
    /// than their length, and each new click cuts off the one before, as in
    /// Oversample.
    #[default]
    Clicks,
    /// A square wave that changes sign every `division / 2` cycles, as on
    /// older FD detectors.
    Square,
}

/// One point of a zero-crossing frequency-time plot (Anabat style). The
/// divider makes one every `division` cycles while armed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ZcPoint {
    /// Index of the sample where the point was made, counted from
    /// construction or the last reset.
    pub sample: u64,
    /// Frequency measured over the last `division` cycles.
    pub freq_hz: f32,
}

/// Streaming zero-crossing frequency divider.
///
/// Oversample sets its arming threshold from the peak of the whole file,
/// which a stream cannot know. This divider compares the envelope with a
/// running noise floor instead: the floor falls towards the envelope with a
/// 10 ms time constant, and rises by at most 6 dB per second. A call lasts
/// a few milliseconds, so it barely lifts the floor, while a lasting change
/// in background noise is followed within a few seconds. A steady tone that
/// lasts several seconds is also treated as noise in the end. The divider arms
/// when the envelope exceeds the floor times the arm ratio, and disarms when
/// it drops below the floor times the square root of that ratio (halfway in
/// dB). After construction or a reset, the floor needs about 0.1 s to fall
/// to the noise level, and nothing arms until then.
#[derive(Clone, Debug)]
pub struct FrequencyDivider {
    sample_rate: f32,
    highpass: Butterworth4,
    lowpass: Biquad,
    attack: f32,
    release: f32,
    floor_fall: f32,
    /// Multiplier per sample for the floor's fastest rise.
    floor_rise: f32,
    arm_ratio: f32,
    disarm_ratio: f32,
    division: u32,
    output: DividerOutput,
    gain: f32,
    click_len: u32,
    /// Longest gap between crossings that still counts as one signal: a
    /// full cycle at the high-pass cutoff.
    max_gap: u64,
    /// Makes the half-sine: half a cycle in `click_len` samples.
    click_osc: Nco,

    /// Index of the sample being processed.
    sample: u64,
    env: f32,
    floor: f32,
    armed: bool,
    /// Comparator state: true after the signal last went above the upper
    /// threshold.
    high: bool,
    /// Previous high-passed sample.
    prev: f32,
    /// The last sign change of the high-passed signal: the index of the
    /// sample after it, and the samples either side.
    zc_sample: u64,
    zc_after: f32,
    zc_before: f32,
    /// Whether a reference crossing has been taken since arming.
    has_ref: bool,
    /// Crossings since the reference crossing.
    crossings: u32,
    last_crossing: u64,
    /// Time of the reference crossing: `ref_sample - ref_back` samples.
    ref_sample: u64,
    ref_back: f32,
    click_pos: u32,
    click_amp: f32,
    square_sign: f32,
}

impl FrequencyDivider {
    pub fn new(sample_rate: f32) -> Self {
        let env_window = libm::roundf(sample_rate * ENVELOPE_MS / 1000.0).max(1.0);
        let click_len = (libm::roundf(sample_rate * CLICK_MS / 1000.0) as u32).max(2);
        Self {
            sample_rate,
            highpass: Butterworth4::highpass(sample_rate, HIGHPASS_HZ),
            lowpass: Biquad::lowpass(sample_rate, OUTPUT_LOWPASS_HZ, BUTTERWORTH2_Q),
            attack: 1.0 / env_window,
            release: 1.0 / (4.0 * env_window),
            floor_fall: 1.0 - libm::expf(-1000.0 / (FLOOR_FALL_MS * sample_rate)),
            floor_rise: libm::powf(10.0, FLOOR_RISE_DB_PER_S / (20.0 * sample_rate)),
            arm_ratio: DEFAULT_ARM_RATIO,
            disarm_ratio: libm::sqrtf(DEFAULT_ARM_RATIO),
            division: DEFAULT_DIVISION,
            output: DividerOutput::default(),
            gain: 1.0,
            click_len,
            max_gap: libm::ceilf(sample_rate / HIGHPASS_HZ) as u64,
            click_osc: Nco::new(sample_rate, sample_rate / (2.0 * click_len as f32)),
            sample: 0,
            env: 0.0,
            floor: FLOOR_START,
            armed: false,
            high: false,
            prev: 0.0,
            zc_sample: 0,
            zc_after: 0.0,
            zc_before: 0.0,
            has_ref: false,
            crossings: 0,
            last_crossing: 0,
            ref_sample: 0,
            ref_back: 0.0,
            click_pos: click_len,
            click_amp: 0.0,
            square_sign: 1.0,
        }
    }

    /// Number of cycles per output event, from 1 to 65536.
    pub fn set_division(&mut self, division: u32) {
        self.division = division.clamp(1, MAX_DIVISION);
        self.has_ref = false;
    }

    pub fn division(&self) -> u32 {
        self.division
    }

    pub fn set_output(&mut self, output: DividerOutput) {
        self.output = output;
    }

    pub fn output(&self) -> DividerOutput {
        self.output
    }

    /// How far the envelope must rise above the noise floor to arm, as an
    /// amplitude ratio (4.0 is about 12 dB). Lower values hear quieter calls
    /// but click more on noise. At least 1.1.
    pub fn set_arm_ratio(&mut self, ratio: f32) {
        self.arm_ratio = ratio.max(MIN_ARM_RATIO);
        self.disarm_ratio = libm::sqrtf(self.arm_ratio);
    }

    pub fn arm_ratio(&self) -> f32 {
        self.arm_ratio
    }

    /// Linear output gain. At 1.0 the clicks, or the square wave, reach about
    /// 0.8 times the amplitude of the call.
    pub fn set_gain(&mut self, gain: f32) {
        self.gain = gain;
    }

    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// Whether a call is present, so its crossings are being counted.
    pub fn is_armed(&self) -> bool {
        self.armed
    }

    /// Current noise floor estimate, as an envelope level.
    pub fn noise_floor(&self) -> f32 {
        self.floor
    }

    /// Clear all state and restart the sample count at 0.
    pub fn reset(&mut self) {
        self.highpass.reset();
        self.lowpass.reset();
        self.sample = 0;
        self.env = 0.0;
        self.floor = FLOOR_START;
        self.armed = false;
        self.high = false;
        self.prev = 0.0;
        self.zc_sample = 0;
        self.zc_after = 0.0;
        self.zc_before = 0.0;
        self.has_ref = false;
        self.crossings = 0;
        self.last_crossing = 0;
        self.ref_sample = 0;
        self.ref_back = 0.0;
        self.click_pos = self.click_len;
        self.click_amp = 0.0;
        self.square_sign = 1.0;
    }

    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        self.process_with_points(input, output, &mut []);
    }

    /// Like [`process`](Self::process), and also writes a [`ZcPoint`] for
    /// each output event, up to `points.len()` of them. Returns how many were
    /// written. Points beyond `points.len()` are dropped.
    pub fn process_with_points(&mut self, input: &[f32], output: &mut [f32], points: &mut [ZcPoint]) -> usize {
        debug_assert_eq!(input.len(), output.len());
        let mut written = 0;
        for (o, &x) in output.iter_mut().zip(input) {
            let sample = self.sample;
            let (y, freq) = self.process_sample(x);
            *o = y;
            if let Some(freq_hz) = freq
                && let Some(p) = points.get_mut(written)
            {
                *p = ZcPoint { sample, freq_hz };
                written += 1;
            }
        }
        written
    }

    /// Returns the output sample, and the measured frequency if this sample
    /// completed `division` cycles.
    #[inline]
    fn process_sample(&mut self, x: f32) -> (f32, Option<f32>) {
        let x = self.highpass.process_sample(x);

        let ax = x.abs();
        let k = if ax > self.env { self.attack } else { self.release };
        self.env += k * (ax - self.env);
        let env = self.env;

        if env < self.floor {
            self.floor += self.floor_fall * (env - self.floor);
        } else {
            self.floor = (self.floor * self.floor_rise).min(env);
        }
        self.floor = self.floor.max(MIN_FLOOR);
        let floor = self.floor;

        if !self.armed && env > floor * self.arm_ratio {
            self.armed = true;
        } else if self.armed && env < floor * self.disarm_ratio {
            self.armed = false;
            self.has_ref = false;
        }

        if (x < 0.0) != (self.prev < 0.0) {
            self.zc_sample = self.sample;
            self.zc_after = x;
            self.zc_before = self.prev;
        }

        let mut freq = None;
        let threshold = (floor * self.disarm_ratio).max(env * CROSSING_FRACTION);
        let crossed = if self.high { x < -threshold } else { x > threshold };
        if crossed {
            self.high = !self.high;
            if self.armed {
                freq = self.on_crossing(env);
            }
        }
        self.prev = x;
        self.sample += 1;

        let y = match self.output {
            DividerOutput::Clicks => {
                if self.click_pos < self.click_len {
                    self.click_pos += 1;
                    self.click_osc.tick().1 * self.click_amp
                } else {
                    0.0
                }
            }
            DividerOutput::Square => {
                if self.armed {
                    self.square_sign * self.gain * env
                } else {
                    0.0
                }
            }
        };
        (self.lowpass.process_sample(y), freq)
    }

    /// Count a crossing while armed. Returns the frequency when `division`
    /// cycles (twice as many crossings) are complete. A gap longer than
    /// `max_gap` restarts the count, so the slow crossings of noise after a
    /// call cannot complete it.
    #[inline]
    fn on_crossing(&mut self, env: f32) -> Option<f32> {
        let gap = self.sample - self.last_crossing;
        self.last_crossing = self.sample;
        if !self.has_ref || gap > self.max_gap {
            self.has_ref = true;
            self.crossings = 0;
            (self.ref_sample, self.ref_back) = self.zero_crossing_time();
            return None;
        }
        self.crossings += 1;
        if self.crossings == self.division {
            self.square_sign = -self.square_sign;
        }
        if self.crossings < 2 * self.division {
            return None;
        }
        self.square_sign = -self.square_sign;
        self.crossings = 0;
        self.click_pos = 0;
        self.click_amp = self.gain * env;
        self.click_osc.reset_phase();

        let (sample, back) = self.zero_crossing_time();
        let whole = (sample - self.ref_sample).min(u32::MAX as u64) as u32;
        let span = whole as f32 - (back - self.ref_back);
        self.ref_sample = sample;
        self.ref_back = back;
        Some(self.division as f32 * self.sample_rate / span)
    }

    /// Time of the last sign change, by linear interpolation: `back` samples
    /// before sample `sample`.
    #[inline]
    fn zero_crossing_time(&self) -> (u64, f32) {
        // Zero only before the first sign change after a reset.
        let d = self.zc_after - self.zc_before;
        let back = if d != 0.0 { self.zc_after / d } else { 0.0 };
        (self.zc_sample, back)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{chirp, noise, sine, zero_crossing_freq};
    use std::vec::Vec;

    const FS: f32 = 192_000.0;
    const NOISE_AMP: f32 = 0.002;
    /// Noise alone before each test call, so the floor can settle.
    const LEAD_IN: usize = 38_400;

    /// `LEAD_IN` samples of noise, then `call`, then `tail` samples of noise.
    fn with_noise(call: &[f32], tail: usize) -> Vec<f32> {
        let n = LEAD_IN + call.len() + tail;
        let mut x = noise(NOISE_AMP, n, 7);
        for (xi, c) in x[LEAD_IN..].iter_mut().zip(call) {
            *xi += c;
        }
        x
    }

    /// Run in blocks of `block`, collecting the output and every point.
    fn run(fd: &mut FrequencyDivider, x: &[f32], block: usize) -> (Vec<f32>, Vec<ZcPoint>) {
        let mut y = std::vec![0.0; x.len()];
        let mut points = Vec::new();
        let mut buf = std::vec![ZcPoint::default(); block];
        for (xi, yi) in x.chunks(block).zip(y.chunks_mut(block)) {
            let n = fd.process_with_points(xi, yi, &mut buf);
            points.extend_from_slice(&buf[..n]);
        }
        (y, points)
    }

    /// Count the clicks in `y`: upward crossings of half its peak.
    fn count_clicks(y: &[f32]) -> usize {
        let half = y.iter().fold(0.0f32, |m, v| m.max(*v)) * 0.5;
        y.windows(2).filter(|w| w[0] < half && w[1] >= half).count()
    }

    #[test]
    fn a_40khz_tone_makes_clicks_at_4khz() {
        let tone_len = 57_600; // 0.3 s
        let x = with_noise(&sine(FS, 40_000.0, 0.3, tone_len), 0);
        let mut fd = FrequencyDivider::new(FS);
        let (y, _) = run(&mut fd, &x, 256);
        assert!(y[..LEAD_IN].iter().all(|&v| v == 0.0), "clicks before the tone");
        // Skip the first 10 ms of the tone while the envelope rises.
        let skip = 1_920;
        let clicks = count_clicks(&y[LEAD_IN + skip..]);
        let rate = clicks as f32 / ((tone_len - skip) as f32 / FS);
        assert!((rate - 4_000.0).abs() < 4_000.0 * 0.01, "{rate} clicks per second");
    }

    #[test]
    fn quiet_noise_alone_makes_no_clicks() {
        let x = noise(0.01, 192_000, 3);
        for output in [DividerOutput::Clicks, DividerOutput::Square] {
            let mut fd = FrequencyDivider::new(FS);
            fd.set_output(output);
            let (y, points) = run(&mut fd, &x, 256);
            assert!(y.iter().all(|&v| v == 0.0), "{output:?} output on noise");
            assert!(points.is_empty());
        }
    }

    #[test]
    fn points_measure_a_40khz_tone_within_1_percent() {
        let tone_len = 57_600;
        let x = with_noise(&sine(FS, 40_000.0, 0.3, tone_len), 0);
        let mut fd = FrequencyDivider::new(FS);
        let (_, points) = run(&mut fd, &x, 256);
        // About 4000 points per second for 0.3 s.
        assert!((1_150..=1_200).contains(&points.len()), "{} points", points.len());
        for p in &points {
            assert!(p.sample >= LEAD_IN as u64);
            assert!((p.freq_hz - 40_000.0).abs() < 400.0, "point at {} Hz", p.freq_hz);
        }
    }

    #[test]
    fn points_fall_during_a_downward_chirp() {
        // A 5 ms sweep from 80 to 30 kHz, like an FM bat call.
        let x = with_noise(&chirp(FS, 80_000.0, 30_000.0, 0.3, 960), 4_000);
        let mut fd = FrequencyDivider::new(FS);
        let (_, points) = run(&mut fd, &x, 256);
        let freqs: Vec<f32> = points.iter().map(|p| p.freq_hz).collect();
        assert!(freqs.len() > 15, "points {freqs:?}");
        assert!(freqs[0] > 70_000.0 && freqs[freqs.len() - 1] < 40_000.0, "points {freqs:?}");
        for w in freqs.windows(2) {
            assert!(w[1] < w[0], "points {freqs:?}");
        }
    }

    #[test]
    fn square_output_with_an_odd_division_is_at_f_over_division() {
        let tone_len = 57_600;
        let x = with_noise(&sine(FS, 40_000.0, 0.3, tone_len), 0);
        let mut fd = FrequencyDivider::new(FS);
        fd.set_output(DividerOutput::Square);
        fd.set_division(5);
        let (y, _) = run(&mut fd, &x, 256);
        let f = zero_crossing_freq(&y[LEAD_IN + 1_920..], FS);
        assert!((f - 8_000.0).abs() < 8_000.0 * 0.01, "square wave at {f} Hz");
    }

    #[test]
    fn block_size_does_not_change_the_output() {
        let mut call = sine(FS, 40_000.0, 0.3, 9_600);
        call.extend(chirp(FS, 80_000.0, 30_000.0, 0.2, 960));
        let x = with_noise(&call, 4_000);
        for output in [DividerOutput::Clicks, DividerOutput::Square] {
            let results: Vec<_> = [1, 37, 4096]
                .iter()
                .map(|&block| {
                    let mut fd = FrequencyDivider::new(FS);
                    fd.set_output(output);
                    run(&mut fd, &x, block)
                })
                .collect();
            assert!(!results[0].1.is_empty());
            assert_eq!(results[0], results[1]);
            assert_eq!(results[0], results[2]);
        }
    }
}
