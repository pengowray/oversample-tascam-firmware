//! Automatic level for what the headphones hear. It changes nothing else: the
//! waterfall, the detector and any recording use the input before this.

use libm::expf;

/// Limits of the gain: +24 dB lifts quiet calls without making the noise floor
/// loud between them, and -24 dB is enough for the loudest input.
const MAX_GAIN: f32 = 15.85;
const MIN_GAIN: f32 = 0.063;
/// How long the level of a call counts. Bats call several times a second, so
/// this holds the gain steady between the calls of a pass.
const HOLD_S: f32 = 1.5;
/// How fast the gain falls for a louder sound, and rises afterwards.
const FALL_S: f32 = 0.001;
const RISE_S: f32 = 0.5;

pub(crate) struct AutoLevel {
    on: bool,
    target: f32,
    envelope: f32,
    gain: f32,
    keep: f32,
    fall: f32,
    rise: f32,
}

impl AutoLevel {
    pub(crate) fn new(sample_rate: f32) -> Self {
        let per_sample = |seconds: f32| 1.0 - expf(-1.0 / (seconds * sample_rate));
        Self {
            on: false,
            target: 0.25,
            envelope: 0.0,
            gain: 1.0,
            keep: 1.0 - per_sample(HOLD_S),
            fall: per_sample(FALL_S),
            rise: per_sample(RISE_S),
        }
    }

    pub(crate) fn is_on(&self) -> bool {
        self.on
    }

    /// Start from no change, so turning it on does not jump.
    pub(crate) fn set_on(&mut self, on: bool) {
        if on && !self.on {
            self.envelope = 0.0;
            self.gain = 1.0;
        }
        self.on = on;
    }

    pub(crate) fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    pub(crate) fn apply(&mut self, out: &mut [f32]) {
        if !self.on {
            return;
        }
        for sample in out {
            let peak = sample.abs();
            self.envelope = if peak > self.envelope { peak } else { self.envelope * self.keep };
            let wanted = (self.target / self.envelope.max(1e-9)).clamp(MIN_GAIN, MAX_GAIN);
            let rate = if wanted < self.gain { self.fall } else { self.rise };
            self.gain += (wanted - self.gain) * rate;
            *sample *= self.gain;
        }
    }
}
