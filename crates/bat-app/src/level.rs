//! Automatic level for what the headphones hear. It changes nothing else: the
//! waterfall, the detector and any recording use the input before this.
//!
//! Two stages run on the listening mode's output. The gain stage keeps calls near
//! the target level without lifting the background noise above a set level. The
//! gate, used only in [`AutoVolume::BatsOnly`], silences the output between calls.
//!
//! In stereo there is one gain and one gate for both ears. Separate gains would bring
//! left and right to the same level, and flatten the stereo image. The envelope and the
//! noise floor both follow the louder ear, so with the same signal in both ears, or one
//! ear silent, the stereo path gives exactly what the mono path gives on that signal.
//! A floor from the mean of the two ears' power would read 3 dB low with one ear silent,
//! and the other ear's noise would come out 3 dB above the limit.

use libm::{expf, log10f};

use crate::Mode;

/// The choices for Auto volume.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AutoVolume {
    /// The output stays at its fixed level.
    Off,
    /// The gain stage runs.
    On,
    /// The gain stage runs, and the output is silent except around detected calls.
    BatsOnly,
}

impl AutoVolume {
    /// In the order the wheel steps through them.
    pub const ALL: [AutoVolume; 3] = [AutoVolume::Off, AutoVolume::On, AutoVolume::BatsOnly];

    pub fn label(self) -> &'static str {
        match self {
            AutoVolume::Off => "Off",
            AutoVolume::On => "On",
            AutoVolume::BatsOnly => "Bats only",
        }
    }
}

/// Limits of the gain: +24 dB lifts quiet calls and -24 dB is enough for the
/// loudest input.
const MAX_GAIN: f32 = 15.85;
const MIN_GAIN: f32 = 0.063;
const MAX_GAIN_DB: f32 = 24.0;
/// Time constant of the level of a call fading. Bats call several times a
/// second, so this keeps the gain steady between the calls of a pass; after a
/// sound stops the gain rises about 6 dB per second.
const HOLD_S: f32 = 1.5;
/// Time constants of the gain falling for a louder sound and rising after one.
/// The rise also applies when the gain falls because the noise floor has risen,
/// so that a change in the floor never moves the gain quickly. Only a sound louder
/// than the gain allows for moves it at the fast rate, and that sound then hides
/// the change.
const FALL_S: f32 = 0.001;
const RISE_S: f32 = 0.5;

/// How far below the target the noise floor may be lifted. A sound this far above the
/// floor reaches the target; a sound closer to the floor comes out below the target by
/// the difference. Steady noise comes out about 36 dB below the target, whatever its
/// level, as far as the gain limits allow, and noise that fluctuates about 34 dB below.
/// Chosen on an X8 recording at night with the
/// recent input gain (261105_0086): the median noise goes from -40.6 dBFS on B231 to
/// -46.4 on HET, from -28.0 to -45.8 on COMB and from -24.8 to -45.4 on 1:1. At 30 dB,
/// HET stayed at -40.6, its limit of +24 dB. This is a judgement made from figures, not
/// from listening.
const NOISE_BELOW_TARGET_DB: f32 = 36.0;
/// Length of the short-term level the floor follows: long enough to average the
/// noise, and short enough that the quiet between calls has windows of its own (the
/// gaps in a pass are 10 ms to a few hundred ms).
const FLOOR_WINDOW_S: f32 = 0.01;
/// Time constants of the floor, in dB. The floor falls four times faster than it
/// rises, so it follows the quieter part of the background. A 5 ms call 40 dB above
/// the floor raises it by about 0.2 dB, and the quiet windows after the call take that
/// away. The floor catches up with a sound that continues, within a few seconds, so a
/// steady hum ends up treated as noise. With a 50 ms fall, the gain followed every dip
/// in the noise: on COMB it moved 0.15 dB per 100 ms, 0.09 dB with this fall, and 0.5 dB
/// on B231.
const FLOOR_RISE_S: f32 = 2.0;
const FLOOR_FALL_S: f32 = 0.5;
/// A window below this (-120 dBFS) is digital silence, which says nothing about the
/// noise: the mix while MANUAL mutes it. The floor stays where it was, so the sound
/// after a mute is not lifted while the floor climbs back.
const SILENCE_DB: f32 = -120.0;

/// How long the gate takes to open and to close. In 5 ms it is open well before the next
/// call of a pass, and with the square law its level starts from zero with no step, so
/// there is no click. A 200 ms close sounds like the sound ending, and not like a cut.
const GATE_OPEN_S: f32 = 0.005;
const GATE_CLOSE_S: f32 = 0.2;
/// How long the gate stays open after a call that has not been confirmed as a bat's,
/// and after one that has. The first is the classifier's own limit: a second call more
/// than 0.6 s after the first would not make a train with it, so waiting longer for
/// one only adds noise after a lone sound. Of the gaps between bat calls under 2 s,
/// 99.3% are shorter than that in the X8's recordings and 98.3% in the library of 236
/// recordings. Once a pass is confirmed, 1 s keeps it in one piece: 99.7% and 99.2% of
/// the gaps are shorter.
///
/// A single opens the gate only at 18 dB or more above the floor. Figures from 880 s of
/// X8 bat recordings and 91 minutes with no bats:
///
/// | Opens on                 | Calls heard whole | Missed before / after first heard | Openings per minute with no bats |
/// |--------------------------|-------------------|-----------------------------------|----------------------------------|
/// | Confirmed bat calls only | 97.2%             | 2.2 / 0.7                         | 1.2                              |
/// | Those and strong singles | 97.7%             | 1.8 / 0.5                         | 1.9                              |
/// | Those and any single     | 99.5%             | 0.5 / 0.1                         | 6.8                              |
///
/// Any single also gave 16 openings a minute on a night recording with no bats, against
/// 0.8: a burst of noise every few seconds, for hearing a pass about 150 ms sooner.
const GATE_HOLD_S: f32 = 0.6;
const GATE_HOLD_BAT_S: f32 = 1.0;

fn db_power(power: f32) -> f32 {
    10.0 * log10f(power.max(1e-20))
}

pub(crate) struct AutoLevel {
    volume: AutoVolume,
    target: f32,
    envelope: f32,
    gain: f32,
    keep: f32,
    fall: f32,
    rise: f32,

    /// The most the noise floor may come out at, in dB.
    ceiling_db: f32,
    /// The background level of the output, in dB, once a window has set it.
    floor_db: Option<f32>,
    /// The most gain that keeps the floor at or under the ceiling.
    floor_gain: f32,
    /// The sum of squares of the window being filled, for one chain or the left ear,
    /// and for the right ear, and the samples left in the window.
    sum: f32,
    sum_right: f32,
    window_left: u32,
    window_len: u32,
    floor_rise: f32,
    floor_fall: f32,

    gate: Gate,
}

impl AutoLevel {
    pub(crate) fn new(sample_rate: f32) -> Self {
        let per_sample = |seconds: f32| 1.0 - expf(-1.0 / (seconds * sample_rate));
        let per_window = |seconds: f32| 1.0 - expf(-FLOOR_WINDOW_S / seconds);
        let window_len = ((FLOOR_WINDOW_S * sample_rate) as u32).max(1);
        let target = 0.25;
        Self {
            volume: AutoVolume::Off,
            target,
            envelope: 0.0,
            gain: 1.0,
            keep: 1.0 - per_sample(HOLD_S),
            fall: per_sample(FALL_S),
            rise: per_sample(RISE_S),
            ceiling_db: ceiling_db(target),
            floor_db: None,
            floor_gain: MAX_GAIN,
            sum: 0.0,
            sum_right: 0.0,
            window_left: window_len,
            window_len,
            floor_rise: per_window(FLOOR_RISE_S),
            floor_fall: per_window(FLOOR_FALL_S),
            gate: Gate::new(sample_rate),
        }
    }

    pub(crate) fn volume(&self) -> AutoVolume {
        self.volume
    }

    /// Going from Off, start from no change, so turning it on does not jump. Going
    /// between On and Bats only, the gain stage carries on.
    pub(crate) fn set_volume(&mut self, volume: AutoVolume) {
        if self.volume == AutoVolume::Off && volume != AutoVolume::Off {
            self.envelope = 0.0;
            self.gain = 1.0;
            self.floor_db = None;
            self.floor_gain = MAX_GAIN;
            self.sum = 0.0;
            self.sum_right = 0.0;
            self.window_left = self.window_len;
        }
        self.volume = volume;
    }

    /// The output now comes from a different mode or input, whose noise level may
    /// differ by 20 dB or more. The next window sets the floor, as it does when Auto
    /// volume is turned on. Until then the gain has the same limit it had.
    pub(crate) fn sound_changed(&mut self) {
        self.floor_db = None;
        self.sum = 0.0;
        self.sum_right = 0.0;
        self.window_left = self.window_len;
    }

    pub(crate) fn set_target(&mut self, target: f32) {
        self.target = target;
        self.ceiling_db = ceiling_db(target);
    }

    pub(crate) fn gain(&self) -> f32 {
        self.gain
    }

    #[cfg(test)]
    pub(crate) fn floor_db(&self) -> Option<f32> {
        self.floor_db
    }

    pub(crate) fn gate_level(&self) -> f32 {
        self.gate.level
    }

    /// A call that opens the gate was detected: one the classifier has confirmed as
    /// part of a train, or one it has not.
    pub(crate) fn call_detected(&mut self, confirmed: bool) {
        self.gate.call_detected(confirmed);
    }

    /// Set the gain and the gate on a block of the listening mode's output. `analysed`
    /// is false when the detector did not hear this block.
    pub(crate) fn apply(&mut self, out: &mut [f32], mode: Mode, analysed: bool) {
        if self.volume != AutoVolume::Off {
            let use_floor = self.floor_applies(mode);
            let mut rest = &mut *out;
            while !rest.is_empty() {
                let n = rest.len().min(self.window_left as usize);
                let (block, later) = rest.split_at_mut(n);
                for sample in block {
                    self.sum += *sample * *sample;
                    *sample *= self.next_gain(sample.abs());
                }
                self.window_advance(n as u32, use_floor);
                rest = later;
            }
        }
        self.gate.apply(out, !gated(self.volume, mode), analysed);
    }

    /// The same for both ears, with one gain and one gate. The slices must have the
    /// same length.
    pub(crate) fn apply_stereo(&mut self, left: &mut [f32], right: &mut [f32], mode: Mode, analysed: bool) {
        if self.volume != AutoVolume::Off {
            let use_floor = self.floor_applies(mode);
            let (mut rest_left, mut rest_right) = (&mut *left, &mut *right);
            while !rest_left.is_empty() {
                let n = rest_left.len().min(self.window_left as usize);
                let (block_left, later_left) = rest_left.split_at_mut(n);
                let (block_right, later_right) = rest_right.split_at_mut(n);
                for (l, r) in block_left.iter_mut().zip(block_right.iter_mut()) {
                    self.sum += *l * *l;
                    self.sum_right += *r * *r;
                    let gain = self.next_gain(l.abs().max(r.abs()));
                    *l *= gain;
                    *r *= gain;
                }
                self.window_advance(n as u32, use_floor);
                (rest_left, rest_right) = (later_left, later_right);
            }
        }
        self.gate.apply_stereo(left, right, !gated(self.volume, mode), analysed);
    }

    /// Whether the noise floor limits the gain in this mode.
    ///
    /// FD and time expansion are silent until a call. FD has its own noise gate, so
    /// its floor would be the level of the calls. Most of the level of what time
    /// expansion plays is low frequencies, slowed to below hearing: on four of the
    /// X8's recordings at its default expansion, the median was -21 to -24 dBFS in
    /// all, and -38 to -51 dBFS above 300 Hz, about where the floor puts the noise in
    /// the other modes. With the floor, the median above 300 Hz fell by 9 to 21 dB,
    /// and the loudest calls above 300 Hz by 2 to 20 dB, to -25 to -43 dBFS.
    fn floor_applies(&mut self, mode: Mode) -> bool {
        let use_floor = !matches!(mode, Mode::Fd | Mode::Te);
        if !use_floor {
            self.floor_gain = MAX_GAIN;
        }
        use_floor
    }

    /// The gain for the next sample, whose level is `peak`.
    #[inline]
    fn next_gain(&mut self, peak: f32) -> f32 {
        self.envelope = if peak > self.envelope { peak } else { self.envelope * self.keep };
        let for_target = self.target / self.envelope.max(1e-9);
        // The fast rate is only for a sound louder than the gain allows for.
        let rate = if for_target < self.gain { self.fall } else { self.rise };
        let wanted = for_target.min(self.floor_gain).clamp(MIN_GAIN, MAX_GAIN);
        self.gain += (wanted - self.gain) * rate;
        self.gain
    }

    /// `n` samples of the floor's window have been added to `sum`.
    #[inline]
    fn window_advance(&mut self, n: u32, use_floor: bool) {
        self.window_left -= n;
        if self.window_left == 0 {
            self.finish_window(use_floor);
        }
    }

    /// Move the floor towards the level of the window just filled, taking the louder ear
    /// in stereo. With one chain `sum_right` is zero.
    fn finish_window(&mut self, use_floor: bool) {
        let level_db = db_power(self.sum.max(self.sum_right) / self.window_len as f32);
        self.sum = 0.0;
        self.sum_right = 0.0;
        self.window_left = self.window_len;
        if !use_floor || level_db < SILENCE_DB {
            return;
        }
        let floor_db = match self.floor_db {
            None => level_db,
            Some(floor_db) => {
                let rate = if level_db > floor_db { self.floor_rise } else { self.floor_fall };
                floor_db + (level_db - floor_db) * rate
            }
        };
        // Below this the +24 dB limit applies anyway, and a lower floor would only
        // take longer to climb back.
        let floor_db = floor_db.max(self.ceiling_db - MAX_GAIN_DB);
        self.floor_db = Some(floor_db);
        self.floor_gain = expf((self.ceiling_db - floor_db) * core::f32::consts::LN_10 / 20.0);
    }
}

/// The most the noise floor may come out at for a target level, in dB.
fn ceiling_db(target: f32) -> f32 {
    20.0 * log10f(target) - NOISE_BELOW_TARGET_DB
}

/// Whether the gate can close. Time expansion is already silent until a call triggers
/// it or MARK is pressed, and its playback lasts seconds, far beyond any hold.
fn gated(volume: AutoVolume, mode: Mode) -> bool {
    volume == AutoVolume::BatsOnly && mode != Mode::Te
}

/// Silences the output between calls. The time left in the hold counts only sound the
/// detector heard. While the host skips the analysis, or when input is lost, the
/// detector cannot report the next call of a pass, so the gate waits for it instead of
/// closing in the middle of the pass; and with no reports, a closed gate stays closed.
struct Gate {
    /// How much of the output is let through before the square law, 0 to 1.
    level: f32,
    /// Samples of analysed sound left before the gate starts to close.
    hold_left: u32,
    hold: u32,
    hold_bat: u32,
    open_step: f32,
    close_step: f32,
}

impl Gate {
    fn new(sample_rate: f32) -> Self {
        Self {
            level: 1.0,
            hold_left: 0,
            hold: (GATE_HOLD_S * sample_rate) as u32,
            hold_bat: (GATE_HOLD_BAT_S * sample_rate) as u32,
            open_step: 1.0 / (GATE_OPEN_S * sample_rate),
            close_step: 1.0 / (GATE_CLOSE_S * sample_rate),
        }
    }

    fn call_detected(&mut self, confirmed: bool) {
        self.hold_left = self.hold_left.max(if confirmed { self.hold_bat } else { self.hold });
    }

    /// Whether the gate is open for a block of `len` samples. Counts the block off the
    /// hold if the detector heard it.
    fn open_for(&mut self, len: usize, always_open: bool, analysed: bool) -> bool {
        let open = always_open || self.hold_left > 0;
        if analysed {
            self.hold_left = self.hold_left.saturating_sub(len as u32);
        }
        open
    }

    fn apply(&mut self, out: &mut [f32], always_open: bool, analysed: bool) {
        let open = self.open_for(out.len(), always_open, analysed);
        match (open, self.level) {
            (true, 1.0) => {}
            (false, 0.0) => out.fill(0.0),
            _ => {
                let step = if open { self.open_step } else { -self.close_step };
                for sample in out {
                    self.level = (self.level + step).clamp(0.0, 1.0);
                    *sample *= self.level * self.level;
                }
            }
        }
    }

    /// The same for both ears, with the same level for each sample.
    fn apply_stereo(&mut self, left: &mut [f32], right: &mut [f32], always_open: bool, analysed: bool) {
        let open = self.open_for(left.len(), always_open, analysed);
        match (open, self.level) {
            (true, 1.0) => {}
            (false, 0.0) => {
                left.fill(0.0);
                right.fill(0.0);
            }
            _ => {
                let step = if open { self.open_step } else { -self.close_step };
                for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                    self.level = (self.level + step).clamp(0.0, 1.0);
                    let let_through = self.level * self.level;
                    *l *= let_through;
                    *r *= let_through;
                }
            }
        }
    }
}
