//! Time expansion (TE).
//!
//! A time-expansion detector records a short window of ultrasound and plays
//! it back slowed down, so a call at `f` is heard at `f / factor` and lasts
//! `factor` times as long. It keeps the whole spectrum and the amplitude
//! detail of a call, but it is not live: while a window plays back, new sound
//! is not captured.
//!
//! While idle, [`TimeExpander`] writes its input into the caller's storage as
//! a ring buffer, so a trigger can include sound from just before it. The
//! window plays back with 4-point cubic (Catmull-Rom) interpolation.
//!
//! Defaults: 50 ms before the trigger, 250 ms after it, and a factor of 10,
//! so a window plays for 3 s.

pub const DEFAULT_PRE_MS: f32 = 50.0;
pub const DEFAULT_POST_MS: f32 = 250.0;
pub const DEFAULT_FACTOR: f32 = 10.0;

/// Playback positions are fixed point with this many fraction bits.
const FRAC_BITS: u32 = 32;
const ONE: u64 = 1 << FRAC_BITS;

/// What a [`TimeExpander`] is doing, for display.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ExpanderState {
    /// Recording into the ring buffer and waiting for a trigger.
    Idle,
    /// Recording the part of the window after the trigger.
    Capturing,
    /// Playing the window back. `progress` goes from 0 to 1.
    Playing { progress: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    Capturing { remaining: usize },
    Playing,
}

/// Dynamic time expansion into caller-provided storage.
///
/// The output is silent while idle or capturing. During playback the input
/// is ignored and nothing is recorded, as on classic time-expansion
/// detectors.
#[derive(Debug)]
pub struct TimeExpander<'a> {
    sample_rate: f32,
    ring: &'a mut [f32],
    /// Ring index where the next input sample goes.
    write: usize,
    /// Samples recorded since the last playback ended (at most the ring
    /// length). Only these can be used as pre-trigger audio.
    filled: usize,
    phase: Phase,
    pre_ms: f32,
    post_ms: f32,
    factor: f32,
    /// Window samples per output sample, fixed point.
    step: u64,
    /// Ring index of the first window sample.
    start: usize,
    /// Window length in samples.
    len: usize,
    /// Playback position in window samples, fixed point.
    pos: u64,
    /// The four window samples around `pos`: one before it, the one at it and two after.
    /// The X8 keeps the ring in uncached SDRAM, where a read is slow, so playback reads the
    /// ring only when `pos` moves on to the next window sample.
    taps: [f32; 4],
}

impl<'a> TimeExpander<'a> {
    /// `storage` holds the ring buffer. Its length limits the window: at
    /// 192 kHz the default 300 ms window needs 57,600 samples.
    pub fn new(sample_rate: f32, storage: &'a mut [f32]) -> Self {
        let mut te = Self {
            sample_rate,
            ring: storage,
            write: 0,
            filled: 0,
            phase: Phase::Idle,
            pre_ms: DEFAULT_PRE_MS,
            post_ms: DEFAULT_POST_MS,
            factor: DEFAULT_FACTOR,
            step: 0,
            start: 0,
            len: 0,
            pos: 0,
            taps: [0.0; 4],
        };
        te.set_factor(DEFAULT_FACTOR);
        te
    }

    /// Audio to keep from before the trigger. Takes effect at the next
    /// trigger.
    pub fn set_pre_ms(&mut self, ms: f32) {
        self.pre_ms = ms.max(0.0);
    }

    pub fn pre_ms(&self) -> f32 {
        self.pre_ms
    }

    /// Audio to record after the trigger. Takes effect at the next trigger.
    pub fn set_post_ms(&mut self, ms: f32) {
        self.post_ms = ms.max(0.0);
    }

    pub fn post_ms(&self) -> f32 {
        self.post_ms
    }

    /// How many times slower playback is. At least 1; need not be a whole
    /// number. A change during playback takes effect at once.
    pub fn set_factor(&mut self, factor: f32) {
        self.factor = factor.max(1.0);
        self.step = ((ONE as f32 / self.factor) as u64).clamp(1, ONE);
    }

    pub fn factor(&self) -> f32 {
        self.factor
    }

    /// Start a capture. Ignored while capturing or playing.
    ///
    /// The window is `pre_ms` before the trigger and `post_ms` after it. If
    /// the storage cannot hold both, both are shortened in proportion. The
    /// pre-trigger part is also limited to the audio recorded since the last
    /// playback ended.
    pub fn trigger(&mut self) {
        if self.phase != Phase::Idle {
            return;
        }
        let (pre, post) = self.window();
        if pre + post == 0 {
            return;
        }
        let cap = self.ring.len();
        self.start = (self.write + cap - pre) % cap;
        self.len = pre + post;
        if post == 0 {
            self.start_playback();
        } else {
            self.phase = Phase::Capturing { remaining: post };
        }
    }

    pub fn state(&self) -> ExpanderState {
        match self.phase {
            Phase::Idle => ExpanderState::Idle,
            Phase::Capturing { .. } => ExpanderState::Capturing,
            Phase::Playing => {
                ExpanderState::Playing { progress: self.pos as f32 / ((self.len as u64) << FRAC_BITS) as f32 }
            }
        }
    }

    /// Length of the playback in output samples: for the current window
    /// while capturing or playing, or for a trigger made now while idle.
    pub fn playback_len(&self) -> usize {
        let len = match self.phase {
            Phase::Idle => {
                let (pre, post) = self.window();
                pre + post
            }
            _ => self.len,
        };
        let end = (len as u64) << FRAC_BITS;
        usize::try_from(end.div_ceil(self.step)).unwrap_or(usize::MAX)
    }

    /// Stop any capture or playback and forget the recorded audio.
    pub fn reset(&mut self) {
        self.phase = Phase::Idle;
        self.write = 0;
        self.filled = 0;
        self.pos = 0;
    }

    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        debug_assert_eq!(input.len(), output.len());
        let n = input.len().min(output.len());
        let mut i = 0;
        while i < n {
            match self.phase {
                Phase::Idle => {
                    self.record(&input[i..n]);
                    output[i..n].fill(0.0);
                    i = n;
                }
                Phase::Capturing { remaining } => {
                    let k = remaining.min(n - i);
                    self.record(&input[i..i + k]);
                    output[i..i + k].fill(0.0);
                    i += k;
                    if k == remaining {
                        self.start_playback();
                    } else {
                        self.phase = Phase::Capturing { remaining: remaining - k };
                    }
                }
                Phase::Playing => i += self.play(&mut output[i..n]),
            }
        }
    }

    /// Pre- and post-trigger lengths in samples for a trigger made now.
    fn window(&self) -> (usize, usize) {
        let cap = self.ring.len();
        let mut pre = self.ms_to_samples(self.pre_ms);
        let mut post = self.ms_to_samples(self.post_ms);
        let total = pre as u128 + post as u128;
        if total > cap as u128 {
            pre = (pre as u128 * cap as u128 / total) as usize;
            post = cap - pre;
        }
        (pre.min(self.filled), post)
    }

    fn ms_to_samples(&self, ms: f32) -> usize {
        (ms * self.sample_rate / 1000.0 + 0.5) as usize
    }

    fn start_playback(&mut self) {
        self.phase = Phase::Playing;
        self.pos = 0;
        self.taps = [self.window_sample(0), self.window_sample(0), self.window_sample(1), self.window_sample(2)];
    }

    fn record(&mut self, mut x: &[f32]) {
        let cap = self.ring.len();
        if cap == 0 {
            return;
        }
        self.filled = self.filled.saturating_add(x.len()).min(cap);
        while !x.is_empty() {
            let k = (cap - self.write).min(x.len());
            self.ring[self.write..self.write + k].copy_from_slice(&x[..k]);
            self.write += k;
            if self.write == cap {
                self.write = 0;
            }
            x = &x[k..];
        }
    }

    /// Fill `out` with playback until it is full or the window ends.
    /// Returns the number of samples written.
    fn play(&mut self, out: &mut [f32]) -> usize {
        let end = (self.len as u64) << FRAC_BITS;
        let mut k = 0;
        while k < out.len() && self.pos < end {
            out[k] = self.interpolate();
            let index = self.pos >> FRAC_BITS;
            self.pos += self.step;
            // `step` is at most one window sample, so `pos` moves on by one at most.
            if self.pos >> FRAC_BITS != index {
                let [_, x0, x1, x2] = self.taps;
                self.taps = [x0, x1, x2, self.window_sample(index as usize + 3)];
            }
            k += 1;
        }
        if self.pos >= end {
            self.phase = Phase::Idle;
            // The ring holds only audio from before this window, which a
            // later trigger must not use as pre-trigger audio.
            self.filled = 0;
        }
        k
    }

    /// The window at the playback position, by Catmull-Rom interpolation.
    /// Neighbours outside the window repeat its first or last sample.
    #[inline]
    fn interpolate(&self) -> f32 {
        let t = (self.pos as u32) as f32 * (1.0 / ONE as f32);
        let [xm1, x0, x1, x2] = self.taps;
        let c1 = 0.5 * (x1 - xm1);
        let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
        let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
        ((c3 * t + c2) * t + c1) * t + x0
    }

    /// Window sample `i`. Past the end, the last sample.
    #[inline]
    fn window_sample(&self, i: usize) -> f32 {
        let j = self.start + i.min(self.len - 1);
        let cap = self.ring.len();
        self.ring[if j >= cap { j - cap } else { j }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{noise, run_blocks, sine, zero_crossing_freq};
    use std::vec::Vec;

    const FS: f32 = 192_000.0;
    /// 50 ms and 250 ms at 192 kHz.
    const PRE: usize = 9_600;
    const POST: usize = 48_000;

    /// Run `x` through `te` in blocks of `block`, triggering after sample
    /// `at`.
    fn run_with_trigger(te: &mut TimeExpander, x: &[f32], block: usize, at: usize) -> Vec<f32> {
        let mut y = run_blocks(&x[..at], block, |i, o| te.process(i, o));
        te.trigger();
        y.extend(run_blocks(&x[at..], block, |i, o| te.process(i, o)));
        y
    }

    /// Run `n` samples of silence.
    fn run_silence(te: &mut TimeExpander, n: usize) -> Vec<f32> {
        let x = std::vec![0.0; n];
        let mut y = std::vec![0.0; n];
        te.process(&x, &mut y);
        y
    }

    /// Catmull-Rom at `pos`, reading all four samples from the ring.
    fn from_the_ring(te: &TimeExpander, pos: u64) -> f32 {
        let i = (pos >> FRAC_BITS) as usize;
        let t = (pos as u32) as f32 * (1.0 / ONE as f32);
        let last = te.len - 1;
        let at = |i: usize| te.ring[(te.start + i) % te.ring.len()];
        let (xm1, x0, x1, x2) = (at(i.saturating_sub(1)), at(i), at((i + 1).min(last)), at((i + 2).min(last)));
        let c1 = 0.5 * (x1 - xm1);
        let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
        let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
        ((c3 * t + c2) * t + c1) * t + x0
    }

    #[test]
    fn playback_matches_reading_the_ring_for_every_sample() {
        let x = noise(0.5, 40_000, 3);
        // The storage is smaller than the audio, so the window crosses the end of the ring.
        for (factor, block, change_to) in [
            (10.0, 256, None),
            (10.0, 7, None),
            (1.0, 64, None),
            (1.5, 33, None),
            (7.3, 256, None),
            (20.0, 100, Some(3.0)),
            (2.0, 100, Some(16.0)),
        ] {
            let mut storage = std::vec![0.0; 9_000];
            let mut te = TimeExpander::new(FS, &mut storage);
            te.set_factor(factor);
            te.set_pre_ms(10.0);
            te.set_post_ms(20.0);
            run_blocks(&x, 256, |i, o| te.process(i, o));
            te.trigger();
            run_blocks(&x[..3_840], 256, |i, o| te.process(i, o));
            assert!(matches!(te.state(), ExpanderState::Playing { .. }));
            let mut played = 0;
            let mut out = std::vec![0.0; block];
            while matches!(te.state(), ExpanderState::Playing { .. }) {
                if played == 1_000
                    && let Some(factor) = change_to
                {
                    te.set_factor(factor);
                }
                let (pos, step) = (te.pos, te.step);
                te.process(&std::vec![0.0; block], &mut out);
                let mut pos_now = pos;
                for &y in &out {
                    if pos_now >= (te.len as u64) << FRAC_BITS {
                        break;
                    }
                    let want = from_the_ring(&te, pos_now);
                    assert_eq!(y.to_bits(), want.to_bits(), "factor {factor}, block {block}, at {pos_now}");
                    pos_now += step;
                    played += 1;
                }
            }
            assert!(played > 5_000);
        }
    }

    #[test]
    fn a_50khz_burst_plays_back_at_5khz_and_10_times_longer() {
        let mut storage = std::vec![0.0; 76_800];
        let mut te = TimeExpander::new(FS, &mut storage);
        let at = 19_200;
        // A 10 ms burst that starts 5 ms before the trigger.
        let burst_start = at - 960;
        let burst_len = 1_920;
        let play_len = (PRE + POST) * 10;
        let mut x = std::vec![0.0; at + POST + play_len + 1_000];
        x[burst_start..burst_start + burst_len].copy_from_slice(&sine(FS, 50_000.0, 0.5, burst_len));
        let y = run_with_trigger(&mut te, &x, 512, at);
        assert_eq!(te.state(), ExpanderState::Idle);

        let loud: Vec<usize> = (0..y.len()).filter(|&i| y[i].abs() > 0.25).collect();
        let (first, last) = (loud[0], loud[loud.len() - 1]);
        let play_start = at + POST;
        let expect_first = play_start + (burst_start - (at - PRE)) * 10;
        assert!(first.abs_diff(expect_first) < 20, "burst starts at {first}, not {expect_first}");
        let heard_len = last - first;
        assert!(heard_len.abs_diff(burst_len * 10) < 40, "burst lasts {heard_len} samples");
        let f = zero_crossing_freq(&y[first + 1_000..last - 1_000], FS);
        assert!((f - 5_000.0).abs() < 5.0, "burst heard at {f} Hz");
    }

    #[test]
    fn pre_trigger_audio_is_included() {
        // At factor 1 the window plays back sample for sample.
        let mut storage = std::vec![0.0; 76_800];
        let mut te = TimeExpander::new(FS, &mut storage);
        te.set_factor(1.0);
        let at = 30_000;
        let x = noise(0.5, at + 2 * POST + PRE, 11);
        let y = run_with_trigger(&mut te, &x, 300, at);
        let play_start = at + POST;
        assert!(y[..play_start].iter().all(|&v| v == 0.0));
        assert_eq!(&y[play_start..play_start + PRE + POST], &x[at - PRE..at + POST]);
    }

    #[test]
    fn triggers_while_capturing_or_playing_are_ignored() {
        let mut storage = std::vec![0.0; 76_800];
        let mut te = TimeExpander::new(FS, &mut storage);
        run_silence(&mut te, PRE);
        te.trigger();
        run_silence(&mut te, POST / 2);
        te.trigger();
        assert_eq!(te.state(), ExpanderState::Capturing);
        run_silence(&mut te, POST / 2);
        let play_len = te.playback_len();
        assert_eq!(play_len, (PRE + POST) * 10);

        run_silence(&mut te, play_len / 2);
        match te.state() {
            ExpanderState::Playing { progress } => assert!((progress - 0.5).abs() < 1e-3),
            s => panic!("state {s:?}"),
        }
        te.trigger();
        assert_eq!(te.playback_len(), play_len);
        run_silence(&mut te, play_len / 2 - 1);
        assert!(matches!(te.state(), ExpanderState::Playing { .. }));
        run_silence(&mut te, 1);
        assert_eq!(te.state(), ExpanderState::Idle);

        // A trigger works again once playback has ended.
        te.trigger();
        assert_eq!(te.state(), ExpanderState::Capturing);
    }

    #[test]
    fn small_storage_shortens_the_window() {
        // 20 ms of storage for a 300 ms window: 1/6 before, 5/6 after.
        let cap = 3_840;
        let (pre, post) = (640, 3_200);
        let mut storage = std::vec![0.0; cap];
        let mut te = TimeExpander::new(FS, &mut storage);
        let at = 19_200;
        run_silence(&mut te, at);
        assert_eq!(te.playback_len(), cap * 10);

        te.set_factor(1.0);
        let x = noise(0.5, at + post + cap + 100, 5);
        let y = run_with_trigger(&mut te, &x, 1_000, at);
        assert_eq!(&y[at + post..at + post + cap], &x[at - pre..at + post]);
        assert!(y[at + post + cap..].iter().all(|&v| v == 0.0));
    }

    #[test]
    fn pre_trigger_uses_only_audio_recorded_while_idle() {
        let mut storage = std::vec![0.0; 76_800];
        let mut te = TimeExpander::new(FS, &mut storage);
        // Only 10 ms recorded so far.
        run_silence(&mut te, 1_920);
        assert_eq!(te.playback_len(), (1_920 + POST) * 10);
        te.trigger();
        run_silence(&mut te, POST + (1_920 + POST) * 10);
        assert_eq!(te.state(), ExpanderState::Idle);
        // Straight after playback, nothing has been recorded.
        assert_eq!(te.playback_len(), POST * 10);
    }

    #[test]
    fn empty_storage_never_plays() {
        let mut storage: [f32; 0] = [];
        let mut te = TimeExpander::new(FS, &mut storage);
        te.trigger();
        assert_eq!(te.state(), ExpanderState::Idle);
        assert_eq!(te.playback_len(), 0);
        let y = run_silence(&mut te, 100);
        assert!(y.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn block_size_does_not_change_the_output() {
        let at = 12_000;
        let mut x = noise(0.05, at + 200_000, 9);
        for (xi, s) in x[at - 2_000..].iter_mut().zip(sine(FS, 45_000.0, 0.4, 4_000)) {
            *xi += s;
        }
        let outputs: Vec<Vec<f32>> = [1, 37, 4096]
            .iter()
            .map(|&block| {
                let mut storage = std::vec![0.0; 20_000];
                let mut te = TimeExpander::new(FS, &mut storage);
                te.set_post_ms(50.0);
                te.set_factor(7.3);
                run_with_trigger(&mut te, &x, block, at)
            })
            .collect();
        assert!(outputs[0].iter().any(|&v| v != 0.0));
        assert_eq!(outputs[0], outputs[1]);
        assert_eq!(outputs[0], outputs[2]);
    }
}
