//! Telling bat calls from the other sounds the call detector finds.
//!
//! [`CallDetector`](crate::detector::CallDetector) reports anything that rises
//! above the background in the ultrasonic band: bat calls, and also clicks,
//! rustles, pecking, sweeping, the upper harmonics of bird calls, and insects.
//! [`CallClassifier`] sorts each detection with checks that hold for bats in
//! general, not for one species:
//!
//! 1. **Broadband**: the call's range reaches the bottom of the detection
//!    band. Clicks and rustles are loudest at low frequencies and reach down
//!    to it; bat calls stop well above it.
//! 2. **Too low**: the peak is below the lowest bat frequency (default
//!    20 kHz, settable for the few bats that call lower).
//! 3. **Audible**: the audible band (2 to 12 kHz) was loud at the same moment
//!    (default: more than 4 dB above its floor). Bats are silent there; birds,
//!    voices and footsteps are not.
//! 4. **Train**: bats call in trains, several calls a second at similar
//!    frequencies. A call that passes the first three checks is a bat call
//!    once a similar call (peak within 5 kHz) passed them in the last 0.6 s,
//!    and at least one of the two is 18 dB above the floor. Until then it is
//!    a single.
//!
//! The first call of a train only becomes a bat call when the second arrives,
//! so [`Verdict::earlier`] hands it back then, for a display to mark too.
//!
//! Tuned on Portacapture X8 recordings at 192 kHz (call detector with
//! 512-point columns every 128 samples): of 97 calls from a bat at 45 to
//! 50 kHz it kept 94, it kept 97 of 98 calls in two annotated passes, and in
//! two recordings with no bats (birds feeding beside the mics, traffic,
//! sweeping) it reduced detections from 96 and 39 a minute to 7.4 and 2.0.
//!
//! Checked on 247 Xeno-canto echolocation recordings of 76 genera from
//! around the world, resampled to 192 kHz (`bat-app` example `library`):
//! 239 gave bat calls, and 69% of all detections in them were classified as
//! bat calls. The 8 that gave none were free-tails calling below 20 kHz (3),
//! horseshoe bats above 100 kHz, beyond the X8's range (2), clips of 0.3 s
//! (2), and a low-quality recording with loud background sound. Lowering the
//! lowest bat frequency to 16 kHz keeps 65% of free-tail calls instead of
//! 54%, but adds about half as many false detections again; a lower audible
//! band and laxer thresholds traded similarly, so the defaults stay.

use crate::detector::CallEvent;

pub const DEFAULT_MIN_BAT_HZ: f32 = 20_000.0;
pub const DEFAULT_MAX_AUDIBLE_DB: f32 = 4.0;
pub const DEFAULT_TRAIN_WINDOW_S: f32 = 0.6;
pub const DEFAULT_TRAIN_FREQ_HZ: f32 = 5_000.0;
pub const DEFAULT_TRAIN_MIN_DB: f32 = 18.0;

/// A range whose bottom is within this of the band's bottom reaches it.
const EDGE_MARGIN_HZ: f32 = 1_000.0;
/// Recent calls remembered for trains.
const RECENT: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CallKind {
    /// Reaches the bottom of the detection band: a click or rustle.
    Broadband,
    /// Peak below the lowest bat frequency.
    TooLow,
    /// A loud audible sound came at the same moment.
    Audible,
    /// Passes the checks, but no similar call came just before it (yet).
    Single,
    /// Passes the checks and follows a similar call: a bat call.
    Bat,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Verdict {
    pub kind: CallKind,
    /// When this call is the second of a train, the first call, which is a
    /// bat call too but was reported as a single.
    pub earlier: Option<CallEvent>,
}

#[derive(Clone, Copy, Debug)]
struct Recent {
    event: CallEvent,
    /// Already reported as a bat call, directly or as `earlier`.
    bat: bool,
}

#[derive(Clone, Debug)]
pub struct CallClassifier {
    sample_rate: f32,
    band_lo_hz: f32,
    min_bat_hz: f32,
    max_audible_db: f32,
    train_window: u64,
    train_freq_hz: f32,
    train_min_db: f32,
    recent: [Option<Recent>; RECENT],
    next: usize,
}

impl CallClassifier {
    /// `band_lo_hz` is the bottom of the call detector's band.
    pub fn new(sample_rate: f32, band_lo_hz: f32) -> Self {
        let mut c = Self {
            sample_rate,
            band_lo_hz,
            min_bat_hz: DEFAULT_MIN_BAT_HZ,
            max_audible_db: DEFAULT_MAX_AUDIBLE_DB,
            train_window: 0,
            train_freq_hz: DEFAULT_TRAIN_FREQ_HZ,
            train_min_db: DEFAULT_TRAIN_MIN_DB,
            recent: [None; RECENT],
            next: 0,
        };
        c.set_train_window_s(DEFAULT_TRAIN_WINDOW_S);
        c
    }

    pub fn set_band_lo_hz(&mut self, hz: f32) {
        self.band_lo_hz = hz;
    }

    pub fn set_min_bat_hz(&mut self, hz: f32) {
        self.min_bat_hz = hz;
    }

    pub fn min_bat_hz(&self) -> f32 {
        self.min_bat_hz
    }

    pub fn set_max_audible_db(&mut self, db: f32) {
        self.max_audible_db = db;
    }

    pub fn max_audible_db(&self) -> f32 {
        self.max_audible_db
    }

    /// How long before a call a similar call may come and still make a train.
    pub fn set_train_window_s(&mut self, s: f32) {
        self.train_window = (s.max(0.0) * self.sample_rate) as u64;
    }

    /// How far apart two calls' peaks may be and still make a train.
    pub fn set_train_freq_hz(&mut self, hz: f32) {
        self.train_freq_hz = hz;
    }

    /// At least one call of a pair must rise this far above the floor.
    pub fn set_train_min_db(&mut self, db: f32) {
        self.train_min_db = db;
    }

    pub fn reset(&mut self) {
        self.recent = [None; RECENT];
        self.next = 0;
    }

    /// Classify the next detection. Detections must come in time order.
    pub fn classify(&mut self, event: &CallEvent) -> Verdict {
        let kind = if event.min_freq_hz <= self.band_lo_hz + EDGE_MARGIN_HZ {
            CallKind::Broadband
        } else if event.peak_freq_hz < self.min_bat_hz {
            CallKind::TooLow
        } else if event.audible_db_above_floor > self.max_audible_db {
            CallKind::Audible
        } else {
            CallKind::Single
        };
        if kind != CallKind::Single {
            return Verdict { kind, earlier: None };
        }

        // The latest earlier call that makes a train with this one.
        let mut partner: Option<usize> = None;
        for (i, slot) in self.recent.iter().enumerate() {
            let Some(r) = slot else { continue };
            let gap = event.start_sample.saturating_sub(r.event.start_sample);
            let similar = (r.event.peak_freq_hz - event.peak_freq_hz).abs() <= self.train_freq_hz;
            let strong = r.event.peak_db_above_floor.max(event.peak_db_above_floor) >= self.train_min_db;
            let later = partner.is_none_or(|p| {
                self.recent[p].is_some_and(|q| r.event.start_sample > q.event.start_sample)
            });
            if gap <= self.train_window && similar && strong && later {
                partner = Some(i);
            }
        }

        let mut verdict = Verdict { kind: CallKind::Single, earlier: None };
        if let Some(p) = partner {
            verdict.kind = CallKind::Bat;
            if let Some(r) = &mut self.recent[p]
                && !r.bat
            {
                r.bat = true;
                verdict.earlier = Some(r.event);
            }
        }
        self.recent[self.next] = Some(Recent { event: *event, bat: verdict.kind == CallKind::Bat });
        self.next = (self.next + 1) % RECENT;
        verdict
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f32 = 192_000.0;

    fn call(at_s: f32, peak_khz: f32, db: f32) -> CallEvent {
        let start = (at_s * FS) as u64;
        CallEvent {
            start_sample: start,
            end_sample: start + 960,
            peak_freq_hz: peak_khz * 1000.0,
            peak_db_above_floor: db,
            audible_db_above_floor: -3.0,
            min_freq_hz: (peak_khz - 1.0) * 1000.0,
            max_freq_hz: (peak_khz + 8.0) * 1000.0,
        }
    }

    fn classifier() -> CallClassifier {
        CallClassifier::new(FS, 15_000.0)
    }

    #[test]
    fn a_train_is_confirmed_on_its_second_call_and_hands_back_the_first() {
        let mut c = classifier();
        let first = call(1.00, 47.0, 25.0);
        assert_eq!(c.classify(&first), Verdict { kind: CallKind::Single, earlier: None });
        let second = c.classify(&call(1.08, 47.5, 22.0));
        assert_eq!(second.kind, CallKind::Bat);
        assert_eq!(second.earlier, Some(first));
        // The third call joins the train; the first is not handed back twice.
        assert_eq!(c.classify(&call(1.16, 46.8, 20.0)), Verdict { kind: CallKind::Bat, earlier: None });
    }

    #[test]
    fn calls_far_apart_in_time_or_frequency_stay_single() {
        let mut c = classifier();
        assert_eq!(c.classify(&call(1.0, 47.0, 25.0)).kind, CallKind::Single);
        assert_eq!(c.classify(&call(2.0, 47.0, 25.0)).kind, CallKind::Single);
        assert_eq!(c.classify(&call(2.1, 60.0, 25.0)).kind, CallKind::Single);
    }

    #[test]
    fn a_weak_pair_stays_single() {
        let mut c = classifier();
        c.classify(&call(1.0, 47.0, 14.0));
        assert_eq!(c.classify(&call(1.1, 47.0, 15.0)).kind, CallKind::Single);
    }

    #[test]
    fn clicks_low_sounds_and_audible_sounds_are_not_bats() {
        let mut c = classifier();
        let mut click = call(1.0, 22.0, 30.0);
        click.min_freq_hz = 15_000.0;
        assert_eq!(c.classify(&click).kind, CallKind::Broadband);
        assert_eq!(c.classify(&call(1.1, 18.0, 30.0)).kind, CallKind::TooLow);
        let mut bird = call(1.2, 26.0, 30.0);
        bird.audible_db_above_floor = 12.0;
        assert_eq!(c.classify(&bird).kind, CallKind::Audible);
        // None of them start a train.
        assert_eq!(c.classify(&call(1.3, 22.0, 30.0)).kind, CallKind::Single);
    }

    #[test]
    fn the_lowest_bat_frequency_is_settable() {
        let mut c = classifier();
        c.set_min_bat_hz(16_000.0);
        assert_eq!(c.classify(&call(1.0, 18.0, 30.0)).kind, CallKind::Single);
    }
}
