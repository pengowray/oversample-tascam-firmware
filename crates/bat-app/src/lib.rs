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

use bat_dsp::classify::CallClassifier;
pub use bat_dsp::classify::CallKind;
use bat_dsp::detector::{CallDetector, CallEvent, DEFAULT_AUDIBLE_BAND_HZ, DEFAULT_BAND_LO_HZ};
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
/// Waterfall contrast defaults; see [`BatApp::set_display`]. Chosen on X8
/// recordings of bats at 45-50 kHz: 3 dB above the background turns the
/// background speckle black and keeps faint calls, and 45 dB of range keeps
/// strong calls from saturating. One floor for all rows (not per row) keeps a
/// steady tone, such as RF hum, visible as a line.
pub const DEFAULT_DISPLAY_OFFSET_DB: f32 = 3.0;
pub const DEFAULT_DISPLAY_RANGE_DB: f32 = 45.0;
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
/// Bins per spectrum column.
const BINS: usize = FFT_N / 2 + 1;
/// A snapshot shows the call's own frequency range plus this much either
/// side, and at least `SNAP_MIN_SPAN_HZ` in all.
const SNAP_FREQ_MARGIN_HZ: f32 = 5_000.0;
const SNAP_MIN_SPAN_HZ: f32 = 20_000.0;
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
/// Choices for the lowest bat call frequency, in kHz; 16 kHz is the default.
const MIN_BAT_KHZ: [f32; 5] = [14.0, 16.0, 18.0, 20.0, 25.0];
const MIN_BAT_DEFAULT: usize = 1;
const SOURCES: [MonitorSource; 3] = [MonitorSource::Mix, MonitorSource::Input3, MonitorSource::Input4];

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

/// Which screen is showing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    Main,
    Settings,
}

/// Rows of the settings screen, top to bottom.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Setting {
    /// Sounds peaking below this are not counted as bat calls.
    LowestBatCall,
    /// The input heard in the headphones and analysed.
    ListenTo,
    /// Whether detections of this kind are marked: a tick under the
    /// waterfall, the last-call panel, and time expansion. On or off.
    Mark(CallKind),
}

impl Setting {
    pub const ALL: [Setting; 6] = [
        Setting::LowestBatCall,
        Setting::ListenTo,
        Setting::Mark(CallKind::Bat),
        Setting::Mark(CallKind::Tone),
        Setting::Mark(CallKind::Audible),
        Setting::Mark(CallKind::Broadband),
    ];
    /// Settings changed with - and +; the rest are switches.
    pub const STEPPED: usize = 2;
}

/// Kinds of detection marked when the app starts: bat calls only.
const MARKED_DEFAULT: u8 = kind_bit(CallKind::Bat);

const fn kind_bit(kind: CallKind) -> u8 {
    1 << kind as u8
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

/// A short list of recent items (call times and kinds, marks), oldest
/// overwritten.
#[derive(Clone, Copy)]
struct Recent<T: Copy> {
    items: [T; 16],
    len: usize,
    next: usize,
}

impl<T: Copy> Recent<T> {
    const fn new(empty: T) -> Self {
        Self { items: [empty; 16], len: 0, next: 0 }
    }

    fn push(&mut self, item: T) {
        self.items[self.next] = item;
        self.next = (self.next + 1) % self.items.len();
        self.len = (self.len + 1).min(self.items.len());
    }

    fn iter(&self) -> impl Iterator<Item = T> + '_ {
        self.items[..self.len].iter().copied()
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
    screen: Screen,
    /// Selected row on the settings screen; the wheel changes it.
    setting: usize,
    min_bat_index: usize,
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
    classifier: CallClassifier,
    /// Kinds of detection that are marked, one bit each (`kind_bit`).
    marked: u8,
    /// A marked detection came in the chunk being processed.
    detected: bool,
    /// Start times and kinds of marked detections.
    calls: Recent<(u64, CallKind)>,
    marks: Recent<u64>,
    last_call: Option<(CallEvent, CallKind)>,
    /// Where the last call's length is measured from: its start, or for a
    /// steady tone, the start of the tone.
    last_call_since: u64,

    /// Waterfall levels (0 to 255), one column of `WF_H` per `WF_COLUMN_MS`,
    /// column-major, as a ring: `wf_next` is the oldest column.
    wf: [u8; WF_W * WF_H],
    wf_next: usize,
    /// Index of the display column being filled (sample time / column length).
    wf_column: u64,
    wf_accum: [f32; WF_H],
    /// Background level of each waterfall row, in dB, followed while no call
    /// is in progress.
    row_floor: [f32; WF_H],
    row_floor_set: bool,
    display_offset_db: f32,
    display_range_db: f32,
    /// Set each row's floor from that row's own background, instead of one
    /// floor for the whole waterfall.
    display_per_row: bool,

    /// The last `HISTORY` detector columns, every bin as a level, for
    /// snapshots.
    history: [u8; HISTORY * BINS],
    history_centre: [u64; HISTORY],
    history_next: usize,
    /// The last call, scaled to the snapshot's size, column-major, and the
    /// frequency range it shows.
    snapshot: [u8; SNAP_W * SNAP_H],
    snapshot_hz: (f32, f32),

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
    ///
    /// This returns the whole app (about 250 KB) by value. On the X8, where it
    /// cannot pass through a stack, build it with
    /// [`init_in_place`](Self::init_in_place) instead.
    pub fn new(sample_rate: f32, te_storage: &'a mut [f32]) -> Self {
        let mut app = core::mem::MaybeUninit::<Self>::uninit();
        // SAFETY: init_in_place writes every field.
        unsafe {
            Self::init_in_place(app.as_mut_ptr(), sample_rate, te_storage);
            app.assume_init()
        }
    }

    /// Build the app at `this`, field by field, so that no more than its
    /// largest part (the pitch shifter, about 23 KB) passes through the stack.
    ///
    /// # Safety
    ///
    /// `this` must be valid for writes and aligned. Whatever it held is
    /// overwritten without being dropped.
    pub unsafe fn init_in_place(this: *mut Self, sample_rate: f32, te_storage: &'a mut [f32]) {
        use core::ptr::addr_of_mut as field;
        /// Fill an array field element by element, without a temporary.
        unsafe fn fill<T: Copy, const N: usize>(array: *mut [T; N], value: T) {
            let first = array.cast::<T>();
            for i in 0..N {
                unsafe { first.add(i).write(value) };
            }
        }
        // SAFETY: every write is to a field of `this`; each field is written
        // before anything reads it, and the references below are taken only
        // once every field is written.
        unsafe {
            field!((*this).sample_rate).write(sample_rate);
            field!((*this).mode).write(Mode::Het);
            field!((*this).source).write(MonitorSource::Mix);
            field!((*this).screen).write(Screen::Main);
            field!((*this).setting).write(0);
            field!((*this).min_bat_index).write(MIN_BAT_DEFAULT);
            field!((*this).samples).write(0);
            field!((*this).het).write(Heterodyne::new(sample_rate, 45_000.0));
            field!((*this).comb).write(HetComb::new(sample_rate, 25_000.0, DEFAULT_COMB_SPACING_HZ, COMB_COUNT));
            field!((*this).comb_first_hz).write(25_000.0);
            field!((*this).fd).write(FrequencyDivider::new(sample_rate));
            field!((*this).fd_index).write(2);
            field!((*this).te).write(TimeExpander::new(sample_rate, te_storage));
            field!((*this).te_index).write(2);
            field!((*this).te_trigger).write(None);
            field!((*this).pitch).write(PitchShifter::unconfigured());
            (*field!((*this).pitch)).init(sample_rate);
            field!((*this).pitch_index).write(2);
            field!((*this).analyzer).write(SpectrumAnalyzer::unconfigured());
            (*field!((*this).analyzer)).init(sample_rate);
            (*field!((*this).analyzer)).set_hop(FFT_HOP);
            field!((*this).detector).write(CallDetector::unconfigured());
            (*field!((*this).detector)).init(sample_rate);
            field!((*this).classifier).write(CallClassifier::new(sample_rate, DEFAULT_BAND_LO_HZ));
            field!((*this).marked).write(MARKED_DEFAULT);
            field!((*this).detected).write(false);
            field!((*this).calls).write(Recent::new((0, CallKind::Bat)));
            field!((*this).marks).write(Recent::new(0));
            field!((*this).last_call).write(None);
            field!((*this).last_call_since).write(0);
            fill(field!((*this).wf), 0);
            field!((*this).wf_next).write(0);
            field!((*this).wf_column).write(0);
            fill(field!((*this).wf_accum), f32::MIN);
            fill(field!((*this).row_floor), -110.0);
            field!((*this).row_floor_set).write(false);
            field!((*this).display_offset_db).write(DEFAULT_DISPLAY_OFFSET_DB);
            field!((*this).display_range_db).write(DEFAULT_DISPLAY_RANGE_DB);
            field!((*this).display_per_row).write(false);
            fill(field!((*this).history), 0);
            fill(field!((*this).history_centre), 0);
            field!((*this).history_next).write(0);
            fill(field!((*this).snapshot), 0);
            field!((*this).snapshot_hz).write((VIEW_LO_HZ, VIEW_HI_HZ));
            field!((*this).meters).write([Meter::new(); 2]);
            field!((*this).recording).write(false);
            field!((*this).recorded_samples).write(0);
            field!((*this).clock_s).write(0);
            fill(field!((*this).mono), 0.0);
            fill(field!((*this).out), 0.0);
            fill(field!((*this).te_out), 0.0);

            let app = &mut *this;
            // Each call reaches about one of the comb's oscillators, and the comb
            // scales its sum by 1 / count, so restore that to match HET's level.
            app.comb.set_gain(COMB_COUNT as f32);
            app.apply_settings();
            app.apply_lowest_bat_call();
        }
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

    pub fn screen(&self) -> Screen {
        self.screen
    }

    /// Show the settings screen, as the menu button does.
    pub fn open_settings(&mut self) {
        self.screen = Screen::Settings;
    }

    /// Lowest frequency counted as a bat call, in Hz.
    pub fn lowest_bat_call_hz(&self) -> f32 {
        MIN_BAT_KHZ[self.min_bat_index] * 1000.0
    }

    /// Selected row on the settings screen.
    pub fn selected_setting(&self) -> Setting {
        Setting::ALL[self.setting]
    }

    /// Step a setting through its choices (clamped at the ends).
    pub fn change_setting(&mut self, setting: Setting, steps: i32) {
        let step = |i: usize, len: usize| (i as i32 + steps).clamp(0, len as i32 - 1) as usize;
        match setting {
            Setting::LowestBatCall => {
                self.min_bat_index = step(self.min_bat_index, MIN_BAT_KHZ.len());
                self.apply_lowest_bat_call();
            }
            Setting::ListenTo => {
                let i = SOURCES.iter().position(|&s| s == self.source).unwrap_or(0);
                self.source = SOURCES[step(i, SOURCES.len())];
            }
            // Turning the wheel up switches on, down switches off.
            Setting::Mark(kind) if steps != 0 => self.set_marked(kind, steps > 0),
            Setting::Mark(_) => {}
        }
    }

    /// Whether detections of this kind are marked.
    pub fn is_marked(&self, kind: CallKind) -> bool {
        self.marked & kind_bit(kind) != 0
    }

    pub fn set_marked(&mut self, kind: CallKind, on: bool) {
        if on {
            self.marked |= kind_bit(kind);
        } else {
            self.marked &= !kind_bit(kind);
        }
    }

    /// Set the classifier's lowest bat frequency. Below 16 kHz the detector's
    /// band starts lower too (it starts at 15 kHz), so calls there are not
    /// mistaken for broadband sounds reaching the band's bottom; the audible
    /// band then stops below it.
    fn apply_lowest_bat_call(&mut self) {
        let min = self.lowest_bat_call_hz();
        let band_lo = (min - 1_000.0).min(DEFAULT_BAND_LO_HZ);
        let (_, band_hi) = self.detector.band();
        self.detector.set_band(band_lo, band_hi);
        self.detector.set_audible_band(DEFAULT_AUDIBLE_BAND_HZ.0, DEFAULT_AUDIBLE_BAND_HZ.1.min(band_lo - 1_000.0));
        self.classifier.set_band_lo_hz(band_lo);
        self.classifier.set_min_bat_hz(min);
    }

    pub fn het_lo_hz(&self) -> f32 {
        self.het.lo()
    }

    pub fn is_recording(&self) -> bool {
        self.recording
    }

    /// The latest marked detection.
    pub fn last_call(&self) -> Option<CallEvent> {
        self.last_call.map(|(e, _)| e)
    }

    /// The kind of the latest marked detection.
    pub fn last_call_kind(&self) -> Option<CallKind> {
        self.last_call.map(|(_, k)| k)
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
            // HOME closes the settings screen; on the main screen it belongs to
            // the X8's launcher.
            Button::Home => self.screen = Screen::Main,
            Button::Play => {}
        }
    }

    /// Turn of the wheel, in detents (positive clockwise): changes the
    /// current mode's setting.
    pub fn wheel(&mut self, steps: i32) {
        if self.screen == Screen::Settings {
            self.change_setting(self.selected_setting(), steps);
            return;
        }
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
        if self.screen == Screen::Settings {
            match layout::settings_hit(x, y, Setting::STEPPED, Setting::ALL.len()) {
                Some(layout::SettingsHit::Row(i)) => self.setting = i,
                Some(layout::SettingsHit::Switch(i)) => {
                    self.setting = i;
                    if let Setting::Mark(kind) = Setting::ALL[i] {
                        self.set_marked(kind, !self.is_marked(kind));
                    }
                }
                Some(layout::SettingsHit::Minus(i)) => {
                    self.setting = i;
                    self.change_setting(Setting::ALL[i], -1);
                }
                Some(layout::SettingsHit::Plus(i)) => {
                    self.setting = i;
                    self.change_setting(Setting::ALL[i], 1);
                }
                Some(layout::SettingsHit::Done) => self.screen = Screen::Main,
                None => {}
            }
            return;
        }
        if layout::MENU.contains(embedded_graphics::prelude::Point::new(x, y)) {
            self.screen = Screen::Settings;
        } else if let Some(mode) = layout::mode_button_at(x, y) {
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

        self.detected = false;
        self.analyse(len);

        // Trigger time expansion on a marked detection. For a bat call that
        // is when it is confirmed, just after the call (the second of a
        // train), within the 50 ms the expander keeps from before its trigger.
        if self.detected && self.mode == Mode::Te && matches!(self.te.state(), ExpanderState::Idle) {
            self.trigger_te();
        }

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
            classifier,
            marked,
            detected,
            mono,
            calls,
            last_call,
            last_call_since,
            wf,
            wf_next,
            wf_column,
            wf_accum,
            row_floor,
            row_floor_set,
            display_offset_db,
            display_range_db,
            display_per_row,
            history,
            history_centre,
            history_next,
            snapshot,
            snapshot_hz,
            sample_rate,
            ..
        } = self;
        let column_len = (*sample_rate * WF_COLUMN_MS * 0.001) as u64;
        analyzer.push(&mono[..len], |col: &Column<'_>| {
            // Only the kinds of detection chosen on the settings screen (bat
            // calls by default) are marked, shown and trigger time expansion;
            // see bat_dsp::classify for the kinds.
            if let Some(event) = detector.process(col) {
                let verdict = classifier.classify(&event);
                if *marked & kind_bit(verdict.kind) != 0 {
                    if let Some(first) = verdict.earlier {
                        calls.push((first.start_sample, CallKind::Bat));
                    }
                    calls.push((event.start_sample, verdict.kind));
                    *last_call = Some((event, verdict.kind));
                    *last_call_since = classifier.tone_start().unwrap_or(event.start_sample);
                    *snapshot_hz = build_snapshot(&event, *sample_rate, history, history_centre, snapshot);
                    *detected = true;
                }
            }

            // Waterfall: the loudest level per pixel over each display column.
            let mut px = [0.0f32; WF_H];
            col.to_pixels(VIEW_HI_HZ, VIEW_LO_HZ, &mut px);
            let this_column = col.centre_sample / column_len.max(1);
            if this_column != *wf_column {
                // Follow each row's background, only while no call is in
                // progress. Digital silence stops at MIN_FLOOR_DB.
                if wf_accum[0] > f32::MIN && !detector.is_active() {
                    let rate = if *row_floor_set { 0.02 } else { 1.0 };
                    for (f, &a) in row_floor.iter_mut().zip(wf_accum.iter()) {
                        *f = (*f + rate * (a - *f)).max(MIN_FLOOR_DB);
                    }
                    *row_floor_set = true;
                }
                let floors = display_floors(row_floor, *display_per_row, *display_offset_db);
                let base = *wf_next * WF_H;
                for ((dst, &db), &floor) in wf[base..base + WF_H].iter_mut().zip(wf_accum.iter()).zip(&floors) {
                    *dst = level(db, floor, *display_range_db);
                }
                *wf_next = (*wf_next + 1) % WF_W;
                *wf_column = this_column;
                wf_accum.fill(f32::MIN);
            }
            for (a, &p) in wf_accum.iter_mut().zip(&px) {
                *a = a.max(p);
            }

            // Snapshot history: every bin, against the floor of the waterfall
            // row at that bin's frequency.
            let floors = display_floors(row_floor, *display_per_row, *display_offset_db);
            let base = *history_next * BINS;
            for (k, (dst, &db)) in history[base..base + BINS].iter_mut().zip(col.db).enumerate() {
                let hz = k as f32 * col.bin_hz;
                let t = ((VIEW_HI_HZ - hz) / (VIEW_HI_HZ - VIEW_LO_HZ)).clamp(0.0, 1.0);
                let row = ((t * WF_H as f32) as usize).min(WF_H - 1);
                *dst = level(db, floors[row], *display_range_db);
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

    /// Waterfall contrast. A level `offset_db` above the background shows as
    /// black, and `range_db` above that as the brightest colour. With
    /// `per_row`, each frequency row has its own background, so a band that
    /// is noisier than the rest (low frequencies, a steady hum) shows as dark
    /// as the quiet bands.
    pub fn set_display(&mut self, offset_db: f32, range_db: f32, per_row: bool) {
        self.display_offset_db = offset_db;
        self.display_range_db = range_db.max(1.0);
        self.display_per_row = per_row;
    }

    /// Whether a tap at (x, y) would do something: the menu button, a mode
    /// button, the waterfall in HET or COMB mode (where a tap tunes), or a
    /// control on the settings screen.
    pub fn is_tappable(&self, x: i32, y: i32) -> bool {
        if self.screen == Screen::Settings {
            return layout::settings_hit(x, y, Setting::STEPPED, Setting::ALL.len()).is_some();
        }
        layout::MENU.contains(embedded_graphics::prelude::Point::new(x, y))
            || layout::mode_button_at(x, y).is_some()
            || (matches!(self.mode, Mode::Het | Mode::Comb) && layout::waterfall_freq_at(x, y).is_some())
    }

    /// Seconds of time-expansion playback still to come, while playing.
    pub fn te_seconds_left(&self) -> f32 {
        match self.te.state() {
            ExpanderState::Playing { progress } => (1.0 - progress) * self.te.playback_len() as f32 / self.sample_rate,
            _ => 0.0,
        }
    }
}

/// Map a level in dB to 0..=255 for the colour map.
fn level(db: f32, floor_db: f32, range_db: f32) -> u8 {
    let t = (db - floor_db) / range_db;
    (t.clamp(0.0, 1.0) * 255.0) as u8
}

/// The level shown as black for each waterfall row: the row's own background,
/// or the average background of all rows, plus the offset.
fn display_floors(row_floor: &[f32; WF_H], per_row: bool, offset_db: f32) -> [f32; WF_H] {
    if per_row {
        row_floor.map(|f| f + offset_db)
    } else {
        let mean = row_floor.iter().sum::<f32>() / WF_H as f32;
        [mean + offset_db; WF_H]
    }
}

/// Draw the call in `event` from the history into `snapshot`: the call's
/// time plus `SNAP_MARGIN_MS` either side across, its frequency range plus
/// `SNAP_FREQ_MARGIN_HZ` either side up, interpolated between columns and
/// bins. Returns the frequency range shown, low and high.
fn build_snapshot(
    event: &CallEvent,
    sample_rate: f32,
    history: &[u8; HISTORY * BINS],
    centres: &[u64; HISTORY],
    snapshot: &mut [u8; SNAP_W * SNAP_H],
) -> (f32, f32) {
    let margin = (SNAP_MARGIN_MS * 0.001 * sample_rate) as u64;
    let from = event.start_sample.saturating_sub(margin);
    let to = event.end_sample + margin;
    // History slots in time order that fall in the window.
    let mut order: [usize; HISTORY] = core::array::from_fn(|i| i);
    order.sort_unstable_by_key(|&i| centres[i]);
    let mut slots = [0usize; HISTORY];
    let mut count = 0;
    for &i in &order {
        if centres[i] >= from && centres[i] <= to {
            slots[count] = i;
            count += 1;
        }
    }

    let mut lo = event.min_freq_hz - SNAP_FREQ_MARGIN_HZ;
    let mut hi = event.max_freq_hz + SNAP_FREQ_MARGIN_HZ;
    if hi - lo < SNAP_MIN_SPAN_HZ {
        let centre = 0.5 * (lo + hi);
        lo = centre - 0.5 * SNAP_MIN_SPAN_HZ;
        hi = centre + 0.5 * SNAP_MIN_SPAN_HZ;
    }
    if lo < VIEW_LO_HZ {
        hi += VIEW_LO_HZ - lo;
        lo = VIEW_LO_HZ;
    }
    if hi > VIEW_HI_HZ {
        lo = (lo - (hi - VIEW_HI_HZ)).max(VIEW_LO_HZ);
        hi = VIEW_HI_HZ;
    }
    if count == 0 {
        snapshot.fill(0);
        return (lo, hi);
    }

    let bin_hz = sample_rate / FFT_N as f32;
    let at = |slot: usize, bin: f32| -> f32 {
        let b0 = (bin as usize).min(BINS - 1);
        let b1 = (b0 + 1).min(BINS - 1);
        let t = bin - b0 as f32;
        let row = &history[slot * BINS..(slot + 1) * BINS];
        row[b0] as f32 * (1.0 - t) + row[b1] as f32 * t
    };
    for x in 0..SNAP_W {
        let pos = if count > 1 { x as f32 * (count - 1) as f32 / (SNAP_W - 1) as f32 } else { 0.0 };
        let i0 = (pos as usize).min(count - 1);
        let i1 = (i0 + 1).min(count - 1);
        let tx = pos - i0 as f32;
        for y in 0..SNAP_H {
            let hz = hi - y as f32 * (hi - lo) / (SNAP_H - 1) as f32;
            let bin = (hz / bin_hz).max(0.0);
            let v = at(slots[i0], bin) * (1.0 - tx) + at(slots[i1], bin) * tx;
            snapshot[x * SNAP_H + y] = v as u8;
        }
    }
    (lo, hi)
}

#[cfg(test)]
mod tests;
