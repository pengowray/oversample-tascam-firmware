//! Bat call detection on spectrogram columns.
//!
//! [`CallDetector`] takes the [`Column`]s of a
//! [`SpectrumAnalyzer`](crate::spectrum::SpectrumAnalyzer) with the same `N`
//! and reports each call it finds as a [`CallEvent`].
//!
//! For each bin it keeps a noise floor: the bin's average level in dB plus its
//! average deviation from that level. A bin whose level jumps around, as
//! noise does, gets a floor well above its average; a bin with a steady
//! level, such as a tone, gets a floor just above it. The floor adapts slowly
//! while no call is in progress, so a steady tone (such as 38 kHz electrical
//! hum) or steady insect noise raises the floor in its own bins and stops
//! counting as a call.
//!
//! Each bin's excess over its floor is averaged with its two neighbours. This
//! evens out random noise peaks and barely changes an FM sweep, whose energy
//! spreads over many bins; a pure tone reads about 4 dB lower.
//!
//! A column is loud when that averaged excess is above the threshold (default
//! 12 dB) somewhere in the band of interest (default 15 to 95 kHz). A call
//! starts on a loud column and continues while columns stay above the lower
//! release threshold (default 9 dB). Calls separated by less than the merge
//! gap (default 2 ms) are joined into one, and calls shorter than the minimum
//! (default 1 ms) are dropped.
//!
//! A sound that keeps a call going for longer than the maximum call length
//! (default 100 ms), such as a tone that switches on, is treated as
//! background: it is not reported, and the floor adapts to it.
//!
//! Times are column centres. The FFT window spreads each call in time, so at
//! high signal-to-noise ratios a call can start up to half a window early and
//! end up to half a window late (1.3 ms each for `N = 512` at 192 kHz).
//!
//! A call's frequency range is measured from its peak: it covers the bins
//! that came within 20 dB of the peak (settable). Measuring from the
//! threshold instead would let the FFT window's side lobes, and the spread
//! of an abrupt start or end, widen a loud call's range far beyond its own
//! frequencies.
//!
//! The first [`SETTLE_MS`] after construction or [`CallDetector::reset`] are
//! used only to learn the floor.

use crate::spectrum::{Column, FftFrame};

pub const DEFAULT_BAND_LO_HZ: f32 = 15_000.0;
pub const DEFAULT_BAND_HI_HZ: f32 = 95_000.0;
pub const DEFAULT_THRESHOLD_DB: f32 = 12.0;
pub const DEFAULT_RELEASE_DB: f32 = 9.0;
pub const DEFAULT_MIN_CALL_MS: f32 = 1.0;
pub const DEFAULT_MERGE_GAP_MS: f32 = 2.0;
pub const DEFAULT_MAX_CALL_MS: f32 = 100.0;
pub const DEFAULT_FLOOR_TIME_S: f32 = 0.5;
/// Bins more than this far below the call's peak are left out of its
/// frequency range.
pub const DEFAULT_RANGE_DB: f32 = 20.0;

/// The frequency range is tracked at levels this far apart above the
/// threshold, so the range for any cutoff can be picked when the call ends.
const RANGE_STEP_DB: f32 = 3.0;
const RANGE_LEVELS: usize = 24;

/// Time after construction or reset during which the detector only learns
/// the floor and reports nothing.
pub const SETTLE_MS: f32 = 50.0;

/// Smallest deviation the floor allows for, in dB, so a bin with a very
/// steady level does not get a floor right at that level.
const MIN_SPREAD_DB: f32 = 0.5;

/// A detected call. Sample indices are the analyzer's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CallEvent {
    /// Centre of the first loud column.
    pub start_sample: u64,
    /// Centre of the last column above the release threshold.
    pub end_sample: u64,
    /// Frequency where the call rose highest above the floor, interpolated
    /// between bins.
    pub peak_freq_hz: f32,
    /// How far the call rose above the floor at that frequency, in dB.
    pub peak_db_above_floor: f32,
    /// Lowest bin frequency that came within the range setting (default
    /// 20 dB) of the call's peak, and above the threshold, during the call.
    pub min_freq_hz: f32,
    /// Highest bin frequency that came within the range setting of the call's
    /// peak, and above the threshold, during the call.
    pub max_freq_hz: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    /// Learning the floor until this sample; nothing is detected.
    Settling { until: u64 },
    Idle,
    /// Call in progress; the last column was above the release threshold.
    Active,
    /// Call in progress; columns have been below the release threshold since
    /// the call's `end`.
    Gap,
    /// Loud for longer than a call lasts; the floor adapts until the band is
    /// below the release threshold.
    Background,
}

/// The call in progress.
#[derive(Clone, Copy, Debug)]
struct Call {
    start: u64,
    /// Centre of the last column above the release threshold.
    end: u64,
    peak_db: f32,
    peak_freq_hz: f32,
    /// Lowest and highest bins whose averaged excess reached
    /// `threshold_db + level * RANGE_STEP_DB`, for each level, in any column
    /// of the call. `usize::MAX` and 0 when none did.
    lo_bin: [usize; RANGE_LEVELS],
    hi_bin: [usize; RANGE_LEVELS],
}

impl Call {
    const fn starting_at(t: u64) -> Self {
        Self {
            start: t,
            end: t,
            peak_db: f32::MIN,
            peak_freq_hz: 0.0,
            lo_bin: [usize::MAX; RANGE_LEVELS],
            hi_bin: [0; RANGE_LEVELS],
        }
    }
}

/// What one column shows in the band.
struct Scan {
    /// Highest averaged excess over the floor, in dB, and its bin.
    peak_db: f32,
    peak_bin: usize,
}

/// Detects bat calls in the columns of an `N`-point
/// [`SpectrumAnalyzer`](crate::spectrum::SpectrumAnalyzer).
#[derive(Clone, Debug)]
pub struct CallDetector<const N: usize>
where
    [f32; N]: FftFrame,
{
    sample_rate: f32,
    bin_hz: f32,
    band_hz: (f32, f32),
    band_bins: (usize, usize),
    threshold_db: f32,
    release_db: f32,
    range_db: f32,
    min_call_ms: f32,
    min_call: u64,
    merge_gap_ms: f32,
    merge_gap: u64,
    max_call_ms: f32,
    max_call: u64,
    settle: u64,
    floor_time_s: f32,
    /// Floor adaptation rate per sample: 1 / (time constant × sample rate).
    floor_rate: f32,
    /// Average level of each bin, in dB.
    mean: <[f32; N] as FftFrame>::Bins,
    /// Average absolute deviation from `mean`, in dB.
    spread: <[f32; N] as FftFrame>::Bins,
    floor_updates: u32,
    last_centre: Option<u64>,
    state: State,
    call: Call,
}

impl<const N: usize> CallDetector<N>
where
    [f32; N]: FftFrame,
{
    /// A detector with the default settings.
    ///
    /// This builds the detector (about 8 KB for `N = 1024`) on the stack and
    /// returns it. On a microcontroller, use
    /// [`unconfigured`](Self::unconfigured) in a `static` and call
    /// [`init`](Self::init) on it instead.
    pub fn new(sample_rate: f32) -> Self {
        let mut det = Self::unconfigured();
        det.init(sample_rate);
        det
    }

    /// A detector with no sample rate yet, for a `static`, so it is never
    /// copied through the stack. Call [`init`](Self::init) before use;
    /// [`process`](Self::process) reports nothing until then.
    pub const fn unconfigured() -> Self {
        Self {
            sample_rate: 0.0,
            bin_hz: 0.0,
            band_hz: (0.0, 0.0),
            band_bins: (0, 0),
            threshold_db: DEFAULT_THRESHOLD_DB,
            release_db: DEFAULT_RELEASE_DB,
            range_db: DEFAULT_RANGE_DB,
            min_call_ms: 0.0,
            min_call: 0,
            merge_gap_ms: 0.0,
            merge_gap: 0,
            max_call_ms: 0.0,
            max_call: 0,
            settle: 0,
            floor_time_s: 0.0,
            floor_rate: 0.0,
            mean: <[f32; N] as FftFrame>::SILENT_BINS,
            spread: <[f32; N] as FftFrame>::SILENT_BINS,
            floor_updates: 0,
            last_centre: None,
            state: State::Idle,
            call: Call::starting_at(0),
        }
    }

    /// Set the sample rate, restore the default settings and clear the noise
    /// floor, in place.
    pub fn init(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.bin_hz = sample_rate / N as f32;
        self.threshold_db = DEFAULT_THRESHOLD_DB;
        self.release_db = DEFAULT_RELEASE_DB;
        self.range_db = DEFAULT_RANGE_DB;
        self.settle = self.ms_to_samples(SETTLE_MS);
        self.set_band(DEFAULT_BAND_LO_HZ, DEFAULT_BAND_HI_HZ);
        self.set_min_call_ms(DEFAULT_MIN_CALL_MS);
        self.set_merge_gap_ms(DEFAULT_MERGE_GAP_MS);
        self.set_max_call_ms(DEFAULT_MAX_CALL_MS);
        self.set_floor_time(DEFAULT_FLOOR_TIME_S);
        self.reset();
    }

    /// Frequency range searched for calls, in Hz. Bins outside it still keep
    /// their floors, so the band can change without relearning.
    pub fn set_band(&mut self, lo_hz: f32, hi_hz: f32) {
        let (lo_hz, hi_hz) = if lo_hz <= hi_hz { (lo_hz, hi_hz) } else { (hi_hz, lo_hz) };
        self.band_hz = (lo_hz, hi_hz);
        // `as usize` saturates, so negative frequencies give bin 0.
        let bin = |hz: f32| ((hz / self.bin_hz + 0.5) as usize).min(N / 2);
        self.band_bins = (bin(lo_hz), bin(hi_hz));
    }

    pub fn band(&self) -> (f32, f32) {
        self.band_hz
    }

    /// dB above the floor that starts a call.
    pub fn set_threshold_db(&mut self, db: f32) {
        self.threshold_db = db;
    }

    pub fn threshold_db(&self) -> f32 {
        self.threshold_db
    }

    /// dB above the floor that keeps a call going once started. Set it below
    /// the threshold.
    pub fn set_release_db(&mut self, db: f32) {
        self.release_db = db;
    }

    pub fn release_db(&self) -> f32 {
        self.release_db
    }

    /// How far below the call's peak, in dB, a bin can be and still count
    /// towards the call's frequency range (default 20 dB). Measured in 3 dB
    /// steps, rounding towards the peak.
    pub fn set_range_db(&mut self, db: f32) {
        self.range_db = db.max(0.0);
    }

    pub fn range_db(&self) -> f32 {
        self.range_db
    }

    /// Calls shorter than this, from `start_sample` to `end_sample`, are
    /// dropped.
    pub fn set_min_call_ms(&mut self, ms: f32) {
        self.min_call_ms = ms;
        self.min_call = self.ms_to_samples(ms);
    }

    pub fn min_call_ms(&self) -> f32 {
        self.min_call_ms
    }

    /// Quiet gaps shorter than this join the calls on either side. A call is
    /// reported once this long has passed after its `end_sample`.
    pub fn set_merge_gap_ms(&mut self, ms: f32) {
        self.merge_gap_ms = ms;
        self.merge_gap = self.ms_to_samples(ms);
    }

    pub fn merge_gap_ms(&self) -> f32 {
        self.merge_gap_ms
    }

    /// Sounds loud for longer than this are treated as background, not calls.
    pub fn set_max_call_ms(&mut self, ms: f32) {
        self.max_call_ms = ms;
        self.max_call = self.ms_to_samples(ms);
    }

    pub fn max_call_ms(&self) -> f32 {
        self.max_call_ms
    }

    /// Time constant of the floor's adaptation, in seconds (at least 1 ms).
    pub fn set_floor_time(&mut self, seconds: f32) {
        self.floor_time_s = seconds.max(0.001);
        self.floor_rate = 1.0 / (self.floor_time_s * self.sample_rate);
    }

    pub fn floor_time(&self) -> f32 {
        self.floor_time_s
    }

    /// Forget the floor and any call in progress, and settle again.
    pub fn reset(&mut self) {
        self.mean.as_mut().fill(0.0);
        self.spread.as_mut().fill(MIN_SPREAD_DB);
        self.floor_updates = 0;
        self.last_centre = None;
        self.state = State::Settling { until: 0 };
    }

    /// True from the first loud column of a call until the call ends,
    /// including short gaps that may still be merged. A call that turns out
    /// shorter than the minimum is active while it lasts but not reported.
    pub fn is_active(&self) -> bool {
        matches!(self.state, State::Active | State::Gap)
    }

    /// Start sample of the call in progress.
    pub fn active_start(&self) -> Option<u64> {
        self.is_active().then_some(self.call.start)
    }

    /// The floor for one bin, in dB.
    pub fn floor_db(&self, bin: usize) -> f32 {
        self.mean.as_ref()[bin] + self.spread.as_ref()[bin]
    }

    /// Feed the next column. Returns a call that has just ended, if any.
    ///
    /// A call ends once the merge gap has passed after its `end_sample`, so
    /// it is reported that much later.
    pub fn process(&mut self, column: &Column<'_>) -> Option<CallEvent> {
        if self.sample_rate == 0.0 {
            return None;
        }
        let db = column.db;
        debug_assert_eq!(db.len(), N / 2 + 1);
        let t = column.centre_sample;
        let dt = match self.last_centre {
            Some(prev) => t.saturating_sub(prev),
            None => {
                self.state = State::Settling { until: t + self.settle };
                0
            }
        };
        self.last_centre = Some(t);

        let scan = self.scan(db);
        let mut event = None;
        match self.state {
            State::Settling { until } => {
                self.adapt(db, dt);
                if t >= until {
                    self.state = State::Idle;
                }
            }
            State::Idle => self.idle(db, &scan, t, dt),
            State::Active => {
                if t.saturating_sub(self.call.start) > self.max_call {
                    self.state = State::Background;
                    self.adapt(db, dt);
                } else if scan.peak_db > self.release_db {
                    self.extend(db, &scan, t);
                } else {
                    self.state = State::Gap;
                }
            }
            State::Gap => {
                let since_end = t.saturating_sub(self.call.end);
                // The quiet time before this column is since_end - dt.
                let quiet = since_end.saturating_sub(dt);
                if scan.peak_db > self.threshold_db && quiet < self.merge_gap {
                    self.extend(db, &scan, t);
                    self.state = State::Active;
                } else if since_end >= self.merge_gap {
                    event = self.finish();
                    self.idle(db, &scan, t, dt);
                }
            }
            State::Background => {
                self.adapt(db, dt);
                if scan.peak_db <= self.release_db {
                    self.state = State::Idle;
                }
            }
        }
        event
    }

    /// End a call in progress now (at the end of a recording, say) and
    /// return it if it is long enough.
    pub fn flush(&mut self) -> Option<CallEvent> {
        if self.is_active() { self.finish() } else { None }
    }

    fn ms_to_samples(&self, ms: f32) -> u64 {
        (ms.max(0.0) * 0.001 * self.sample_rate) as u64
    }

    /// Find the band's peak excess over the floor, averaged over 3 bins.
    fn scan(&self, db: &[f32]) -> Scan {
        let mut scan = Scan { peak_db: f32::MIN, peak_bin: self.band_bins.0 };
        self.for_each_excess(db, |k, e| {
            if e > scan.peak_db {
                scan.peak_db = e;
                scan.peak_bin = k;
            }
        });
        scan
    }

    /// Call `f(bin, excess)` for each bin in the band, with each bin's excess
    /// over its floor averaged with its two neighbours.
    fn for_each_excess(&self, db: &[f32], mut f: impl FnMut(usize, f32)) {
        let mean = self.mean.as_ref();
        let spread = self.spread.as_ref();
        let last = N / 2;
        let excess = |k: usize| db[k] - mean[k] - spread[k];
        let (lo, hi) = self.band_bins;
        let mut prev = excess(lo.saturating_sub(1));
        let mut cur = excess(lo);
        for k in lo..=hi {
            let next = excess((k + 1).min(last));
            f(k, (prev + cur + next) * (1.0 / 3.0));
            prev = cur;
            cur = next;
        }
    }

    /// Move the floor towards this column. Starts as a running average, so
    /// the first columns settle it quickly, then becomes an exponential
    /// average with the floor time constant.
    fn adapt(&mut self, db: &[f32], dt: u64) {
        if self.floor_updates == 0 {
            self.mean.as_mut().copy_from_slice(db);
        } else {
            let running = 1.0 / (self.floor_updates as f32 + 1.0);
            let rate = running.max(dt as f32 * self.floor_rate).min(1.0);
            let pairs = self.mean.as_mut().iter_mut().zip(self.spread.as_mut());
            for ((m, s), &x) in pairs.zip(db) {
                let d = x - *m;
                *m += rate * d;
                *s = (*s + rate * (d.abs() - *s)).max(MIN_SPREAD_DB);
            }
        }
        self.floor_updates = self.floor_updates.saturating_add(1);
    }

    /// Handle a column with no call in progress.
    fn idle(&mut self, db: &[f32], scan: &Scan, t: u64, dt: u64) {
        self.state = State::Idle;
        if scan.peak_db > self.threshold_db {
            self.call = Call::starting_at(t);
            self.extend(db, scan, t);
            self.state = State::Active;
        } else {
            self.adapt(db, dt);
        }
    }

    /// Add a column above the release threshold to the call in progress.
    ///
    /// The frequency range is recorded at several levels, because which level
    /// to report depends on the call's peak, which is known only at the end.
    /// Measuring from the call's peak keeps a loud call's range to its own
    /// frequencies: the FFT window's side lobes, and the spread of an abrupt
    /// start or end, clear the threshold far from them, but stay well below
    /// the peak.
    fn extend(&mut self, db: &[f32], scan: &Scan, t: u64) {
        let mut call = self.call;
        call.end = t;
        if scan.peak_db > call.peak_db {
            call.peak_db = scan.peak_db;
            call.peak_freq_hz = peak_position(db, scan.peak_bin) * self.bin_hz;
        }
        let threshold = self.threshold_db;
        self.for_each_excess(db, |k, e| {
            if e > threshold {
                let top = (((e - threshold) / RANGE_STEP_DB) as usize).min(RANGE_LEVELS - 1);
                for level in 0..=top {
                    call.lo_bin[level] = call.lo_bin[level].min(k);
                    call.hi_bin[level] = call.hi_bin[level].max(k);
                }
            }
        });
        self.call = call;
    }

    fn finish(&mut self) -> Option<CallEvent> {
        self.state = State::Idle;
        let c = self.call;
        // The lowest level at or above (peak - range_db), and at most the
        // highest level any bin reached.
        let cutoff = (c.peak_db - self.range_db - self.threshold_db).max(0.0);
        let mut level = ((libm::ceilf(cutoff / RANGE_STEP_DB)) as usize).min(RANGE_LEVELS - 1);
        while level > 0 && c.lo_bin[level] == usize::MAX {
            level -= 1;
        }
        (c.end.saturating_sub(c.start) >= self.min_call).then_some(CallEvent {
            start_sample: c.start,
            end_sample: c.end,
            peak_freq_hz: c.peak_freq_hz,
            peak_db_above_floor: c.peak_db,
            min_freq_hz: c.lo_bin[level] as f32 * self.bin_hz,
            max_freq_hz: c.hi_bin[level] as f32 * self.bin_hz,
        })
    }
}

/// Position of a peak near bin `k`, in bins, from a parabola through the dB
/// values of `k` and its neighbours. Returns `k` if it is not a local maximum.
fn peak_position(db: &[f32], k: usize) -> f32 {
    if k == 0 || k + 1 >= db.len() {
        return k as f32;
    }
    let (a, b, c) = (db[k - 1], db[k], db[k + 1]);
    let curve = a - 2.0 * b + c;
    if b < a || b < c || curve >= 0.0 {
        return k as f32;
    }
    k as f32 + (0.5 * (a - c) / curve).clamp(-0.5, 0.5)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::biquad::Butterworth4;
    use crate::spectrum::SpectrumAnalyzer;
    use crate::test_util::{chirp, noise, sine};
    use std::vec::Vec;

    const FS: f32 = 192_000.0;
    const N: usize = 512;

    fn ms(t: f32) -> usize {
        (t * 0.001 * FS) as usize
    }

    /// Run `x` through a 512-point analyzer and `det` in 480-sample blocks.
    fn detect(det: &mut CallDetector<N>, x: &[f32]) -> Vec<CallEvent> {
        let mut an = SpectrumAnalyzer::<N>::new(FS);
        let mut events = Vec::new();
        for block in x.chunks(480) {
            an.push(block, |c| events.extend(det.process(c)));
        }
        events.extend(det.flush());
        events
    }

    fn add(x: &mut [f32], at: usize, s: &[f32]) {
        for (a, b) in x[at..].iter_mut().zip(s) {
            *a += b;
        }
    }

    /// 80 kHz down to 40 kHz in 5 ms.
    fn fm_call(amp: f32) -> Vec<f32> {
        chirp(FS, 80_000.0, 40_000.0, amp, ms(5.0))
    }

    fn assert_near(sample: u64, expect: usize, what: &str) {
        let err_ms = (sample as f32 - expect as f32) / FS * 1000.0;
        assert!(err_ms.abs() <= 1.0, "{what} off by {err_ms} ms");
    }

    #[test]
    fn finds_an_fm_sweep_in_noise_once_with_its_times_and_range() {
        // Call at -20 dBFS in noise at -35 dBFS RMS.
        let mut x = noise(0.03, ms(300.0), 1);
        let t0 = ms(150.0);
        add(&mut x, t0, &fm_call(0.1));
        let events = detect(&mut CallDetector::new(FS), &x);
        assert_eq!(events.len(), 1, "{events:?}");
        let e = events[0];
        assert_near(e.start_sample, t0, "start");
        assert_near(e.end_sample, t0 + ms(5.0), "end");
        assert!((35_000.0..=45_000.0).contains(&e.min_freq_hz), "{e:?}");
        assert!((75_000.0..=85_000.0).contains(&e.max_freq_hz), "{e:?}");
        assert!((40_000.0..=80_000.0).contains(&e.peak_freq_hz), "{e:?}");
        assert!(e.peak_db_above_floor > DEFAULT_THRESHOLD_DB, "{e:?}");
    }

    #[test]
    fn a_loud_abrupt_call_reports_its_own_frequency_range() {
        // 42 dB above the floor, with no fade in or out: the FFT window's side
        // lobes clear the threshold well outside 40 to 80 kHz.
        let mut x = noise(0.003, ms(300.0), 3);
        let t0 = ms(150.0);
        add(&mut x, t0, &fm_call(0.1));
        let events = detect(&mut CallDetector::new(FS), &x);
        assert_eq!(events.len(), 1, "{events:?}");
        let e = events[0];
        assert!((35_000.0..=45_000.0).contains(&e.min_freq_hz), "{e:?}");
        assert!((75_000.0..=85_000.0).contains(&e.max_freq_hz), "{e:?}");
    }

    #[test]
    fn white_noise_alone_gives_no_events() {
        for (amp, seed) in [(0.03, 7), (0.3, 8)] {
            let events = detect(&mut CallDetector::new(FS), &noise(amp, ms(2000.0), seed));
            assert!(events.is_empty(), "noise {amp}: {events:?}");
        }
    }

    #[test]
    fn steady_38khz_tone_is_absorbed_and_a_call_on_top_is_found() {
        let len = ms(1500.0);
        let mut x = noise(0.03, len, 3);
        add(&mut x, 0, &sine(FS, 38_000.0, 0.1, len));
        let t0 = ms(1200.0);
        add(&mut x, t0, &fm_call(0.1));
        let events = detect(&mut CallDetector::new(FS), &x);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_near(events[0].start_sample, t0, "start");
        assert!(events[0].min_freq_hz > 38_500.0, "{:?}", events[0]);
    }

    #[test]
    fn tone_switching_on_is_treated_as_background() {
        let len = ms(2000.0);
        let mut x = noise(0.03, len, 4);
        let on = ms(300.0);
        add(&mut x, on, &sine(FS, 38_000.0, 0.3, len - on));
        let t0 = ms(1500.0);
        add(&mut x, t0, &fm_call(0.1));
        let mut det = CallDetector::new(FS);
        let events = detect(&mut det, &x);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_near(events[0].start_sample, t0, "start");
        // The tone's bins now have a floor near the tone's level.
        let tone_bin = (38_000.0 / (FS / N as f32) + 0.5) as usize;
        assert!(det.floor_db(tone_bin) > -16.0, "floor {}", det.floor_db(tone_bin));
    }

    #[test]
    fn steady_insect_band_noise_is_absorbed() {
        // Loud noise from 20 to 35 kHz, all the time.
        let len = ms(1500.0);
        let mut band = noise(0.5, len, 5);
        let mut hp = Butterworth4::highpass(FS, 20_000.0);
        let mut lp = Butterworth4::lowpass(FS, 35_000.0);
        hp.process_in_place(&mut band);
        lp.process_in_place(&mut band);
        let mut x = noise(0.03, len, 6);
        add(&mut x, 0, &band);
        let t0 = ms(1000.0);
        add(&mut x, t0, &chirp(FS, 80_000.0, 50_000.0, 0.1, ms(5.0)));
        let events = detect(&mut CallDetector::new(FS), &x);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_near(events[0].start_sample, t0, "start");
    }

    #[test]
    fn two_calls_20_ms_apart_give_two_events() {
        let mut x = noise(0.03, ms(300.0), 9);
        let (t0, t1) = (ms(150.0), ms(170.0));
        add(&mut x, t0, &fm_call(0.1));
        add(&mut x, t1, &fm_call(0.1));
        let events = detect(&mut CallDetector::new(FS), &x);
        assert_eq!(events.len(), 2, "{events:?}");
        assert_near(events[0].start_sample, t0, "first start");
        assert_near(events[1].start_sample, t1, "second start");
    }

    #[test]
    fn calls_closer_than_the_merge_gap_are_joined() {
        // Two 3 ms pulses with 6 ms between them.
        let pulse = chirp(FS, 70_000.0, 60_000.0, 0.1, ms(3.0));
        let mut x = noise(0.03, ms(300.0), 10);
        let (t0, t1) = (ms(150.0), ms(159.0));
        add(&mut x, t0, &pulse);
        add(&mut x, t1, &pulse);

        assert_eq!(detect(&mut CallDetector::new(FS), &x).len(), 2);

        let mut det = CallDetector::new(FS);
        det.set_merge_gap_ms(10.0);
        let events = detect(&mut det, &x);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_near(events[0].start_sample, t0, "start");
        assert_near(events[0].end_sample, t1 + ms(3.0), "end");
    }

    #[test]
    fn calls_shorter_than_the_minimum_are_dropped() {
        let mut x = noise(0.03, ms(300.0), 11);
        add(&mut x, ms(150.0), &fm_call(0.1));
        let mut det = CallDetector::new(FS);
        det.set_min_call_ms(10.0);
        assert!(detect(&mut det, &x).is_empty());
    }

    #[test]
    fn calls_outside_the_band_are_ignored() {
        let mut x = noise(0.03, ms(300.0), 12);
        add(&mut x, ms(150.0), &fm_call(0.1));
        let mut det = CallDetector::new(FS);
        det.set_band(15_000.0, 35_000.0);
        assert!(detect(&mut det, &x).is_empty());
    }

    #[test]
    fn is_active_from_the_first_loud_column_until_the_call_ends() {
        let mut x = noise(0.03, ms(300.0), 13);
        let t0 = ms(150.0);
        add(&mut x, t0, &fm_call(0.1));
        let mut an = SpectrumAnalyzer::<N>::new(FS);
        let mut det = CallDetector::<N>::new(FS);
        let mut active = Vec::new();
        let mut events = Vec::new();
        an.push(&x, |c| {
            events.extend(det.process(c));
            active.push((c.centre_sample, det.active_start()));
        });
        assert_eq!(events.len(), 1);
        let e = events[0];
        let first = active.iter().find(|(_, a)| a.is_some()).unwrap();
        assert_eq!(first.0, e.start_sample);
        assert_eq!(first.1, Some(e.start_sample));
        // Active until the merge gap after the end has passed, then not.
        let last = active.iter().rev().find(|(_, a)| a.is_some()).unwrap();
        assert!(last.0 >= e.end_sample);
        assert!(last.0 < e.end_sample + ms(DEFAULT_MERGE_GAP_MS) as u64);
    }
}
