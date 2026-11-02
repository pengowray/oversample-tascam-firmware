//! The bat app for the TASCAM Portacapture X8: listening modes, call
//! detection, and its screen.
//!
//! [`BatApp`] takes the recorder's inputs 3/4 in blocks, makes the audio for
//! the headphones in the chosen listening mode, finds bat calls, and draws a
//! 320 × 480 screen (the X8's panel, used in portrait like the stock apps)
//! into an RGB565 framebuffer. It knows nothing about the hardware: the X8
//! firmware, or the web emulator in `crates/bat-app-web`, feeds it audio and
//! button, wheel and touch events, and copies the framebuffer to a display.
//!
//! `no_std` and no allocation. The time expander's storage is passed in.

#![no_std]

#[cfg(test)]
extern crate std;

pub mod fb;
mod layout;
pub mod palette;
mod screen;

use bat_dsp::detector::{CallDetector, CallEvent};
use bat_dsp::freq_div::FrequencyDivider;
use bat_dsp::heterodyne::{DEFAULT_COMB_SPACING_HZ, DEFAULT_CUTOFF_HZ, HetComb, Heterodyne};
use bat_dsp::pitch_shift::PitchShifter;
use bat_dsp::spectrum::{Column, SpectrumAnalyzer};
use bat_dsp::time_expansion::{ExpanderState, TimeExpander};

pub use fb::Rgb565Buffer;
use layout::{SNAP_H, SNAP_W, WF_H, WF_W};

/// Screen size in pixels.
pub const WIDTH: usize = 320;
pub const HEIGHT: usize = 480;

/// Frequency range of the waterfall and the call snapshot.
pub const VIEW_LO_HZ: f32 = 15_000.0;
pub const VIEW_HI_HZ: f32 = 96_000.0;

/// Time covered by one waterfall pixel column.
const WF_COLUMN_MS: f32 = 10.0;
/// Range of levels shown, in dB above the display floor.
const DISPLAY_RANGE_DB: f32 = 50.0;
/// Lowest display floor. Without it, digital silence (-200 dB) pulls the
/// floor down until the silence itself shows as faint colour.
const MIN_FLOOR_DB: f32 = -140.0;
/// Spectrum for the waterfall and the call detector: 512-point FFT (375 Hz
/// bins at 192 kHz) every 128 samples.
const FFT_N: usize = 512;
const FFT_HOP: usize = 128;
/// Detector columns kept for call snapshots: 256 × 0.67 ms = 170 ms, longer
/// than the longest call the detector reports (100 ms).
const HISTORY: usize = 256;
/// Audio is processed in pieces of this many samples.
const CHUNK: usize = 256;
/// Margin shown either side of a call in its snapshot.
const SNAP_MARGIN_MS: f32 = 2.0;

const HET_MIN_HZ: f32 = 15_000.0;
const HET_MAX_HZ: f32 = 95_000.0;
const HET_STEP_HZ: f32 = 500.0;
const COMB_COUNT: usize = 3;
const COMB_STEP_HZ: f32 = 1_000.0;
const FD_DIVISIONS: [u32; 5] = [4, 8, 10, 16, 20];
const TE_FACTORS: [f32; 6] = [5.0, 8.0, 10.0, 16.0, 20.0, 32.0];
const PITCH_FACTORS: [f32; 6] = [4.0, 8.0, 10.0, 16.0, 20.0, 32.0];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Heterodyne: a call at f is heard at |f - tuned frequency|.
    Het,
    /// Three heterodynes spread across the band, mixed.
    Comb,
    /// Frequency division: zero crossings divided down.
    Fd,
    /// Time expansion: a detected call is captured and played back slower.
    Te,
    /// Pitch shift: frequencies divided, at real-time speed.
    Pitch,
}

impl Mode {
    pub const ALL: [Mode; 5] = [Mode::Het, Mode::Comb, Mode::Fd, Mode::Te, Mode::Pitch];

    /// Button label. HET, FD and TE are the usual names on bat detectors.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Het => "HET",
            Mode::Comb => "COMB",
            Mode::Fd => "FD",
            Mode::Te => "TE",
            Mode::Pitch => "PITCH",
        }
    }
}

/// The X8's hardware buttons that the app uses.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    /// Returns to the X8's launcher. Handled outside the app.
    Home,
    /// Playback. Handled outside the app (the emulator plays its file).
    Play,
    /// Start or stop recording.
    Rec,
    /// Add a mark; in TE mode also captures a window now.
    Mark,
}

/// Which input feeds the headphones and the detector.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MonitorSource {
    Input3,
    Input4,
    /// The average of inputs 3 and 4.
    Mix,
}

/// Time expansion state, for the screen.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TeStatus {
    Waiting,
    Capturing,
    Playing { progress: f32 },
}

/// A short list of recent sample times (calls, marks), oldest overwritten.
#[derive(Clone, Copy)]
struct Recent {
    times: [u64; 16],
    len: usize,
    next: usize,
}

impl Recent {
    const fn new() -> Self {
        Self { times: [0; 16], len: 0, next: 0 }
    }

    fn push(&mut self, t: u64) {
        self.times[self.next] = t;
        self.next = (self.next + 1) % self.times.len();
        self.len = (self.len + 1).min(self.times.len());
    }

    fn iter(&self) -> impl Iterator<Item = u64> + '_ {
        self.times[..self.len].iter().copied()
    }
}

/// Input level meter for one channel, in dBFS.
#[derive(Clone, Copy)]
struct Meter {
    level_db: f32,
    hold_db: f32,
    hold_left: u32,
}

impl Meter {
    const FLOOR_DB: f32 = -90.0;

    const fn new() -> Self {
        Self { level_db: Self::FLOOR_DB, hold_db: Self::FLOOR_DB, hold_left: 0 }
    }

    /// Add a chunk's peak. The bar falls 20 dB per second; the hold mark
    /// stays 1.5 s.
    fn update(&mut self, peak: f32, chunk_s: f32) {
        let db = if peak > 0.0 { 20.0 * libm::log10f(peak) } else { Self::FLOOR_DB };
        self.level_db = db.max(self.level_db - 20.0 * chunk_s).max(Self::FLOOR_DB);
        if db >= self.hold_db {
            self.hold_db = db;
            self.hold_left = (1.5 / chunk_s) as u32;
        } else if self.hold_left > 0 {
            self.hold_left -= 1;
        } else {
            self.hold_db = self.level_db;
        }
    }
}

pub struct BatApp<'a> {
    sample_rate: f32,
    mode: Mode,
    source: MonitorSource,
    /// Samples processed since start.
    samples: u64,

    het: Heterodyne,
    comb: HetComb<COMB_COUNT>,
    comb_first_hz: f32,
    fd: FrequencyDivider,
    fd_index: usize,
    te: TimeExpander<'a>,
    te_index: usize,
    /// Sample time of the last time-expansion trigger, for the screen.
    te_trigger: Option<u64>,
    pitch: PitchShifter,
    pitch_index: usize,

    analyzer: SpectrumAnalyzer<FFT_N>,
    detector: CallDetector<FFT_N>,
    detector_was_active: bool,
    calls: Recent,
    marks: Recent,
    last_call: Option<CallEvent>,

    /// Waterfall levels (0 to 255), one column of `WF_H` per `WF_COLUMN_MS`,
    /// column-major, as a ring: `wf_next` is the oldest column.
    wf: [u8; WF_W * WF_H],
    wf_next: usize,
    /// Index of the display column being filled (sample time / column length).
    wf_column: u64,
    wf_accum: [f32; WF_H],
    /// Display floor in dB, following the background level.
    floor_db: f32,

    /// The last `HISTORY` detector columns at snapshot height, for snapshots.
    history: [u8; HISTORY * SNAP_H],
    history_centre: [u64; HISTORY],
    history_next: usize,
    /// The last call, stretched to the snapshot's width, column-major.
    snapshot: [u8; SNAP_W * SNAP_H],

    meters: [Meter; 2],
    recording: bool,
    recorded_samples: u64,
    clock_s: u32,

    mono: [f32; CHUNK],
    out: [f32; CHUNK],
    te_out: [f32; CHUNK],
}

impl<'a> BatApp<'a> {
    /// `te_storage` holds the time expander's ring buffer: one second
    /// (`sample_rate` samples) is plenty for the default 300 ms window.
    pub fn new(sample_rate: f32, te_storage: &'a mut [f32]) -> Self {
        let mut analyzer = SpectrumAnalyzer::new(sample_rate);
        analyzer.set_hop(FFT_HOP);
        let mut app = Self {
            sample_rate,
            mode: Mode::Het,
            source: MonitorSource::Mix,
            samples: 0,
            het: Heterodyne::new(sample_rate, 45_000.0),
            comb: HetComb::new(sample_rate, 25_000.0, DEFAULT_COMB_SPACING_HZ, COMB_COUNT),
            comb_first_hz: 25_000.0,
            fd: FrequencyDivider::new(sample_rate),
            fd_index: 2,
            te: TimeExpander::new(sample_rate, te_storage),
            te_index: 2,
            te_trigger: None,
            pitch: PitchShifter::new(sample_rate),
            pitch_index: 2,
            analyzer,
            detector: CallDetector::new(sample_rate),
            detector_was_active: false,
            calls: Recent::new(),
            marks: Recent::new(),
            last_call: None,
            wf: [0; WF_W * WF_H],
            wf_next: 0,
            wf_column: 0,
            wf_accum: [f32::MIN; WF_H],
            floor_db: -110.0,
            history: [0; HISTORY * SNAP_H],
            history_centre: [0; HISTORY],
            history_next: 0,
            snapshot: [0; SNAP_W * SNAP_H],
            meters: [Meter::new(); 2],
            recording: false,
            recorded_samples: 0,
            clock_s: 0,
            mono: [0.0; CHUNK],
            out: [0.0; CHUNK],
            te_out: [0.0; CHUNK],
        };
        // Each call reaches about one of the comb's oscillators, and the comb
        // scales its sum by 1 / count, so restore that to match HET's level.
        app.comb.set_gain(COMB_COUNT as f32);
        app.apply_settings();
        app
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: Mode) {
        if mode == self.mode {
            return;
        }
        self.mode = mode;
        // Start the new mode clean, so no stale sound from its last use plays.
        match mode {
            Mode::Het => self.het.reset(),
            Mode::Comb => self.comb.reset(),
            Mode::Fd => self.fd.reset(),
            Mode::Te => {}
            Mode::Pitch => self.pitch.reset(),
        }
    }

    pub fn set_source(&mut self, source: MonitorSource) {
        self.source = source;
    }

    pub fn source(&self) -> MonitorSource {
        self.source
    }

    pub fn het_lo_hz(&self) -> f32 {
        self.het.lo()
    }

    pub fn is_recording(&self) -> bool {
        self.recording
    }

    pub fn last_call(&self) -> Option<CallEvent> {
        self.last_call
    }

    /// Wall-clock time of day, for the status bar.
    pub fn set_clock(&mut self, seconds_since_midnight: u32) {
        self.clock_s = seconds_since_midnight % 86_400;
    }

    /// A press of a hardware button.
    pub fn button(&mut self, button: Button) {
        match button {
            Button::Rec => {
                self.recording = !self.recording;
                if self.recording {
                    self.recorded_samples = 0;
                }
            }
            Button::Mark => {
                self.marks.push(self.samples);
                if self.mode == Mode::Te && matches!(self.te.state(), ExpanderState::Idle) {
                    self.trigger_te();
                }
            }
            Button::Home | Button::Play => {}
        }
    }

    /// Turn of the wheel, in detents (positive clockwise): changes the
    /// current mode's setting.
    pub fn wheel(&mut self, steps: i32) {
        let step = |i: usize, len: usize| (i as i32 + steps).clamp(0, len as i32 - 1) as usize;
        match self.mode {
            Mode::Het => self.set_het_lo(self.het.lo() + steps as f32 * HET_STEP_HZ),
            Mode::Comb => self.set_comb_first(self.comb_first_hz + steps as f32 * COMB_STEP_HZ),
            Mode::Fd => self.fd_index = step(self.fd_index, FD_DIVISIONS.len()),
            Mode::Te => self.te_index = step(self.te_index, TE_FACTORS.len()),
            Mode::Pitch => self.pitch_index = step(self.pitch_index, PITCH_FACTORS.len()),
        }
        self.apply_settings();
    }

    /// A tap on the touchscreen, in screen pixels.
    pub fn touch(&mut self, x: i32, y: i32) {
        if let Some(mode) = layout::mode_button_at(x, y) {
            self.set_mode(mode);
        } else if let Some(hz) = layout::waterfall_freq_at(x, y) {
            match self.mode {
                Mode::Het => self.set_het_lo(hz),
                Mode::Comb => {
                    // Move the comb so its nearest LO lands on the tapped frequency.
                    let spacing = DEFAULT_COMB_SPACING_HZ;
                    let i = libm::roundf((hz - self.comb_first_hz) / spacing).clamp(0.0, (COMB_COUNT - 1) as f32);
                    self.set_comb_first(hz - i * spacing);
                }
                _ => {}
            }
        }
    }

    fn set_het_lo(&mut self, hz: f32) {
        let hz = libm::roundf(hz / HET_STEP_HZ) * HET_STEP_HZ;
        self.het.set_lo(hz.clamp(HET_MIN_HZ, HET_MAX_HZ));
    }

    fn set_comb_first(&mut self, hz: f32) {
        let last_max = VIEW_HI_HZ - (COMB_COUNT - 1) as f32 * DEFAULT_COMB_SPACING_HZ;
        self.comb_first_hz = (libm::roundf(hz / COMB_STEP_HZ) * COMB_STEP_HZ).clamp(HET_MIN_HZ, last_max);
        self.comb.set_evenly_spaced(self.comb_first_hz, DEFAULT_COMB_SPACING_HZ, COMB_COUNT);
    }

    fn apply_settings(&mut self) {
        self.fd.set_division(FD_DIVISIONS[self.fd_index]);
        self.te.set_factor(TE_FACTORS[self.te_index]);
        self.pitch.set_factor(PITCH_FACTORS[self.pitch_index]);
    }

    fn trigger_te(&mut self) {
        self.te.trigger();
        self.te_trigger = Some(self.samples);
    }

    pub fn te_status(&self) -> TeStatus {
        match self.te.state() {
            ExpanderState::Idle => TeStatus::Waiting,
            ExpanderState::Capturing => TeStatus::Capturing,
            ExpanderState::Playing { progress } => TeStatus::Playing { progress },
        }
    }

    /// Process a block of inputs 3 and 4 into the headphone output. All four
    /// slices must have the same length; any length works.
    pub fn process(&mut self, in3: &[f32], in4: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        let n = in3.len().min(in4.len()).min(out_l.len()).min(out_r.len());
        let mut start = 0;
        while start < n {
            let end = (start + CHUNK).min(n);
            self.process_chunk(&in3[start..end], &in4[start..end], &mut out_l[start..end], &mut out_r[start..end]);
            start = end;
        }
    }

    fn process_chunk(&mut self, in3: &[f32], in4: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        let len = in3.len();
        let chunk_s = len as f32 / self.sample_rate;
        let peak = |x: &[f32]| x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        self.meters[0].update(peak(in3), chunk_s);
        self.meters[1].update(peak(in4), chunk_s);

        let mono = &mut self.mono[..len];
        for ((m, &a), &b) in mono.iter_mut().zip(in3).zip(in4) {
            *m = match self.source {
                MonitorSource::Input3 => a,
                MonitorSource::Input4 => b,
                MonitorSource::Mix => 0.5 * (a + b),
            };
        }

        self.analyse(len);

        // Trigger time expansion at the start of a call.
        let active = self.detector.is_active();
        if active && !self.detector_was_active && self.mode == Mode::Te && matches!(self.te.state(), ExpanderState::Idle)
        {
            self.trigger_te();
        }
        self.detector_was_active = active;

        let mono = &self.mono[..len];
        let out = &mut self.out[..len];
        self.te.process(mono, &mut self.te_out[..len]);
        match self.mode {
            Mode::Het => self.het.process(mono, out),
            Mode::Comb => self.comb.process(mono, out),
            Mode::Fd => self.fd.process(mono, out),
            Mode::Te => out.copy_from_slice(&self.te_out[..len]),
            Mode::Pitch => self.pitch.process(mono, out),
        }
        out_l.copy_from_slice(out);
        out_r.copy_from_slice(out);

        self.samples += len as u64;
        if self.recording {
            self.recorded_samples += len as u64;
        }
    }

    /// Feed the mono chunk to the spectrum analyzer, and each finished column
    /// to the detector, the waterfall and the snapshot history.
    fn analyse(&mut self, len: usize) {
        let Self {
            analyzer,
            detector,
            mono,
            calls,
            last_call,
            wf,
            wf_next,
            wf_column,
            wf_accum,
            floor_db,
            history,
            history_centre,
            history_next,
            snapshot,
            sample_rate,
            ..
        } = self;
        let column_len = (*sample_rate * WF_COLUMN_MS * 0.001) as u64;
        analyzer.push(&mono[..len], |col: &Column<'_>| {
            if let Some(event) = detector.process(col) {
                calls.push(event.start_sample);
                *last_call = Some(event);
                build_snapshot(&event, *sample_rate, history, history_centre, snapshot);
            }

            // Waterfall: the loudest level per pixel over each display column.
            let mut px = [0.0f32; WF_H];
            col.to_pixels(VIEW_HI_HZ, VIEW_LO_HZ, &mut px);
            let this_column = col.centre_sample / column_len.max(1);
            if this_column != *wf_column {
                // Follow the background: the column's median-ish level, only
                // while no call is in progress.
                if !detector.is_active() {
                    let mean = wf_accum.iter().sum::<f32>() / WF_H as f32;
                    if mean > -200.0 {
                        *floor_db += 0.02 * (mean - 3.0 - *floor_db);
                        *floor_db = floor_db.max(MIN_FLOOR_DB);
                    }
                }
                let base = *wf_next * WF_H;
                for (dst, &db) in wf[base..base + WF_H].iter_mut().zip(wf_accum.iter()) {
                    *dst = level(db, *floor_db);
                }
                *wf_next = (*wf_next + 1) % WF_W;
                *wf_column = this_column;
                wf_accum.fill(f32::MIN);
            }
            for (a, &p) in wf_accum.iter_mut().zip(&px) {
                *a = a.max(p);
            }

            // Snapshot history at snapshot height.
            let mut sp = [0.0f32; SNAP_H];
            col.to_pixels(VIEW_HI_HZ, VIEW_LO_HZ, &mut sp);
            let base = *history_next * SNAP_H;
            for (dst, &db) in history[base..base + SNAP_H].iter_mut().zip(&sp) {
                *dst = level(db, *floor_db);
            }
            history_centre[*history_next] = col.centre_sample;
            *history_next = (*history_next + 1) % HISTORY;
        });
    }

    /// Recorded length of the current or last recording, in seconds.
    pub fn recorded_seconds(&self) -> u64 {
        (self.recorded_samples as f32 / self.sample_rate) as u64
    }

    /// Draw the whole screen.
    pub fn render(&self, fb: &mut Rgb565Buffer<'_>) {
        screen::draw(self, fb);
    }
}

/// Map a level in dB to 0..=255 for the colour map.
fn level(db: f32, floor_db: f32) -> u8 {
    let t = (db - floor_db) / DISPLAY_RANGE_DB;
    (t.clamp(0.0, 1.0) * 255.0) as u8
}

/// Copy the history columns around `event` into `snapshot`, stretched to
/// `SNAP_W` columns.
fn build_snapshot(
    event: &CallEvent,
    sample_rate: f32,
    history: &[u8; HISTORY * SNAP_H],
    centres: &[u64; HISTORY],
    snapshot: &mut [u8; SNAP_W * SNAP_H],
) {
    let margin = (SNAP_MARGIN_MS * 0.001 * sample_rate) as u64;
    let from = event.start_sample.saturating_sub(margin);
    let to = event.end_sample + margin;
    // History slots in time order that fall in the window.
    let mut slots = [0usize; HISTORY];
    let mut count = 0;
    let mut order: [usize; HISTORY] = core::array::from_fn(|i| i);
    order.sort_unstable_by_key(|&i| centres[i]);
    for &i in &order {
        if centres[i] >= from && centres[i] <= to {
            slots[count] = i;
            count += 1;
        }
    }
    if count == 0 {
        return;
    }
    for x in 0..SNAP_W {
        let src = slots[x * count / SNAP_W];
        snapshot[x * SNAP_H..(x + 1) * SNAP_H].copy_from_slice(&history[src * SNAP_H..(src + 1) * SNAP_H]);
    }
}

#[cfg(test)]
mod tests;
