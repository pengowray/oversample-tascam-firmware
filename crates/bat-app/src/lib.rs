//! The bat app for the TASCAM Portacapture X8: listening modes, call
//! detection, and its screen.
//!
//! [`BatApp`] takes two of the recorder's inputs in blocks, makes the audio for
//! the headphones in the chosen listening mode (each input to its own ear in HET,
//! COMB and 1:1, and mono in the others), finds bat calls, and draws a
//! 320 × 480 screen (the X8's panel, used in portrait like the stock apps)
//! into an RGB565 framebuffer. It knows nothing about the hardware: the X8
//! firmware, or the web emulator in `crates/bat-app-web`, feeds it audio and
//! button, wheel and touch events, and copies the framebuffer to a display.
//!
//! `no_std` and no allocation. The time expander's storage is passed in.

#![no_std]

#[cfg(test)]
extern crate std;

mod chime_test;
mod cpu_test;
pub mod fb;
mod layout;
mod level;
pub mod palette;
mod phone_time;
pub mod player;
mod qr;
mod remote;
mod screen;
mod stages;

pub use bat_dsp::classify::CallKind;
use bat_dsp::classify::{CallClassifier, DEFAULT_TRAIN_MIN_DB};
use bat_dsp::detector::{CallDetector, CallEvent, DEFAULT_AUDIBLE_BAND_HZ, DEFAULT_BAND_LO_HZ};
use bat_dsp::freq_div::FrequencyDivider;
use bat_dsp::heterodyne::{DEFAULT_COMB_SPACING_HZ, DEFAULT_CUTOFF_HZ, HetComb, Heterodyne};
use bat_dsp::pitch_shift::PitchShifter;
use bat_dsp::spectrum::{Column, SpectrumAnalyzer};
use bat_dsp::time_expansion::{ExpanderState, TimeExpander};

pub use cpu_test::{HostFigures, Row as CpuTestRow, RowResult as CpuTestResult};
pub use fb::Rgb565Buffer;
use layout::{SNAP_H, SNAP_W, WF_H, WF_W};
pub use level::AutoVolume;
pub use phone_time::{ClockState, PhoneTime};
pub use player::{FileEntry, Player};
pub use remote::{ListeningMode, RecorderCommand, RemoteCommand, RemoteResult};
pub use stages::Stage;

/// Screen size in pixels.
pub const WIDTH: usize = 320;
pub const HEIGHT: usize = 480;

/// Frequency range of the waterfall and the call snapshot.
/// From 0 Hz, so audible sound shows too and the input can be seen working.
pub const VIEW_LO_HZ: f32 = 0.0;
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
/// bins at 192 kHz) every 192 samples (1 ms).
///
/// The analysis is most of the cost of processing, and nearly all of it is per
/// column. Hop 192 costs two thirds of hop 128 and finds as many bat calls (on
/// 238 recordings, 5486 against 5399). Moving the hop 128 columns by half a hop
/// changes which calls are found by as much as moving to hop 192 does. Above 192
/// short calls are lost: 5246 at hop 208, 4902 at hop 224, 4495 at hop 256. At
/// hop 192 two loud columns span the detector's 1 ms minimum call length; at
/// hops 128 to 191 it takes three. See the `hop_compare` example.
pub const FFT_HOP: usize = 192;
const FFT_N: usize = 512;
/// Detector columns kept for call snapshots: 192 × 1 ms = 192 ms, longer than
/// the longest call the detector reports (100 ms).
const HISTORY: usize = 192;
/// Bins per spectrum column.
const BINS: usize = FFT_N / 2 + 1;
/// A snapshot shows the call's own frequency range plus this much either
/// side, and at least `SNAP_MIN_SPAN_HZ` in all.
const SNAP_FREQ_MARGIN_HZ: f32 = 5_000.0;
const SNAP_MIN_SPAN_HZ: f32 = 20_000.0;
/// Audio is processed in pieces of this many samples.
const CHUNK: usize = 256;
/// Waterfall value of a display column that was not analysed. The colour map
/// shows it as grey, not as a level; real levels stop one below it.
const WF_NO_DATA: u8 = 255;
/// After lost input, how many samples the step in the input at the join is faded
/// over (0.33 ms at 192 kHz).
const JOIN_FADE_SAMPLES: u32 = 64;
/// How long the headphones take to fade out when a take starts with Mute while recording on, and
/// to fade back in when it ends: 20 ms at 192 kHz.
const MUTE_FADE_SAMPLES: f32 = 3_840.0;
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
    /// The input as it is, unshifted (only audible sound is heard).
    Thru,
}

impl From<ListeningMode> for Mode {
    fn from(mode: ListeningMode) -> Self {
        match mode {
            ListeningMode::Het => Mode::Het,
            ListeningMode::Comb => Mode::Comb,
            ListeningMode::Fd => Mode::Fd,
            ListeningMode::Te => Mode::Te,
            ListeningMode::Pitch => Mode::Pitch,
            ListeningMode::Thru => Mode::Thru,
        }
    }
}

impl Mode {
    pub const ALL: [Mode; 6] = [Mode::Het, Mode::Comb, Mode::Fd, Mode::Te, Mode::Pitch, Mode::Thru];
    /// The modes the X8 runs fast enough: PITCH cannot keep up there.
    pub const WITHOUT_PITCH: [Mode; 5] = [Mode::Het, Mode::Comb, Mode::Fd, Mode::Te, Mode::Thru];

    /// Button label. HET, FD and TE are the usual names on bat detectors.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Het => "HET",
            Mode::Comb => "COMB",
            Mode::Fd => "FD",
            Mode::Te => "TE",
            Mode::Pitch => "PITCH",
            Mode::Thru => "1:1",
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

/// What the recorder is doing, as the status bar shows it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Recorder {
    Stopped,
    /// Record standby: armed, waiting for a second REC press.
    Standby,
    Recording,
    Paused,
}

/// Which screen is showing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    Main,
    Settings,
    /// Shows what the chime decoders read, while they listen: see [`chime_test`].
    ChimeTest,
    /// The CPU test's results: see [`cpu_test`]. The test itself runs on the main screen.
    CpuTest,
    /// BAT PLAYBACK's list of recordings on the card: see [`player`].
    Files,
    /// BAT PLAYBACK's choice of the folder listed, from the folders at the top of the card.
    Folders,
    /// BAT PLAYBACK's information about one file: its format, and the XRI and GUANO texts in it.
    FileInfo,
}

/// How long a finger stays down without moving on a row of BAT PLAYBACK's list to open that
/// file's information. The host times it and calls [`BatApp::hold`].
pub const LONG_PRESS_MS: u32 = 600;

/// A finger on the screen that may move: see `BatApp::release`.
#[derive(Clone, Copy, Debug)]
struct Gesture {
    down: Option<(i32, i32)>,
    at: (i32, i32),
    moved: bool,
    top: usize,
}

impl Gesture {
    const NONE: Self = Self { down: None, at: (0, 0), moved: false, top: 0 };
}

/// The pages of the settings screen, in the order of their tabs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingsPage {
    /// What changes the sound in the headphones, and the sounds to mark.
    Listen,
    /// What a take's files get.
    Record,
    /// The phone remote, the phone page's QR code and the chime test: everything the phone page
    /// does with the X8, on one page.
    Phone,
    /// The debug figures and the CPU test.
    Tests,
}

impl SettingsPage {
    pub const ALL: [SettingsPage; 4] =
        [SettingsPage::Listen, SettingsPage::Record, SettingsPage::Phone, SettingsPage::Tests];

    pub fn label(self) -> &'static str {
        match self {
            SettingsPage::Listen => "Listen",
            SettingsPage::Record => "Record",
            SettingsPage::Phone => "Phone",
            SettingsPage::Tests => "Tests",
        }
    }
}

/// Settings that the wheel can change, page by page, top to bottom.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Setting {
    /// Sounds peaking below this are not counted as bat calls.
    LowestBatCall,
    /// The sound heard in the headphones and analysed: see [`MonitorSource`].
    ListenTo,
    /// Whether the headphone level follows the sound automatically, and whether the
    /// headphones are silent between calls: see [`AutoVolume`].
    AutoLevel,
    /// Whether the host shows its figures for testing.
    DebugInfo,
    /// Whether the host redraws the screen only after a touch, button or wheel turn: see
    /// [`BatApp::draw_on_input`].
    DrawOnInput,
    /// Whether a take also writes the stock recorder's MIX file: see
    /// [`BatApp::set_mix_file_available`].
    MixFile,
    /// Whether a phone remote chime also sets the host's clock: see [`PhoneTime`].
    ClockFromRemote,
    /// Whether REC pauses a take: see [`BatApp::set_pause_option_available`].
    PauseWithRec,
    /// Whether the headphones are silent while a take is being recorded: see
    /// [`BatApp::mute_while_recording`].
    MuteWhileRecording,
    /// Whether the host runs from its cells when a USB supply is weak, instead of asking about
    /// the supply: see [`BatApp::set_power_option_available`].
    CellsOnWeakUsb,
    /// Whether detections of this kind are marked: a tick under the
    /// waterfall, the last-call panel, and time expansion. On or off.
    Mark(CallKind),
}

impl Setting {
    pub const ALL: [Setting; 14] = [
        Setting::LowestBatCall,
        Setting::ListenTo,
        Setting::AutoLevel,
        Setting::DebugInfo,
        Setting::DrawOnInput,
        Setting::MixFile,
        Setting::PauseWithRec,
        Setting::MuteWhileRecording,
        Setting::CellsOnWeakUsb,
        Setting::ClockFromRemote,
        Setting::Mark(CallKind::Bat),
        Setting::Mark(CallKind::Tone),
        Setting::Mark(CallKind::Audible),
        Setting::Mark(CallKind::Broadband),
    ];
    /// The sounds to mark, top to bottom.
    pub const MARKS: [CallKind; 4] = [CallKind::Bat, CallKind::Tone, CallKind::Audible, CallKind::Broadband];

    pub fn page(self) -> SettingsPage {
        match self {
            Setting::MixFile | Setting::PauseWithRec | Setting::MuteWhileRecording | Setting::CellsOnWeakUsb => {
                SettingsPage::Record
            }
            Setting::ClockFromRemote => SettingsPage::Phone,
            Setting::DebugInfo | Setting::DrawOnInput => SettingsPage::Tests,
            _ => SettingsPage::Listen,
        }
    }
}

/// How long the main screen says that REC did not pause a take, in seconds.
const PAUSE_REFUSED_S: u64 = 3;

/// Kinds of detection marked when the app starts: bat calls only.
const MARKED_DEFAULT: u8 = kind_bit(CallKind::Bat);

const fn kind_bit(kind: CallKind) -> u8 {
    1 << kind as u8
}

/// Which of the two blocks passed to `process` feeds the headphones and the
/// detector.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MonitorSource {
    First,
    Second,
    /// In HET, COMB and 1:1 with two inputs on, the first block in the left ear and the
    /// second in the right. Everything else hears the average of the two: the waterfall,
    /// the detector, the time expansion recording, and FD, TE and PITCH.
    Both,
}

/// Why the host's mix is silent, for the notice over the waterfall.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MixSilence {
    /// The recorder is stopped with REC PAUSE on.
    RecPause,
    /// The recorder is stopped with REC PAUSE off and the mix has stayed muted.
    Stopped,
    /// One of the host's settings pages is open underneath.
    PageOpen,
}

/// What the host passes to `process`: two inputs, or the left and right of its
/// own stereo mix.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Feed {
    Inputs,
    Mix,
}

/// What an input is called: `short` beside its meter, `long` on the settings
/// screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct InputName {
    pub short: &'static str,
    pub long: &'static str,
}

/// What the app hears: live input, or a recording the X8 is playing. It sets
/// the screen's title and the advice in the wrong-rate notice.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InputKind {
    Live,
    Playback,
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

/// The app's large buffers: about 158 KB. Its processing touches them only once per
/// spectrum column or display column, except for the chime test's decoders, which it
/// touches for every sample while that screen is open. They are kept outside
/// [`BatApp`] so that on the X8 the app's own state (about 44 KB, touched for
/// every sample) can sit in fast on-chip memory while these stay in SDRAM.
pub struct Buffers {
    /// Waterfall levels (0 to 254, or `WF_NO_DATA`), one column of `WF_H` per
    /// `WF_COLUMN_MS`, column-major, as a ring: `BatApp::wf_next` is the oldest
    /// column.
    wf: [u8; WF_W * WF_H],
    /// The last `HISTORY` detector columns, every bin as a level, for
    /// snapshots, and their centre sample times.
    history: [u8; HISTORY * BINS],
    history_centre: [u64; HISTORY],
    /// The last call, scaled to the snapshot's size, column-major.
    snapshot: [u8; SNAP_W * SNAP_H],
    /// The chime test's decoders, one per block of input, built when the screen opens.
    chime: [core::mem::MaybeUninit<chime::Decoder>; 2],
    /// PITCH's shifter for the right ear, used only in stereo. The left ear's is in the app's
    /// own state, which on the X8 is in DTCM and has no room for a second.
    pitch_right: PitchShifter,
}

impl Buffers {
    /// All zero, with the chime test's decoders not built yet, for a `static`.
    pub const fn new() -> Self {
        Self {
            wf: [0; WF_W * WF_H],
            history: [0; HISTORY * BINS],
            history_centre: [0; HISTORY],
            snapshot: [0; SNAP_W * SNAP_H],
            chime: [const { core::mem::MaybeUninit::uninit() }; 2],
            pitch_right: PitchShifter::unconfigured(),
        }
    }
}

impl Default for Buffers {
    fn default() -> Self {
        Self::new()
    }
}

pub struct BatApp<'a> {
    sample_rate: f32,
    mode: Mode,
    source: MonitorSource,
    screen: Screen,
    settings_page: SettingsPage,
    /// Selected row on the settings screen; the wheel changes it.
    setting: usize,
    min_bat_index: usize,
    /// Samples processed since start.
    samples: u64,

    het: Heterodyne,
    comb: HetComb<COMB_COUNT>,
    comb_first_hz: f32,
    /// Which of the comb's LOs the finger moves: the nearest at touch down, kept for the drag.
    comb_grip: u8,
    fd: FrequencyDivider,
    fd_index: usize,
    te: TimeExpander<'a>,
    te_index: usize,
    /// While a file plays slowed: samples of the slowed input to pass over before the next one
    /// analysed.
    slowed_phase: usize,
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

    /// The waterfall, history and snapshot (see [`Buffers`]).
    bufs: &'a mut Buffers,
    /// The oldest column of the waterfall ring.
    wf_next: usize,
    /// Index of the display column being filled (sample time / column length).
    wf_column: u64,
    /// The loudest level of each bin over the display column being filled.
    bin_max: [f32; BINS],
    /// The waterfall row each bin falls in.
    bin_row: [u8; BINS],
    /// The level each bin shows as black in the snapshot history: the floor of
    /// its waterfall row. The floors change once per display column.
    bin_floor: [f32; BINS],
    /// Background level of each waterfall row, in dB, followed while no call
    /// is in progress.
    row_floor: [f32; WF_H],
    row_floor_set: bool,
    display_offset_db: f32,
    display_range_db: f32,
    /// Set each row's floor from that row's own background, instead of one
    /// floor for the whole waterfall.
    display_per_row: bool,

    history_next: usize,
    /// The frequency range the snapshot shows.
    snapshot_hz: (f32, f32),

    /// The chunks now being passed are not analysed; see
    /// [`set_skip_analysis`](Self::set_skip_analysis).
    skip_analysis: bool,
    chime: chime_test::ChimeTest,
    cpu_test: cpu_test::CpuTest,
    remote: remote::Remote,
    player: player::Player<'a>,
    /// A finger on the screen that may move: where it touched down, where it is now, whether it
    /// has moved far enough to be a drag, and the list's first row when it touched down.
    gesture: Gesture,
    /// The screen the file information view returns to: the list, or the main screen.
    info_from: Screen,
    skipped_chunks: u32,
    /// The next chunk does not follow on from the last one.
    input_gap: bool,
    /// Frames lost before the next chunk that the analyzer has not counted yet.
    lost_frames: u32,
    /// The offset added to the input of the listening modes after lost input, and
    /// how many samples of its fade are left.
    join_step: f32,
    join_left: u32,
    /// The last sample passed to the listening modes.
    last_mono: f32,
    /// The same offset for each ear's own input, for the stereo chains, and the last
    /// sample passed to the chain each ear hears. With one chain that is the mono
    /// sample in both, so a change to stereo at a join starts from what was heard.
    /// `mono` is faded apart from these because the time expansion recording uses it.
    join_step_ears: [f32; 2],
    last_ears: [f32; 2],
    /// An extra offset on the mono sound passed to HET or COMB, for a join in the same
    /// chunk as a change from stereo to one chain. That chain's filter last received the
    /// left ear's sample. `join_step` starts from `last_mono`, the average, which is what
    /// the time expansion recording last received.
    join_step_filter: f32,
    /// Whether the last chunk gave each ear its own chain.
    stereo: bool,
    /// The offset added to each ear's output after a change between one chain and two,
    /// and how many samples of its fade are left, so neither ear steps to its new
    /// sound. `last_out` is the last output sample of each ear.
    switch_step: [f32; 2],
    switch_left: u32,
    last_out: [f32; 2],

    meters: [Meter; 2],
    recording: bool,
    recorded_samples: u64,
    /// The recorder's state and take length in seconds, when a recorder
    /// outside the app (the X8's own) is running and REC belongs to it.
    external_recorder: Option<(Recorder, u64)>,
    /// The input's sample rate in Hz, when it is not one the app works at.
    rate_warning: Option<u32>,
    input_kind: InputKind,
    /// The names of the inputs passed to `process`. The second is `None` when
    /// both blocks carry the same input, and both are `None` when no input is
    /// on and the blocks are silent. On the host's mix these name the inputs turned
    /// on, and the blocks carry the mix's two sides: see [`set_inputs`](Self::set_inputs).
    inputs: [Option<InputName>; 2],
    /// Whether the host offers its stereo mix, and whether Listen to has it
    /// chosen.
    mix_available: bool,
    feed: Feed,
    /// Why the host's mix is silent, if it is.
    mix_silence: Option<MixSilence>,
    /// Frames passed to `process` while [`idle`](BatApp::idle), not yet counted as lost.
    idle_frames: u32,
    /// Listen to while the input kind is playback, apart from the choice for
    /// live input.
    playback_source: MonitorSource,
    level: level::AutoLevel,
    /// Whether the host draws figures for testing, and whether it has any.
    debug_available: bool,
    debug_info: bool,
    /// The host's build number, for the CPU test's results.
    host_build: Option<u32>,
    stages: stages::StageClock,
    mix_file_available: bool,
    mix_file: bool,
    /// MANUAL's MASTER has REC READY on, so a take can have a MIX file.
    manual_mix_file: bool,
    /// Whether the take being recorded writes a MIX file, or None with no take's files open.
    take_mix_file: Option<bool>,
    /// The latest chime that gave the phone's time, and one not yet passed to the host with
    /// whether to set the clock from it.
    phone: Option<phone_time::Reading>,
    phone_out: Option<(phone_time::Reading, bool)>,
    clock_from_remote: bool,
    pause_option_available: bool,
    pause_with_rec: bool,
    /// See [`mute_while_recording`](Self::mute_while_recording).
    mute_while_recording: bool,
    power_option_available: bool,
    /// See [`cells_on_weak_usb`](Self::cells_on_weak_usb).
    cells_on_weak_usb: bool,
    draw_option_available: bool,
    /// See [`draw_on_input`](Self::draw_on_input).
    draw_on_input: bool,
    /// The headphones' gain for Mute while recording: 1 heard, 0 silent, between while fading.
    listen_gain: f32,
    /// The listening modes were skipped while muted, so they start clean when the sound returns.
    modes_skipped: bool,
    /// Until this sample, the main screen says that REC did not pause the take.
    pause_refused_until: u64,
    /// The host's clock: local seconds since 1970 at an app sample.
    host_clock: Option<(i64, u64)>,
    clock_state: ClockState,
    guano_available: bool,
    guano: bool,
    /// The mode buttons, left to right.
    modes: &'static [Mode],
    clock_s: u32,

    mono: [f32; CHUNK],
    /// The listening mode's output with one chain. With two, in a chunk with a join
    /// fade, the left ear's faded input.
    out: [f32; CHUNK],
    /// The time expander's output. With two chains the modes do not use it, so in a
    /// chunk with a join fade it holds the right ear's faded input.
    te_out: [f32; CHUNK],
}

impl<'a> BatApp<'a> {
    /// `te_storage` holds the time expander's ring buffer: one second
    /// (`sample_rate` samples) is plenty for the default 300 ms window.
    ///
    /// This returns the whole app (about 250 KB) by value. On the X8, where it
    /// cannot pass through a stack, build it with
    /// [`init_in_place`](Self::init_in_place) instead.
    pub fn new(sample_rate: f32, te_storage: &'a mut [f32], buffers: &'a mut Buffers) -> Self {
        let mut app = core::mem::MaybeUninit::<Self>::uninit();
        // SAFETY: init_in_place writes every field.
        unsafe {
            Self::init_in_place(app.as_mut_ptr(), sample_rate, te_storage, buffers);
            app.assume_init()
        }
    }

    /// Build the app at `this`, field by field, so that no more than its
    /// largest part (a pitch shifter, about 17 KB) passes through the stack.
    ///
    /// # Safety
    ///
    /// `this` must be valid for writes and aligned. Whatever it held is
    /// overwritten without being dropped.
    pub unsafe fn init_in_place(
        this: *mut Self,
        sample_rate: f32,
        te_storage: &'a mut [f32],
        buffers: &'a mut Buffers,
    ) {
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
            field!((*this).source).write(MonitorSource::Both);
            field!((*this).screen).write(Screen::Main);
            field!((*this).settings_page).write(SettingsPage::Listen);
            field!((*this).setting).write(0);
            field!((*this).min_bat_index).write(MIN_BAT_DEFAULT);
            field!((*this).samples).write(0);
            field!((*this).het).write(Heterodyne::new(sample_rate, 45_000.0));
            field!((*this).comb).write(HetComb::new(sample_rate, 25_000.0, DEFAULT_COMB_SPACING_HZ, COMB_COUNT));
            field!((*this).comb_first_hz).write(25_000.0);
            field!((*this).comb_grip).write(0);
            field!((*this).fd).write(FrequencyDivider::new(sample_rate));
            field!((*this).fd_index).write(2);
            field!((*this).te).write(TimeExpander::new(sample_rate, te_storage));
            field!((*this).te_index).write(2);
            field!((*this).slowed_phase).write(0);
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
            buffers.wf.fill(0);
            buffers.history.fill(0);
            buffers.history_centre.fill(0);
            buffers.snapshot.fill(0);
            // Ready from `Buffers::new`, or all zero: either way `init` sets every setting.
            buffers.pitch_right.init(sample_rate);
            field!((*this).bufs).write(buffers);
            field!((*this).wf_next).write(0);
            field!((*this).wf_column).write(0);
            fill(field!((*this).bin_max), f32::MIN);
            fill(field!((*this).bin_floor), -110.0 + DEFAULT_DISPLAY_OFFSET_DB);
            let bin_hz = sample_rate / FFT_N as f32;
            for k in 0..BINS {
                let t = ((VIEW_HI_HZ - k as f32 * bin_hz) / (VIEW_HI_HZ - VIEW_LO_HZ)).clamp(0.0, 1.0);
                let row = ((t * WF_H as f32) as usize).min(WF_H - 1);
                field!((*this).bin_row).cast::<u8>().add(k).write(row as u8);
            }
            fill(field!((*this).row_floor), -110.0);
            field!((*this).row_floor_set).write(false);
            field!((*this).display_offset_db).write(DEFAULT_DISPLAY_OFFSET_DB);
            field!((*this).display_range_db).write(DEFAULT_DISPLAY_RANGE_DB);
            field!((*this).display_per_row).write(false);
            field!((*this).history_next).write(0);
            field!((*this).snapshot_hz).write((VIEW_LO_HZ, VIEW_HI_HZ));
            field!((*this).skip_analysis).write(false);
            field!((*this).chime).write(chime_test::ChimeTest::new());
            field!((*this).cpu_test).write(cpu_test::CpuTest::new());
            field!((*this).remote).write(remote::Remote::new());
            field!((*this).player).write(player::Player::new());
            field!((*this).gesture).write(Gesture::NONE);
            field!((*this).info_from).write(Screen::Files);
            field!((*this).skipped_chunks).write(0);
            field!((*this).input_gap).write(false);
            field!((*this).lost_frames).write(0);
            field!((*this).join_step).write(0.0);
            field!((*this).join_left).write(0);
            field!((*this).last_mono).write(0.0);
            field!((*this).join_step_ears).write([0.0; 2]);
            field!((*this).last_ears).write([0.0; 2]);
            field!((*this).join_step_filter).write(0.0);
            field!((*this).stereo).write(false);
            field!((*this).switch_step).write([0.0; 2]);
            field!((*this).switch_left).write(0);
            field!((*this).last_out).write([0.0; 2]);
            field!((*this).meters).write([Meter::new(); 2]);
            field!((*this).recording).write(false);
            field!((*this).recorded_samples).write(0);
            field!((*this).external_recorder).write(None);
            field!((*this).rate_warning).write(None);
            field!((*this).input_kind).write(InputKind::Live);
            field!((*this).inputs).write([
                Some(InputName { short: "3", long: "Input 3" }),
                Some(InputName { short: "4", long: "Input 4" }),
            ]);
            field!((*this).mix_available).write(false);
            field!((*this).feed).write(Feed::Inputs);
            field!((*this).mix_silence).write(None);
            field!((*this).idle_frames).write(0);
            field!((*this).playback_source).write(MonitorSource::Both);
            field!((*this).level).write(level::AutoLevel::new(sample_rate));
            field!((*this).debug_available).write(false);
            field!((*this).debug_info).write(false);
            field!((*this).host_build).write(None);
            field!((*this).stages).write(stages::StageClock::new());
            field!((*this).mix_file_available).write(false);
            field!((*this).mix_file).write(true);
            field!((*this).manual_mix_file).write(true);
            field!((*this).take_mix_file).write(None);
            field!((*this).phone).write(None);
            field!((*this).phone_out).write(None);
            field!((*this).clock_from_remote).write(true);
            field!((*this).pause_option_available).write(false);
            field!((*this).pause_with_rec).write(false);
            field!((*this).mute_while_recording).write(false);
            field!((*this).power_option_available).write(false);
            field!((*this).cells_on_weak_usb).write(true);
            field!((*this).draw_option_available).write(false);
            field!((*this).draw_on_input).write(false);
            field!((*this).listen_gain).write(1.0);
            field!((*this).modes_skipped).write(false);
            field!((*this).pause_refused_until).write(0);
            field!((*this).host_clock).write(None);
            field!((*this).clock_state).write(ClockState::Idle);
            field!((*this).guano_available).write(false);
            field!((*this).guano).write(true);
            field!((*this).modes).write(&Mode::ALL);
            field!((*this).clock_s).write(0);
            fill(field!((*this).mono), 0.0);
            fill(field!((*this).out), 0.0);
            fill(field!((*this).te_out), 0.0);

            let app = &mut *this;
            app.remote.configure(sample_rate as u32, bin_hz);
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

    /// The modes offered, in button order.
    pub fn modes(&self) -> &'static [Mode] {
        self.modes
    }

    /// Offer only these modes (at least one). If the current mode is not
    /// among them, the first one is selected.
    pub fn set_modes(&mut self, modes: &'static [Mode]) {
        if modes.is_empty() {
            return;
        }
        self.modes = modes;
        if !modes.contains(&self.mode) {
            self.set_mode(modes[0]);
        }
    }

    /// The next mode in button order, after the last the first: what the
    /// X8's PLAY button selects.
    pub fn next_mode(&mut self) {
        let i = self.modes.iter().position(|&m| m == self.mode).map_or(0, |i| i + 1);
        self.set_mode(self.modes[i % self.modes.len()]);
    }

    pub fn set_mode(&mut self, mode: Mode) {
        if mode == self.mode {
            return;
        }
        self.mode = mode;
        self.level.sound_changed();
        self.reset_mode();
    }

    /// Start the current mode clean, so no stale sound from its last use plays.
    fn reset_mode(&mut self) {
        match self.mode {
            Mode::Het => self.het.reset(),
            Mode::Comb => self.comb.reset(),
            Mode::Fd => self.fd.reset(),
            Mode::Te => {}
            Mode::Pitch => {
                self.pitch.reset();
                self.bufs.pitch_right.reset();
            }
            Mode::Thru => {}
        }
    }

    pub fn set_source(&mut self, source: MonitorSource) {
        if self.input_kind == InputKind::Playback {
            self.playback_source = source;
        } else {
            self.source = source;
        }
    }

    pub fn source(&self) -> MonitorSource {
        if self.input_kind == InputKind::Playback { self.playback_source } else { self.source }
    }

    pub fn screen(&self) -> Screen {
        self.screen
    }

    /// Show the settings screen, as the menu button does, on the page it last showed.
    pub fn open_settings(&mut self) {
        self.screen = Screen::Settings;
    }

    /// Show a page of the settings screen, as its tab does. The wheel then changes the page's first
    /// setting that this host shows, if it has one.
    pub fn open_settings_page(&mut self, page: SettingsPage) {
        self.screen = Screen::Settings;
        if self.settings_page() != page {
            self.settings_page = page;
            if let Some(i) = (0..Setting::ALL.len()).find(|&i| Setting::ALL[i].page() == page && self.setting_shown(i))
            {
                self.setting = i;
            }
        }
    }

    /// The page shown: the one last picked, or Listen if this host no longer offers it.
    pub fn settings_page(&self) -> SettingsPage {
        if self.settings_pages().any(|page| page == self.settings_page) {
            self.settings_page
        } else {
            SettingsPage::Listen
        }
    }

    /// The pages this host has anything for: Record is left out when it offers none of the MIX
    /// file, GUANO metadata and pausing, and Tests when it has no debug figures. Phone always has
    /// the QR code and the chime test.
    pub fn settings_pages(&self) -> impl Iterator<Item = SettingsPage> + '_ {
        SettingsPage::ALL.into_iter().filter(|&page| match page {
            SettingsPage::Record => {
                self.mix_file_available
                    || self.guano_available
                    || self.pause_option_available
                    || self.power_option_available
            }
            SettingsPage::Tests => self.debug_available,
            SettingsPage::Listen | SettingsPage::Phone => true,
        })
    }

    /// The settings on the page shown that this host shows, top to bottom.
    fn page_settings(&self) -> impl Iterator<Item = Setting> + '_ {
        (0..Setting::ALL.len())
            .filter(|&i| Setting::ALL[i].page() == self.settings_page() && self.setting_shown(i))
            .map(|i| Setting::ALL[i])
    }

    /// Show the chime test, which starts listening with nothing read.
    pub fn open_chime_test(&mut self) {
        self.screen = Screen::ChimeTest;
        self.remote.decoders_taken();
        self.chime.restart(&mut self.bufs.chime, self.sample_rate as u32);
    }

    /// Show the CPU test's screen, which offers the test and shows the latest results.
    pub fn open_cpu_test(&mut self) {
        self.screen = Screen::CpuTest;
        if self.cpu_test.count == 0 || !self.cpu_test.has_results() {
            self.cpu_test.plan(self.modes, self.inputs[1].is_some());
        }
    }

    /// Whether the CPU test is running.
    pub fn cpu_test_running(&self) -> bool {
        self.cpu_test.running
    }

    /// The CPU test's rows: each one's settings, and its figures if it has run.
    pub fn cpu_test_rows(&self) -> &[cpu_test::Row] {
        &self.cpu_test.rows[..self.cpu_test.count]
    }

    /// Run the CPU test from its first row, on the main screen.
    pub fn start_cpu_test(&mut self) {
        self.cpu_test.plan(self.modes, self.inputs[1].is_some());
        self.cpu_test.saved =
            Some(cpu_test::Saved { mode: self.mode, volume: self.level.volume(), source: self.source() });
        self.cpu_test.running = true;
        self.screen = Screen::Main;
        self.begin_cpu_test_row();
    }

    fn begin_cpu_test_row(&mut self) {
        let Some(row) = self.cpu_test.current().copied() else {
            self.stop_cpu_test();
            return;
        };
        self.set_mode(row.mode);
        self.level.set_volume(row.volume);
        self.set_source(row.source());
        self.remote.set_forced(row.decoders);
        self.cpu_test.begin_row(self.samples);
    }

    /// End the CPU test and show what it has measured so far.
    fn stop_cpu_test(&mut self) {
        self.cpu_test.running = false;
        self.remote.set_forced(false);
        if let Some(saved) = self.cpu_test.saved.take() {
            self.set_mode(saved.mode);
            self.level.set_volume(saved.volume);
            self.set_source(saved.source);
        }
        self.screen = Screen::CpuTest;
    }

    /// The host's figures, at each redraw while the CPU test runs.
    pub fn cpu_test_figures(&mut self, figures: HostFigures) {
        if !self.cpu_test.running {
            return;
        }
        let recorder = self.recorder().0;
        let state = (recorder, self.take_mix_file);
        let finished = self.cpu_test.figures(
            figures,
            state,
            self.skipped_chunks,
            &self.stages,
            self.samples,
            self.sample_rate as u64,
        );
        if finished {
            self.cpu_test.at += 1;
            self.begin_cpu_test_row();
        }
    }

    /// Whether the host should pass its input through [`add_test_calls`](Self::add_test_calls).
    pub fn test_calls_on(&self) -> bool {
        self.cpu_test.running && self.cpu_test.test_calls
    }

    /// Add the CPU test's synthetic bat calls to the blocks about to be passed to
    /// [`process`](Self::process). They reach the headphones and the waterfall, and not a
    /// recorder outside the app.
    pub fn add_test_calls(&mut self, first: &mut [f32], second: &mut [f32]) {
        self.cpu_test.add_calls(self.sample_rate, first, second);
    }

    /// The host can start and stop its recorder: offer the phone remote.
    pub fn set_remote_available(&mut self, available: bool) {
        self.remote.available = available;
    }

    pub fn remote_available(&self) -> bool {
        self.remote.available
    }

    /// Whether the phone remote's switch is on, and the host offers the remote.
    pub fn remote_on(&self) -> bool {
        self.remote.enabled()
    }

    /// Turn the phone remote on or off.
    pub fn set_remote_on(&mut self, on: bool) {
        self.remote.on = on;
    }

    /// Tones heard, chimes read and chimes that failed, since the host started.
    pub fn remote_counts(&self) -> (u32, u32, u32) {
        self.remote.counts()
    }

    /// A start or stop command from the phone that the host has not acted on yet.
    pub fn take_remote_command(&mut self) -> Option<RecorderCommand> {
        self.remote.take_command()
    }

    /// What came of a command: shows on the main screen and beeps in the headphones.
    pub fn remote_result(&mut self, result: RemoteResult) {
        self.remote.report(result, self.samples);
    }

    /// How many blocks of input the chime test listens to: with one input on, both blocks
    /// carry it, except on the mix, where the input may be panned to either side.
    fn chime_channels(&self) -> usize {
        match (self.feed(), self.inputs) {
            (_, [None, _]) => 0,
            (Feed::Inputs, [_, None]) => 1,
            _ => 2,
        }
    }

    /// Whether the chunks passed now are analysed.
    fn analysis_runs(&self) -> bool {
        !self.skip_analysis && self.screen != Screen::ChimeTest
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
                let (choices, n) = self.listen_choices();
                if n > 1 {
                    // With one input, every source means that input, whichever is
                    // stored. The stored one comes back with a second input, unless
                    // it has been stepped since, which stores the new choice.
                    let one_input = self.feed() == Feed::Inputs && self.inputs[1].is_none();
                    let current = (self.feed(), if one_input { MonitorSource::First } else { self.source() });
                    // A choice no longer offered counts as one past the end, so
                    // minus gives the last choice and plus changes nothing.
                    let i = choices[..n].iter().position(|&c| c == current).unwrap_or(n);
                    let new = step(i, n);
                    if new != i && (i < n || steps < 0) {
                        let (feed, source) = choices[new];
                        self.level.sound_changed();
                        if self.input_kind == InputKind::Playback {
                            self.playback_source = source;
                        } else {
                            (self.feed, self.source) = (feed, source);
                        }
                    }
                }
            }
            Setting::AutoLevel => {
                let i = AutoVolume::ALL.iter().position(|&v| v == self.level.volume()).unwrap_or(0);
                self.level.set_volume(AutoVolume::ALL[step(i, AutoVolume::ALL.len())]);
            }
            // Turning the wheel up switches on, down switches off.
            Setting::DebugInfo if steps != 0 => self.debug_info = steps > 0,
            Setting::DebugInfo => {}
            Setting::MixFile if steps != 0 => self.mix_file = steps > 0,
            Setting::MixFile => {}
            Setting::ClockFromRemote if steps != 0 => self.clock_from_remote = steps > 0,
            Setting::ClockFromRemote => {}
            Setting::PauseWithRec if steps != 0 => self.pause_with_rec = steps > 0,
            Setting::PauseWithRec => {}
            Setting::MuteWhileRecording if steps != 0 => self.mute_while_recording = steps > 0,
            Setting::MuteWhileRecording => {}
            Setting::CellsOnWeakUsb if steps != 0 => self.cells_on_weak_usb = steps > 0,
            Setting::CellsOnWeakUsb => {}
            Setting::DrawOnInput if steps != 0 => self.draw_on_input = steps > 0,
            Setting::DrawOnInput => {}
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
        self.recorder().0 == Recorder::Recording
    }

    /// The recorder's state and the length of the current take in seconds.
    pub fn recorder(&self) -> (Recorder, u64) {
        match self.external_recorder {
            Some(r) => r,
            None if self.recording => (Recorder::Recording, self.recorded_seconds()),
            None => (Recorder::Stopped, self.recorded_seconds()),
        }
    }

    /// Show a recorder outside the app instead of the app's own: on the X8,
    /// REC goes to the X8's recorder and this reports what it is doing.
    pub fn set_recorder(&mut self, state: Recorder, take_seconds: u64) {
        self.external_recorder = Some((state, take_seconds));
    }

    /// The input's sample rate in Hz, to warn when it is not the one the app
    /// was built for (the warning covers the waterfall). Rates within 1 Hz
    /// of the app's clear it.
    pub fn set_input_rate(&mut self, rate_hz: u32) {
        let ok = (rate_hz as f32 - self.sample_rate).abs() < 1.0;
        self.rate_warning = if ok { None } else { Some(rate_hz) };
    }

    pub fn rate_warning(&self) -> Option<u32> {
        self.rate_warning
    }

    /// A change of kind is a change of tile (BAT DETECTOR or BAT PLAYBACK), so the app opens at
    /// its main screen: the other tile's file list or settings page is not shown.
    pub fn set_input_kind(&mut self, kind: InputKind) {
        if kind != self.input_kind {
            self.screen = Screen::Main;
            self.gesture = Gesture::NONE;
        }
        self.input_kind = kind;
    }

    pub fn input_kind(&self) -> InputKind {
        self.input_kind
    }

    /// Name the inputs passed to [`process`](Self::process). With the second
    /// `None`, both blocks must carry the first input: one meter shows, and
    /// Listen to has nothing to choose. With both `None`, no input is on: a
    /// notice covers the waterfall. On the host's mix (see [`set_feed`](Self::set_feed))
    /// these name the inputs turned on, but the blocks carry the mix's left and right
    /// whatever they are, so both meters show and Listen to keeps its choices. A second
    /// name also gives each ear its own chain on the mix: see `wants_stereo`.
    pub fn set_inputs(&mut self, inputs: [Option<InputName>; 2]) {
        self.inputs = inputs;
    }

    pub fn inputs(&self) -> [Option<InputName>; 2] {
        self.inputs
    }

    /// Offer the host's stereo mix in Listen to. The host reads the choice with
    /// [`feed`](Self::feed) and passes the mix's left and right to `process`.
    pub fn set_mix_available(&mut self, available: bool) {
        self.mix_available = available;
    }

    pub fn set_feed(&mut self, feed: Feed) {
        self.feed = feed;
    }

    pub fn feed(&self) -> Feed {
        if self.mix_available { self.feed } else { Feed::Inputs }
    }

    /// The host's mix is silent: a small notice shows in a corner of the waterfall.
    pub fn set_mix_silence(&mut self, silence: Option<MixSilence>) {
        self.mix_silence = silence;
    }

    pub fn mix_silence(&self) -> Option<MixSilence> {
        self.mix_silence
    }

    /// The host's mix is muted (see [`set_mix_silence`](Self::set_mix_silence)) and nothing in
    /// the app needs the input or the time it counts: no time-expansion capture or playback, no
    /// CPU test, no phone remote beep, and no notice timed in samples on screen. While
    /// idle, [`process`](Self::process) only outputs silence and lets the meters fall, the
    /// waterfall stands still, and the host can redraw less often. The frames passed meanwhile
    /// count as lost input when processing starts again.
    pub fn idle(&self) -> bool {
        self.mix_silence.is_some()
            && self.input_kind == InputKind::Live
            && !self.cpu_test.running
            && matches!(self.te.state(), ExpanderState::Idle)
            && !self.remote.beeping()
            && self.remote.result_shown(self.samples).is_none()
            && !self.pause_refused_shown()
    }

    /// The level automatic headphone level aims for, in the units of
    /// `process`'s output (0.25 unless set).
    pub fn set_auto_level_target(&mut self, target: f32) {
        self.level.set_target(target);
    }

    pub fn auto_volume(&self) -> AutoVolume {
        self.level.volume()
    }

    pub fn set_auto_volume(&mut self, volume: AutoVolume) {
        self.level.set_volume(volume);
    }

    /// The gain Auto volume is using, for tuning tools.
    pub fn auto_volume_gain(&self) -> f32 {
        self.level.gain()
    }

    /// How far open the Bats only gate is, from 0 to 1, for tuning tools.
    pub fn auto_volume_gate(&self) -> f32 {
        self.level.gate_level()
    }

    /// The host has figures for testing to show: offer the switch for them.
    pub fn set_debug_available(&mut self, available: bool) {
        self.debug_available = available;
    }

    pub fn debug_available(&self) -> bool {
        self.debug_available
    }

    /// The host's build number, such as the X8 patch's B240. The CPU test shows it beside its
    /// results, so a photo of them says which build measured them. 0 means unknown.
    pub fn set_host_build(&mut self, build: u32) {
        self.host_build = (build != 0).then_some(build);
    }

    pub fn host_build(&self) -> Option<u32> {
        self.host_build
    }

    /// A counter that the host's clock moves on, `ticks_per_second` times a second, for the
    /// CPU test's Stages table. See [`Stage`].
    pub fn set_stage_clock(&mut self, ticks: &'static core::sync::atomic::AtomicU32, ticks_per_second: u32) {
        self.stages.set(ticks, ticks_per_second);
    }

    pub(crate) fn stages_available(&self) -> bool {
        self.stages.running()
    }

    pub fn debug_info(&self) -> bool {
        self.debug_info
    }

    /// The host records takes with a MIX file it can leave out: offer the switch.
    pub fn set_mix_file_available(&mut self, available: bool) {
        self.mix_file_available = available;
    }

    pub fn mix_file_available(&self) -> bool {
        self.mix_file_available
    }

    /// The MIX file switch: on (the default), a take has a MIX file when MANUAL's MASTER has REC
    /// READY on ([`set_manual_mix_file`](Self::set_manual_mix_file)); off, it has none.
    pub fn mix_file(&self) -> bool {
        self.mix_file
    }

    /// Whether MANUAL's MASTER has REC READY on, so a take can have a MIX file. The MIX file
    /// panel's line says which of the two decides.
    pub fn set_manual_mix_file(&mut self, on: bool) {
        self.manual_mix_file = on;
    }

    pub fn manual_mix_file(&self) -> bool {
        self.manual_mix_file
    }

    /// Whether the take being recorded writes a MIX file, or None with no take's files open. The CPU
    /// test shows it, since the MIX file costs CPU while recording.
    pub fn set_take_mix_file(&mut self, written: Option<bool>) {
        self.take_mix_file = written;
    }

    pub fn take_mix_file(&self) -> Option<bool> {
        self.take_mix_file
    }

    /// The phone's time from the latest chime read, once for each chime: from the phone remote on
    /// the main screen, and from the clock screen. `set_clock` says whether to set the host's
    /// clock from it.
    pub fn take_phone_time(&mut self) -> Option<PhoneTime> {
        let (reading, set_clock) = self.phone_out.take()?;
        Some(PhoneTime {
            local_ms: reading.local_ms_at(self.samples, self.sample_rate as u32),
            timezone_minutes: reading.timezone_minutes,
            location: reading.location,
            set_clock,
        })
    }

    /// The host's clock now, as local seconds since 1970, for the clock screen to compare with the
    /// phone's. None when the host has not read it.
    pub fn set_host_clock(&mut self, local_seconds: Option<i64>) {
        self.host_clock = local_seconds.map(|s| (s, self.samples));
    }

    /// What the host's clock is doing, for the clock screen. The outcome of a phone's clock command
    /// also beeps and shows on the main screen.
    pub fn set_clock_state(&mut self, state: ClockState) {
        self.clock_state = state;
        if self.remote.clock_pending {
            let result = match state {
                ClockState::Set(_) => RemoteResult::ClockSet,
                ClockState::AfterTake => RemoteResult::ClockAfterTake,
                ClockState::Failed => RemoteResult::ClockFailed,
                ClockState::Idle | ClockState::Setting => return,
            };
            self.remote.clock_pending = false;
            self.remote.report(result, self.samples);
        }
    }

    pub fn clock_state(&self) -> ClockState {
        self.clock_state
    }

    /// The host can keep REC from pausing takes: offer the switch.
    pub fn set_pause_option_available(&mut self, available: bool) {
        self.pause_option_available = available;
    }

    pub fn pause_option_available(&self) -> bool {
        self.pause_option_available
    }

    /// Whether REC may pause a take. Off by default, so takes have no gaps.
    pub fn pause_with_rec(&self) -> bool {
        self.pause_with_rec
    }

    /// Whether the headphones are silent while a take is being recorded (not in record standby
    /// or while paused), so the sound from headphones or a speaker is not recorded. The
    /// listening mode is skipped then, which saves its CPU. Off by default. The CPU test plays
    /// every mode whatever this says.
    pub fn mute_while_recording(&self) -> bool {
        self.mute_while_recording
    }

    pub fn set_mute_while_recording(&mut self, on: bool) {
        self.mute_while_recording = on;
    }

    /// The host can run from its cells when a USB supply is weak (the X8 patch, in place of the
    /// stock question that leads to POWER SAVE MODE): offer the switch.
    pub fn set_power_option_available(&mut self, available: bool) {
        self.power_option_available = available;
    }

    pub fn power_option_available(&self) -> bool {
        self.power_option_available
    }

    /// Whether the host runs from its cells when a USB supply is weak. On by default, and at every
    /// start, since the settings reset at power-off.
    pub fn cells_on_weak_usb(&self) -> bool {
        self.cells_on_weak_usb
    }

    pub fn set_cells_on_weak_usb(&mut self, on: bool) {
        self.cells_on_weak_usb = on;
    }

    /// The host can stop redrawing the screen (the X8 patch): offer the switch on the Tests page,
    /// which is shown when the host has debug figures.
    pub fn set_draw_option_available(&mut self, available: bool) {
        self.draw_option_available = available;
    }

    pub fn draw_option_available(&self) -> bool {
        self.debug_available && self.draw_option_available
    }

    /// Whether the host redraws the screen only after a touch, button or wheel turn, and leaves
    /// the last frame up otherwise. For measuring what the drawing costs, in power or processor
    /// time, against a screen that is redrawn all the time. Off by default. The CPU test measures
    /// the drawing, so the host draws as usual while it runs.
    pub fn draw_on_input(&self) -> bool {
        self.draw_on_input && !self.cpu_test.running
    }

    /// The switch's own state, for the settings page.
    pub fn draw_on_input_switch(&self) -> bool {
        self.draw_on_input
    }

    pub fn set_draw_on_input(&mut self, on: bool) {
        self.draw_on_input = on;
    }

    /// Whether Mute while recording is silencing the headphones now (or fading them out).
    pub fn muted_for_take(&self) -> bool {
        self.mute_while_recording && self.input_kind == InputKind::Live && self.is_recording() && !self.cpu_test.running
    }

    /// The host kept REC from pausing a take: the main screen says so for a few seconds.
    pub fn pause_refused(&mut self) {
        self.pause_refused_until = self.samples + PAUSE_REFUSED_S * self.sample_rate as u64;
    }

    pub(crate) fn pause_refused_shown(&self) -> bool {
        self.samples < self.pause_refused_until
    }

    /// Whether a phone remote chime also sets the host's clock. On by default.
    pub fn clock_from_remote(&self) -> bool {
        self.clock_from_remote
    }

    /// Ask the host to set its clock from the latest chime, as the clock screen's button does.
    /// Does nothing before a chime has been read.
    pub fn set_clock_from_phone(&mut self) {
        if let Some(reading) = self.phone {
            self.phone_out = Some((reading, true));
        }
    }

    /// The phone's time now and the host's, as local seconds since 1970, once both are known.
    pub(crate) fn clocks_now(&self) -> Option<(i64, i64)> {
        let phone = self.phone?.local_ms_at(self.samples, self.sample_rate as u32).div_euclid(1000);
        let (host, at) = self.host_clock?;
        Some((phone, host + (self.samples.saturating_sub(at) as f32 / self.sample_rate) as i64))
    }

    /// The readings the decoders gave this chunk, kept and queued for the host, and a listening
    /// mode the phone asked for. A clock command sets the clock from either screen, whatever the
    /// setting for other phone chimes. Not inlined, so it stays out of `process`, which the X8
    /// patch runs from ITCM.
    #[inline(never)]
    fn collect_phone_readings(&mut self) {
        if let Some((reading, clock_command)) = self.chime.take_reading() {
            self.phone = Some(reading);
            self.phone_out = Some((reading, clock_command));
            self.remote.clock_pending |= clock_command;
        }
        if let Some((reading, clock_command)) = self.remote.take_reading() {
            self.phone = Some(reading);
            self.phone_out = Some((reading, clock_command || self.clock_from_remote));
        }
        if let Some(mode) = self.remote.take_mode() {
            let mode = Mode::from(mode);
            let result = if self.modes.contains(&mode) {
                self.set_mode(mode);
                RemoteResult::ModeSet(mode)
            } else {
                RemoteResult::ModeNotOffered(mode)
            };
            self.remote.report(result, self.samples);
        }
    }

    /// The host can add GUANO metadata to its takes' WAV files: offer the switch on the tools
    /// screen.
    pub fn set_guano_available(&mut self, available: bool) {
        self.guano_available = available;
    }

    pub fn guano_available(&self) -> bool {
        self.guano_available
    }

    /// Whether takes should get GUANO metadata. On by default.
    pub fn guano(&self) -> bool {
        self.guano
    }

    /// Turn GUANO metadata on or off, as the switch does.
    pub fn set_guano(&mut self, on: bool) {
        self.guano = on;
    }

    /// Whether switch `i` of `Setting::ALL` is shown on this host.
    fn setting_shown(&self, i: usize) -> bool {
        match Setting::ALL[i] {
            Setting::DebugInfo => self.debug_available,
            Setting::MixFile => self.mix_file_available,
            Setting::ClockFromRemote => self.remote.available,
            Setting::PauseWithRec => self.pause_option_available,
            Setting::MuteWhileRecording => self.input_kind == InputKind::Live,
            Setting::CellsOnWeakUsb => self.power_option_available,
            Setting::DrawOnInput => self.draw_option_available(),
            _ => true,
        }
    }

    /// The choices for Listen to, in order: the whole mix, its left and right,
    /// then the inputs.
    fn listen_choices(&self) -> ([(Feed, MonitorSource); 6], usize) {
        let mut choices = [(Feed::Inputs, MonitorSource::Both); 6];
        let mut n = 0;
        let mut add = |sources: &[MonitorSource], feed: Feed| {
            for &source in sources {
                choices[n] = (feed, source);
                n += 1;
            }
        };
        const ALL: [MonitorSource; 3] = [MonitorSource::Both, MonitorSource::First, MonitorSource::Second];
        if self.mix_available {
            add(&ALL, Feed::Mix);
        }
        match self.inputs {
            [Some(_), Some(_)] => add(&ALL, Feed::Inputs),
            [Some(_), None] => add(&ALL[1..2], Feed::Inputs),
            _ => {}
        }
        (choices, n)
    }

    /// Whether Listen to has more than one choice.
    pub fn listen_has_choices(&self) -> bool {
        self.listen_choices().1 > 1
    }

    /// The panel the last call is shown in, which the host's figures replace:
    /// its top row and height.
    pub fn last_call_area(&self) -> (i32, u32) {
        (layout::SNAP_PANEL.top_left.y, layout::SNAP_PANEL.size.height)
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
            // BAT PLAYBACK reads the card, which a take would compete with: REC does nothing.
            Button::Rec if self.external_recorder.is_some() || self.input_kind == InputKind::Playback => {}
            Button::Rec => {
                self.recording = !self.recording;
                if self.recording {
                    self.recorded_samples = 0;
                }
            }
            Button::Mark => {
                self.marks.push(self.samples);
                if self.live_te() {
                    self.trigger_te();
                }
            }
            // HOME closes the settings screen; on the main screen it belongs to
            // the X8's launcher, except in BAT PLAYBACK, where it opens the list.
            Button::Home => {
                if self.cpu_test.running {
                    self.stop_cpu_test();
                }
                if self.input_kind == InputKind::Playback && self.screen == Screen::Main {
                    self.open_files();
                } else if self.screen == Screen::FileInfo {
                    self.screen = self.info_from;
                } else if self.screen == Screen::Folders {
                    self.screen = Screen::Files;
                } else {
                    self.screen = Screen::Main;
                }
            }
            // In BAT PLAYBACK: plays the highlighted file on the list, or plays and pauses.
            Button::Play if self.input_kind == InputKind::Playback => match self.screen {
                Screen::Files => {
                    if self.player.choose(self.player.list.highlight) {
                        self.screen = Screen::Main;
                    }
                }
                Screen::Folders => self.choose_folder(self.player.folder_list.highlight),
                Screen::FileInfo if self.details_play_shown() => self.play_details_file(),
                Screen::FileInfo if self.info_from == Screen::Main => self.play_button(),
                Screen::Main if !self.cpu_test.running => self.play_button(),
                _ => {}
            },
            Button::Play => {}
        }
    }

    /// Turn of the wheel, in detents (positive clockwise): changes the
    /// current mode's setting.
    pub fn wheel(&mut self, steps: i32) {
        if self.screen == Screen::Settings {
            // The wheel changes the selected setting only while its page is shown.
            if self.selected_setting().page() == self.settings_page() && self.setting_shown(self.setting) {
                self.change_setting(self.selected_setting(), steps);
            }
            return;
        }
        if self.screen == Screen::Files {
            self.player.move_highlight(steps, layout::LIST_ROWS);
            return;
        }
        if self.screen == Screen::Folders {
            self.player.move_folder_highlight(steps, layout::LIST_ROWS);
            return;
        }
        if self.screen == Screen::FileInfo {
            self.scroll_details_to(self.player.details_top as i32 + steps);
            return;
        }
        if matches!(self.screen, Screen::ChimeTest | Screen::CpuTest) || self.cpu_test.running {
            return;
        }
        let step = |i: usize, len: usize| (i as i32 + steps).clamp(0, len as i32 - 1) as usize;
        match self.mode {
            Mode::Het => self.set_het_lo(self.het.lo() + steps as f32 * HET_STEP_HZ),
            Mode::Comb => self.set_comb_first(self.comb_first_hz + steps as f32 * COMB_STEP_HZ),
            Mode::Fd => self.fd_index = step(self.fd_index, FD_DIVISIONS.len()),
            Mode::Te => self.te_index = step(self.te_index, TE_FACTORS.len()),
            Mode::Pitch => self.pitch_index = step(self.pitch_index, PITCH_FACTORS.len()),
            Mode::Thru => {}
        }
        self.apply_settings();
    }

    /// A tap on the touchscreen, in screen pixels.
    pub fn touch(&mut self, x: i32, y: i32) {
        // A tap anywhere stops the CPU test, so a tap cannot change a setting under it.
        if self.cpu_test.running {
            self.stop_cpu_test();
            return;
        }
        let p = embedded_graphics::prelude::Point::new(x, y);
        match self.screen {
            Screen::ChimeTest => {
                if layout::CLOCK_BACK.contains(p) {
                    self.screen = Screen::Settings;
                } else if layout::CLOCK_SET.contains(p) {
                    self.set_clock_from_phone();
                }
                return;
            }
            Screen::CpuTest => {
                if layout::CPU_TEST_BACK.contains(p) {
                    self.screen = Screen::Settings;
                } else if layout::CPU_TEST_START.contains(p) && self.inputs[0].is_some() && self.rate_warning.is_none()
                {
                    self.start_cpu_test();
                } else if layout::CPU_TEST_CALLS.contains(p) {
                    self.cpu_test.test_calls = !self.cpu_test.test_calls;
                } else if layout::CPU_TEST_VIEW.contains(p) && self.stages.running() {
                    self.cpu_test.view = match self.cpu_test.view {
                        cpu_test::View::Totals => cpu_test::View::Stages,
                        cpu_test::View::Stages => cpu_test::View::Totals,
                    };
                }
                return;
            }
            Screen::Files => {
                self.gesture = Gesture { down: Some((x, y)), at: (x, y), moved: false, top: self.player.list.top };
                if layout::FILES_BACK.contains(p) {
                    self.gesture = Gesture::NONE;
                    self.screen = Screen::Main;
                } else if layout::FILES_FOLDER.contains(p) {
                    self.gesture = Gesture::NONE;
                    self.open_folders();
                }
                return;
            }
            Screen::FileInfo => {
                self.gesture = Gesture { down: Some((x, y)), at: (x, y), moved: false, top: self.player.details_top };
                let play = self.details_play_shown();
                if layout::info_back(play).contains(p) {
                    self.gesture = Gesture::NONE;
                    self.screen = self.info_from;
                } else if play && layout::INFO_PLAY.contains(p) {
                    self.gesture = Gesture::NONE;
                    self.play_details_file();
                }
                return;
            }
            Screen::Folders => {
                self.gesture =
                    Gesture { down: Some((x, y)), at: (x, y), moved: false, top: self.player.folder_list.top };
                if layout::LIST_BACK.contains(p) {
                    self.gesture = Gesture::NONE;
                    self.screen = Screen::Files;
                }
                return;
            }
            _ => {}
        }
        if self.screen == Screen::Settings {
            self.settings_touch(x, y);
            return;
        }
        if self.input_kind == InputKind::Playback {
            if layout::TITLE_INFO.contains(p) && self.player.show_current_details() {
                self.info_from = Screen::Main;
                self.screen = Screen::FileInfo;
                return;
            }
            if layout::TITLE_MIDDLE.contains(p) {
                self.open_files();
                return;
            }
            // With no file chosen, the whole bar opens the list: it says so.
            if layout::PLAY_BUTTON.contains(p)
                || (layout::STATUS.contains(p) && self.player.transport() == player::Transport::NoFile)
            {
                self.play_button();
                return;
            }
            if layout::SEEK.contains(p) {
                self.gesture = Gesture { down: Some((x, y)), at: (x, y), moved: true, top: 0 };
                return;
            }
        }
        if layout::MENU.contains(embedded_graphics::prelude::Point::new(x, y)) {
            self.screen = Screen::Settings;
        } else if let Some(mode) = layout::mode_button_at(x, y, self.modes) {
            self.set_mode(mode);
        } else if let Some(hz) = layout::waterfall_freq_at(x, y) {
            self.tune_to(hz);
        }
    }

    fn settings_touch(&mut self, x: i32, y: i32) {
        let p = embedded_graphics::prelude::Point::new(x, y);
        let pages = self.settings_pages().count();
        let tapped =
            self.settings_pages().enumerate().find_map(|(i, page)| layout::tab(i, pages).contains(p).then_some(page));
        if let Some(page) = tapped {
            self.open_settings_page(page);
            return;
        }
        match self.settings_page() {
            SettingsPage::Record
                if self.guano_available && layout::panel_switch_row(layout::RECORD_GUANO).contains(p) =>
            {
                self.guano = !self.guano;
                return;
            }
            SettingsPage::Phone
                if self.remote.available && layout::panel_switch_row(layout::PHONE_REMOTE).contains(p) =>
            {
                self.remote.on = !self.remote.on;
                return;
            }
            SettingsPage::Phone if layout::phone_time(self.remote.available).contains(p) => {
                self.open_chime_test();
                return;
            }
            SettingsPage::Tests if self.debug_available && layout::TESTS_CPU_TEST.contains(p) => {
                self.open_cpu_test();
                return;
            }
            _ => {}
        }
        let select = |app: &mut Self, setting: Setting| {
            if let Some(i) = Setting::ALL.iter().position(|&s| s == setting) {
                app.setting = i;
            }
        };
        let hit = layout::settings_hit(x, y, self.page_settings());
        match hit {
            Some(layout::SettingsHit::Row(setting)) => select(self, setting),
            Some(layout::SettingsHit::Control(setting)) => {
                select(self, setting);
                match setting {
                    Setting::Mark(kind) => self.set_marked(kind, !self.is_marked(kind)),
                    Setting::AutoLevel => self.level.set_volume(layout::auto_volume_at(x)),
                    Setting::DebugInfo => self.debug_info = !self.debug_info,
                    Setting::MixFile => self.mix_file = !self.mix_file,
                    Setting::ClockFromRemote => self.clock_from_remote = !self.clock_from_remote,
                    Setting::PauseWithRec => self.pause_with_rec = !self.pause_with_rec,
                    Setting::MuteWhileRecording => self.mute_while_recording = !self.mute_while_recording,
                    Setting::CellsOnWeakUsb => self.cells_on_weak_usb = !self.cells_on_weak_usb,
                    Setting::DrawOnInput => self.draw_on_input = !self.draw_on_input,
                    _ => {}
                }
            }
            Some(layout::SettingsHit::Minus(setting)) => {
                select(self, setting);
                self.change_setting(setting, -1);
            }
            Some(layout::SettingsHit::Plus(setting)) => {
                select(self, setting);
                self.change_setting(setting, 1);
            }
            Some(layout::SettingsHit::Done) => self.screen = Screen::Main,
            None => {}
        }
    }

    /// A finger moving after touching down at a point that was passed to
    /// [`touch`](Self::touch): on the waterfall, HET and COMB follow it.
    pub fn drag(&mut self, x: i32, y: i32) {
        if let Some((_, y0)) = self.gesture.down {
            self.gesture.at = (x, y);
            if matches!(self.screen, Screen::Files | Screen::Folders | Screen::FileInfo) {
                // A list row is chosen when the finger lifts, unless it moved: then it scrolls.
                if (y - y0).abs() > 8 {
                    self.gesture.moved = true;
                }
                if self.gesture.moved {
                    let row_h = if self.screen == Screen::FileInfo { layout::INFO_ROW_H } else { layout::LIST_ROW_H };
                    let rows = (y0 - y) as f32 / row_h as f32;
                    let top = self.gesture.top as i32 + libm::roundf(rows) as i32;
                    match self.screen {
                        Screen::Files => self.player.scroll_to(top, layout::LIST_ROWS),
                        Screen::Folders => self.player.scroll_folders_to(top, layout::LIST_ROWS),
                        _ => self.scroll_details_to(top),
                    }
                }
            }
            return;
        }
        if self.screen == Screen::Main
            && let Some(hz) = layout::waterfall_freq_at(x, y)
        {
            match self.mode {
                Mode::Het => self.set_het_lo(hz),
                // The LO grabbed at touch down stays under the finger. Choosing the nearest
                // LO again here made the comb jump once it stopped at either end of its range.
                Mode::Comb => self.set_comb_first(hz - self.comb_grip as f32 * DEFAULT_COMB_SPACING_HZ),
                _ => {}
            }
        }
    }

    /// Whether a touch down at (x, y) starts a drag that tunes.
    pub fn is_draggable(&self, x: i32, y: i32) -> bool {
        let p = embedded_graphics::prelude::Point::new(x, y);
        if matches!(self.screen, Screen::Files | Screen::Folders) {
            return layout::LIST.contains(p);
        }
        if self.screen == Screen::FileInfo {
            return layout::INFO_ROWS_AREA.contains(p);
        }
        if self.screen == Screen::Main && self.input_kind == InputKind::Playback && layout::SEEK.contains(p) {
            return true;
        }
        self.screen == Screen::Main
            && !self.cpu_test.running
            && matches!(self.mode, Mode::Het | Mode::Comb)
            && layout::waterfall_freq_at(x, y).is_some()
    }

    /// The finger has lifted after [`touch`](Self::touch). Choosing a list row and moving to a
    /// new place in the file happen here, so a finger that moves can scroll the list or slide
    /// along the seek bar first.
    pub fn release(&mut self) {
        let gesture = core::mem::replace(&mut self.gesture, Gesture::NONE);
        let Some((_, y0)) = gesture.down else { return };
        match self.screen {
            Screen::Files if !gesture.moved => {
                if let Some(row) = layout::list_row_at(y0)
                    && self.player.choose(self.player.list.top + row)
                {
                    self.screen = Screen::Main;
                }
            }
            Screen::Folders if !gesture.moved => {
                if let Some(row) = layout::list_row_at(y0) {
                    self.choose_folder(self.player.folder_list.top + row);
                }
            }
            Screen::Main if self.input_kind == InputKind::Playback => {
                self.player.seek(layout::seek_fraction(gesture.at.0));
            }
            _ => {}
        }
    }

    /// The finger that touched down has stayed down for [`LONG_PRESS_MS`] (the host times it).
    /// On a row of the list of recordings, if it has not moved, this opens that file's
    /// information, and the finger lifting then does nothing, so the row is not also chosen.
    pub fn hold(&mut self) {
        let Some((_, y0)) = self.gesture.down else { return };
        if self.screen != Screen::Files || self.gesture.moved {
            return;
        }
        if let Some(row) = layout::list_row_at(y0)
            && self.player.show_details(self.player.list.top + row)
        {
            self.gesture = Gesture::NONE;
            self.info_from = Screen::Files;
            self.screen = Screen::FileInfo;
        }
    }

    /// The host is reading the card again, which gives the files new ids: the file information
    /// view, if it is showing, closes, back to where it was opened from.
    pub fn close_file_info(&mut self) {
        if self.screen == Screen::FileInfo {
            self.screen = self.info_from;
        }
    }

    /// Whether the file information view has a Play button: opened from the list, for a file
    /// that can be played.
    pub(crate) fn details_play_shown(&self) -> bool {
        self.info_from == Screen::Files
            && self.player.details_file().is_some_and(|e| !matches!(e.info, player::FileInfo::NotPlayable(_)))
    }

    fn play_details_file(&mut self) {
        if self.player.choose_details_file() {
            self.screen = Screen::Main;
        }
    }

    /// Scroll the file information view so row `top` is first, keeping the rows shown filled.
    fn scroll_details_to(&mut self, top: i32) {
        let last = screen::details_row_count(self).saturating_sub(layout::INFO_ROWS) as i32;
        self.player.details_top = top.clamp(0, last.max(0)) as usize;
    }

    /// Where the seek bar's knob is drawn, as a fraction of the file: under the finger while it
    /// slides along the bar, otherwise at the time played.
    pub(crate) fn seek_shown(&self) -> Option<f32> {
        if self.screen == Screen::Main && self.gesture.down.is_some() && self.gesture.moved {
            return Some(layout::seek_fraction(self.gesture.at.0));
        }
        let audio = self.player.current_audio()?;
        Some(if audio.frames == 0 { 0.0 } else { self.player.position() as f32 / audio.frames as f32 })
    }

    /// BAT PLAYBACK's player: the list of recordings and what plays. The host lists the card and
    /// feeds the chosen file's audio; see [`player::Request`].
    pub fn player(&self) -> &player::Player<'a> {
        &self.player
    }

    pub fn player_mut(&mut self) -> &mut player::Player<'a> {
        &mut self.player
    }

    /// Show the list of recordings, asking the host to read the card the first time.
    pub fn open_files(&mut self) {
        if matches!(self.player.scan(), player::Scan::NotStarted | player::Scan::Failed) {
            self.player.rescan();
        }
        if let Some(i) = self.player.current().and_then(|c| self.player.files().iter().position(|e| e.id == c.id)) {
            self.player.list.highlight = i;
            self.player.move_highlight(0, layout::LIST_ROWS);
        }
        self.screen = Screen::Files;
    }

    /// Show the folders at the top of the card, asking the host to list them again.
    fn open_folders(&mut self) {
        self.player.list_folders();
        self.screen = Screen::Folders;
    }

    /// List the folder at folder choice row `row`, and show the list of recordings.
    fn choose_folder(&mut self, row: usize) {
        if self.player.choose_folder(row) {
            self.screen = Screen::Files;
        }
    }

    /// The play button and PLAY: with no file chosen, the list.
    fn play_button(&mut self) {
        if self.player.transport() == player::Transport::NoFile {
            self.open_files();
        } else {
            self.player.play_pause();
        }
    }

    fn tune_to(&mut self, hz: f32) {
        match self.mode {
            Mode::Het => self.set_het_lo(hz),
            Mode::Comb => {
                // Move the comb so its nearest LO lands on the frequency.
                let spacing = DEFAULT_COMB_SPACING_HZ;
                let i = libm::roundf((hz - self.comb_first_hz) / spacing).clamp(0.0, (COMB_COUNT - 1) as f32);
                self.comb_grip = i as u8;
                self.set_comb_first(hz - i * spacing);
            }
            _ => {}
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
        self.bufs.pitch_right.set_factor(PITCH_FACTORS[self.pitch_index]);
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

    /// Whether each ear gets its own block: HET, COMB, PITCH or 1:1, with Both chosen and two
    /// inputs on. With one input on, one chain runs, whatever is stored for Listen to.
    /// On the inputs both blocks carry that input. On the mix with Both chosen, the chain
    /// plays the average of the mix's two sides, so one mic panned to one side is heard in
    /// both ears. The app does not read the pans, so two inputs panned to one side stay in
    /// that ear.
    fn wants_stereo(&self) -> bool {
        (matches!(self.mode, Mode::Het | Mode::Comb | Mode::Pitch | Mode::Thru) || self.playback_slowing() > 1)
            && self.source() == MonitorSource::Both
            && self.inputs[1].is_some()
    }

    /// Whether the last chunk gave each ear its own chain.
    pub fn is_stereo(&self) -> bool {
        self.stereo
    }

    /// Process a block of the two inputs into the headphone output. In stereo (see
    /// [`is_stereo`](Self::is_stereo)) `first` is heard in `out_l` and `second` in
    /// `out_r`; otherwise both outputs carry the same sound. All four slices must have
    /// the same length; any length works.
    ///
    /// In BAT PLAYBACK with TE chosen, the host plays the file slowed by
    /// [`playback_slowing`](Self::playback_slowing), so the input is already slowed: it is heard
    /// as it is, and every that-many-th sample (the file at its real speed) is analysed.
    pub fn process(&mut self, first: &[f32], second: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        let n = first.len().min(second.len()).min(out_l.len()).min(out_r.len());
        if self.idle() {
            out_l[..n].fill(0.0);
            out_r[..n].fill(0.0);
            let seconds = n as f32 / self.sample_rate;
            for meter in &mut self.meters {
                meter.update(0.0, seconds);
            }
            self.idle_frames = self.idle_frames.saturating_add(n as u32);
            return;
        }
        if self.idle_frames > 0 {
            let frames = core::mem::take(&mut self.idle_frames);
            self.input_lost(frames);
        }
        let slowing = self.playback_slowing() as usize;
        let mut start = 0;
        while start < n {
            let end = (start + CHUNK).min(n);
            let (first, second) = (&first[start..end], &second[start..end]);
            let (out_l, out_r) = (&mut out_l[start..end], &mut out_r[start..end]);
            if slowing > 1 {
                self.process_slowed_chunk(slowing, first, second, out_l, out_r);
            } else {
                self.take_input(first, second);
                self.make_output(first, second, out_l, out_r);
                self.count_samples(first.len());
            }
            start = end;
        }
    }

    /// How many times slower the host plays the file: TE's factor in BAT PLAYBACK with TE chosen,
    /// otherwise 1. See [`process`](Self::process).
    pub fn playback_slowing(&self) -> u32 {
        if self.input_kind == InputKind::Playback && self.mode == Mode::Te {
            TE_FACTORS[self.te_index] as u32
        } else {
            1
        }
    }

    /// A chunk of the file slowed `slowing` times: analyse every `slowing`-th sample, and play
    /// the chunk as it is.
    fn process_slowed_chunk(
        &mut self,
        slowing: usize,
        first: &[f32],
        second: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) {
        let mut real = [[0.0f32; CHUNK / 4 + 1]; 2];
        let mut kept = 0;
        let mut i = self.slowed_phase;
        while i < first.len() && kept < real[0].len() {
            real[0][kept] = first[i];
            real[1][kept] = second[i];
            kept += 1;
            i += slowing;
        }
        self.slowed_phase = i.saturating_sub(first.len());
        if kept > 0 {
            self.take_input(&real[0][..kept], &real[1][..kept]);
            self.count_samples(kept);
        }
        self.mix_mono(first, second);
        self.make_output(first, second, out_l, out_r);
    }

    /// Skip the analysis of the chunks passed from now on, or go back to
    /// analysing them. For a host that has fallen behind: the headphone output
    /// is unchanged and costs far less without the analysis, so the host can
    /// catch up without a gap in the sound.
    ///
    /// While skipping, no calls are detected, so time expansion does not trigger
    /// by itself (MARK still triggers it), and the waterfall shows grey columns.
    /// A call in progress when skipping starts is reported when analysis restarts,
    /// at the length it had reached, or carries on if the sound is still there.
    /// Analysis restarts with a new window, and the detector keeps the background
    /// levels it had learned.
    pub fn set_skip_analysis(&mut self, skip: bool) {
        self.skip_analysis = skip;
    }

    /// Chunks of 256 samples (1.33 ms at 192 kHz) that the host has chosen not to have
    /// analysed since the start. The chime test's stop to the analysis does not count.
    pub fn skipped_chunks(&self) -> u32 {
        self.skipped_chunks
    }

    /// `frames` of input were lost before the next block passed to
    /// [`process`](Self::process), or with 0 the next block does not follow on
    /// from the last for some other reason, such as a pause. Times carry on as if
    /// the frames had been heard, and the analysis restarts, so the jump does not
    /// show as a click on the waterfall or a false call. After lost frames the
    /// sound passed to the listening modes also has no step at the join. With 0, the
    /// host's output is taken to start again from silence.
    pub fn input_lost(&mut self, frames: u32) {
        self.input_gap = true;
        self.lost_frames = self.lost_frames.saturating_add(frames);
        self.slowed_phase = 0;
        self.samples += frames as u64;
        if self.recording {
            self.recorded_samples += frames as u64;
        }
        if frames > 0 {
            self.join_left = JOIN_FADE_SAMPLES;
            self.chime.input_jumped(self.samples);
        } else {
            // The host paused, so the CPU test's row has a hole in it.
            if self.cpu_test.running {
                self.stop_cpu_test();
            }
            // If the number of chains changes next, its fade has to start from what is
            // heard, which is silence and not the last sample from before the pause.
            self.last_out = [0.0; 2];
            // The time since a chime started can no longer be counted, and the decoders'
            // input has a jump in it.
            if self.screen == Screen::ChimeTest {
                self.chime.restart(&mut self.bufs.chime, self.sample_rate as u32);
            }
        }
    }

    /// The meters, the decoders and the analysis, for a chunk of the inputs at their real speed.
    /// Leaves the chunk's mono mix in `mono`.
    fn take_input(&mut self, first: &[f32], second: &[f32]) {
        let len = first.len();
        let chunk_s = len as f32 / self.sample_rate;
        let peak = |x: &[f32]| x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        self.stages.start();
        self.meters[0].update(peak(first), chunk_s);
        self.meters[1].update(peak(second), chunk_s);
        self.stages.lap(Stage::Input);

        if self.screen == Screen::ChimeTest {
            let channels = self.chime_channels();
            self.chime.listen(&mut self.bufs.chime, [first, second], channels, self.samples);
        } else if self.remote.wants_decoders(self.samples) {
            let channels = self.chime_channels();
            self.remote.listen(&mut self.bufs.chime, [first, second], channels, self.samples);
        } else {
            self.remote.sleep_if_done(self.samples);
        }
        self.collect_phone_readings();
        self.stages.lap(Stage::Decoders);

        self.mix_mono(first, second);

        self.stages.lap(Stage::Input);
        self.detected = false;
        self.analyse(len);

        // Trigger time expansion on a marked detection. For a bat call that
        // is when it is confirmed, just after the call (the second of a
        // train), within the 50 ms the expander keeps from before its trigger.
        // A file playing slowed is expanded already.
        if self.detected && self.live_te() {
            self.trigger_te();
        }
        self.stages.lap(Stage::Analysis);
    }

    /// The chunk's mono mix of the inputs, into `mono`, as Listen to chooses.
    fn mix_mono(&mut self, first: &[f32], second: &[f32]) {
        let source = self.source();
        let mono = &mut self.mono[..first.len()];
        for ((m, &a), &b) in mono.iter_mut().zip(first).zip(second) {
            *m = match source {
                MonitorSource::First => a,
                MonitorSource::Second => b,
                MonitorSource::Both => 0.5 * (a + b),
            };
        }
    }

    /// Whether TE is chosen and its expander waits for a call: not while a file plays slowed.
    fn live_te(&self) -> bool {
        self.mode == Mode::Te && self.playback_slowing() == 1 && matches!(self.te.state(), ExpanderState::Idle)
    }

    fn count_samples(&mut self, len: usize) {
        self.samples += len as u64;
        if self.recording {
            self.recorded_samples += len as u64;
        }
    }

    /// The headphone output for a chunk of the inputs, whose mono mix is in `mono`.
    fn make_output(&mut self, first: &[f32], second: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        let len = first.len();
        let muted = self.muted_for_take();
        if muted && self.listen_gain == 0.0 {
            // Silent for the take: no listening mode, time expansion or Auto volume runs. A phone
            // remote beep is used up silently, so it does not play late when the take ends.
            if self.remote.beeping() {
                self.remote.add_beep(out_l, out_r);
            }
            out_l.fill(0.0);
            out_r.fill(0.0);
            self.modes_skipped = true;
            self.last_mono = self.mono[len - 1];
            self.last_ears = [first[len - 1], second[len - 1]];
            self.last_out = [0.0; 2];
            self.stages.lap(Stage::Input);
            self.stages.lap(Stage::Output);
            return;
        }
        if self.modes_skipped {
            // The take has ended: the modes start from silence, under the fade in.
            self.modes_skipped = false;
            self.reset_mode();
            self.level.sound_changed();
        }
        let analysis_runs = self.analysis_runs();
        let stereo = self.wants_stereo();
        if stereo && !self.stereo {
            // The second filter still holds whatever it held when stereo last stopped.
            match self.mode {
                Mode::Het => self.het.copy_filter_to_second_channel(),
                Mode::Comb => self.comb.copy_filter_to_second_channel(),
                // The left shifter carries on. The right one holds sound from when stereo
                // last stopped, so it starts from silence, under the switch fade.
                Mode::Pitch => self.bufs.pitch_right.reset(),
                _ => {}
            }
        }

        // After lost input, start the sound from where it stopped, so the join has
        // no step. The offset fades to nothing over JOIN_FADE_SAMPLES. This comes
        // before the listening modes and the time expansion recording, because a
        // step in the input would pass through their mixers and filters as a click
        // that no change to their output could remove. The analysis has already
        // restarted, so it does not need this.
        let mono = &mut self.mono[..len];
        if self.join_left == JOIN_FADE_SAMPLES {
            self.join_step = self.last_mono - mono[0];
            self.join_step_ears = [self.last_ears[0] - first[0], self.last_ears[1] - second[0]];
            let filter_heard_left = self.stereo && !stereo && matches!(self.mode, Mode::Het | Mode::Comb);
            self.join_step_filter = if filter_heard_left { self.last_ears[0] - self.last_mono } else { 0.0 };
        }
        let join_left = self.join_left;
        add_fading_step(mono, self.join_step, join_left);
        self.join_left = join_left.saturating_sub(len as u32);
        self.last_mono = mono[len - 1];
        self.stages.lap(Stage::Input);
        let mono = &self.mono[..len];
        let slowed = self.playback_slowing() > 1;
        if !slowed {
            self.te.process(mono, &mut self.te_out[..len]);
        }
        self.stages.lap(Stage::TeRecord);
        if stereo {
            // The two inputs go straight to the modes, except in a chunk with a join fade.
            let (left, right) = if join_left > 0 {
                let (left, right) = (&mut self.out[..len], &mut self.te_out[..len]);
                left.copy_from_slice(first);
                right.copy_from_slice(second);
                add_fading_step(left, self.join_step_ears[0], join_left);
                add_fading_step(right, self.join_step_ears[1], join_left);
                (&*left, &*right)
            } else {
                (first, second)
            };
            self.last_ears = [left[len - 1], right[len - 1]];
            match self.mode {
                Mode::Het => self.het.process_stereo(left, right, out_l, out_r),
                Mode::Comb => self.comb.process_stereo(left, right, out_l, out_r),
                Mode::Pitch => {
                    self.pitch.process(left, out_l);
                    self.bufs.pitch_right.process(right, out_r);
                }
                _ => {
                    out_l.copy_from_slice(left);
                    out_r.copy_from_slice(right);
                }
            }
            self.stages.lap(Stage::Mode);
            self.level.apply_stereo(out_l, out_r, self.mode, analysis_runs);
            self.stages.lap(Stage::Volume);
        } else {
            // The time expansion recording has had its copy, so this does not reach it.
            if join_left > 0 && self.join_step_filter != 0.0 {
                add_fading_step(&mut self.mono[..len], self.join_step_filter, join_left);
            }
            let mono = &self.mono[..len];
            let out = &mut self.out[..len];
            match self.mode {
                Mode::Het => self.het.process(mono, out),
                Mode::Comb => self.comb.process(mono, out),
                Mode::Fd => self.fd.process(mono, out),
                Mode::Te if !slowed => out.copy_from_slice(&self.te_out[..len]),
                Mode::Te => out.copy_from_slice(mono),
                Mode::Pitch => self.pitch.process(mono, out),
                Mode::Thru => out.copy_from_slice(mono),
            }
            self.stages.lap(Stage::Mode);
            self.level.apply(out, self.mode, analysis_runs);
            self.stages.lap(Stage::Volume);
            out_l.copy_from_slice(out);
            out_r.copy_from_slice(out);
            self.last_ears = [mono[len - 1]; 2];
        }

        // After a change between one chain and two, each ear starts from where it was.
        // This has to come after the modes, because the ears may have been hearing
        // different filters. Nothing was heard before the first chunk.
        if stereo != self.stereo {
            self.stereo = stereo;
            if self.samples > 0 {
                self.switch_step = [self.last_out[0] - out_l[0], self.last_out[1] - out_r[0]];
                self.switch_left = JOIN_FADE_SAMPLES;
            }
        }
        add_fading_step(out_l, self.switch_step[0], self.switch_left);
        add_fading_step(out_r, self.switch_step[1], self.switch_left);
        self.switch_left = self.switch_left.saturating_sub(len as u32);
        if self.remote.beeping() {
            self.remote.add_beep(out_l, out_r);
        }
        let target = if muted { 0.0 } else { 1.0 };
        if self.listen_gain != target || target != 1.0 {
            let step = if muted { -1.0 / MUTE_FADE_SAMPLES } else { 1.0 / MUTE_FADE_SAMPLES };
            let mut gain = self.listen_gain;
            for (l, r) in out_l.iter_mut().zip(out_r.iter_mut()) {
                gain = (gain + step).clamp(0.0, 1.0);
                *l *= gain;
                *r *= gain;
            }
            self.listen_gain = gain;
        }
        self.last_out = [out_l[len - 1], out_r[len - 1]];
        self.stages.lap(Stage::Output);
    }

    /// Feed the mono chunk to the spectrum analyzer, and each finished column
    /// to the detector, the waterfall and the snapshot history. When the chunk
    /// is skipped or input was lost, restart the analysis instead.
    fn analyse(&mut self, len: usize) {
        let Self {
            analyzer,
            detector,
            classifier,
            marked,
            detected,
            mono,
            samples,
            calls,
            last_call,
            last_call_since,
            bufs,
            wf_next,
            wf_column,
            bin_max,
            bin_row,
            bin_floor,
            row_floor,
            row_floor_set,
            display_offset_db,
            display_range_db,
            display_per_row,
            history_next,
            snapshot_hz,
            sample_rate,
            skip_analysis,
            screen,
            skipped_chunks,
            input_gap,
            lost_frames,
            level: auto_level,
            remote,
            ..
        } = self;
        let Buffers { wf, history, history_centre, snapshot, .. } = &mut **bufs;
        let column_len = ((*sample_rate * WF_COLUMN_MS * 0.001) as u64).max(1);
        let bin_hz = *sample_rate / FFT_N as f32;
        let scale = 255.0 / *display_range_db;

        // Only the kinds of detection chosen on the settings screen (bat calls
        // by default) are marked, shown and trigger time expansion; see
        // bat_dsp::classify for the kinds.
        let mut mark = |event: &CallEvent, history: &[u8; HISTORY * BINS], centres: &[u64; HISTORY]| {
            let verdict = classifier.classify(event);
            // In Bats only, a marked detection opens the headphones. So does a single
            // that is strong enough to make a train with any similar call after it,
            // which saves waiting for that call. A single has passed every other check,
            // including the lowest bat frequency. Weaker singles do not open them: see
            // the figures at `GATE_HOLD_S` in level.rs.
            let strong_single = verdict.kind == CallKind::Single
                && event.peak_db_above_floor >= DEFAULT_TRAIN_MIN_DB
                && *marked & kind_bit(CallKind::Bat) != 0;
            if strong_single || *marked & kind_bit(verdict.kind) != 0 {
                auto_level.call_detected(verdict.kind == CallKind::Bat);
            }
            if *marked & kind_bit(verdict.kind) != 0 {
                if let Some(first) = verdict.earlier {
                    calls.push((first.start_sample, CallKind::Bat));
                }
                calls.push((event.start_sample, verdict.kind));
                *last_call = Some((*event, verdict.kind));
                *last_call_since = classifier.tone_start().unwrap_or(event.start_sample);
                *snapshot_hz = build_snapshot(event, *sample_rate, history, centres, snapshot);
                *detected = true;
            }
        };

        // Move the waterfall on to display column `to`: write the column being
        // filled, then one column marked as not analysed for each display
        // column that had no spectrum at all, so times stay true on screen.
        // With `hold_floor`, the column being filled may hold a call, so the
        // background levels do not follow it.
        let mut advance = |to: u64, hold_floor: bool, bin_max: &mut [f32; BINS], bin_floor: &mut [f32; BINS]| {
            if to <= *wf_column {
                return;
            }
            let base = *wf_next * WF_H;
            let column = &mut wf[base..base + WF_H];
            if bin_max[0] > f32::MIN {
                // The loudest level per pixel over the display column. Each
                // pixel takes the loudest of its bins, so taking the pixels once
                // from the loudest level of each bin gives the same result as
                // taking them from every spectrum column.
                let mut pixels = [0.0f32; WF_H];
                let loudest = Column { centre_sample: 0, bin_hz, db: &bin_max[..] };
                loudest.to_pixels(VIEW_HI_HZ, VIEW_LO_HZ, &mut pixels);
                // Follow each row's background, only while no call is in
                // progress. Digital silence stops at MIN_FLOOR_DB.
                if !hold_floor {
                    let rate = if *row_floor_set { 0.02 } else { 1.0 };
                    for (f, &a) in row_floor.iter_mut().zip(&pixels) {
                        *f = (*f + rate * (a - *f)).max(MIN_FLOOR_DB);
                    }
                    *row_floor_set = true;
                }
                let floors = display_floors(row_floor, *display_per_row, *display_offset_db);
                for ((dst, &db), &floor) in column.iter_mut().zip(&pixels).zip(&floors) {
                    *dst = level(db, floor, scale).min(WF_NO_DATA - 1);
                }
                for (f, &row) in bin_floor.iter_mut().zip(bin_row.iter()) {
                    *f = floors[row as usize];
                }
            } else {
                column.fill(WF_NO_DATA);
            }
            *wf_next = (*wf_next + 1) % WF_W;
            // Past a whole screen of missed columns, more would change nothing.
            for _ in 1..(to - *wf_column).min(WF_W as u64) {
                let base = *wf_next * WF_H;
                wf[base..base + WF_H].fill(WF_NO_DATA);
                *wf_next = (*wf_next + 1) % WF_W;
            }
            *wf_column = to;
            bin_max.fill(f32::MIN);
        };

        let skip = *skip_analysis || *screen == Screen::ChimeTest;
        if skip || *input_gap {
            *input_gap = false;
            // The analysis stays stopped for as long as the chime test is open, so a call in
            // progress ends now, and not when the user goes back.
            let ended = if *screen == Screen::ChimeTest { detector.flush() } else { None };
            // No later column may span the jump.
            if let Some(event) = ended.or(detector.gap()) {
                mark(&event, history, history_centre);
            }
            let skipped = if skip { len as u64 } else { 0 };
            analyzer.skip(core::mem::take(lost_frames) as u64 + skipped);
        }
        if skip {
            if *skip_analysis {
                *skipped_chunks = skipped_chunks.wrapping_add(1);
            }
            // Keep the waterfall moving, so old columns do not stay at "now". A
            // column cut short by the skip may have held a call that has just
            // been ended, so it does not count as background.
            let newest = (*samples + len as u64).saturating_sub(FFT_N as u64 / 2) / column_len;
            advance(newest, true, bin_max, bin_floor);
            return;
        }

        // Also while the decoders run: the phone's second tone has to keep them running if its
        // first chime was not read.
        let looks_for_tone = remote.enabled();
        analyzer.push(&mono[..len], |col: &Column<'_>| {
            if looks_for_tone {
                remote.column(col.db);
            }
            if let Some(event) = detector.process(col) {
                mark(&event, history, history_centre);
            }
            advance(col.centre_sample / column_len, detector.is_active(), bin_max, bin_floor);
            for (m, &db) in bin_max.iter_mut().zip(col.db) {
                *m = m.max(db);
            }

            // Snapshot history: every bin, against the floor of the waterfall
            // row at that bin's frequency.
            let base = *history_next * BINS;
            for ((dst, &db), &floor) in history[base..base + BINS].iter_mut().zip(col.db).zip(bin_floor.iter()) {
                *dst = level(db, floor, scale);
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
        let floors = display_floors(&self.row_floor, per_row, offset_db);
        for (f, &row) in self.bin_floor.iter_mut().zip(&self.bin_row) {
            *f = floors[row as usize];
        }
    }

    /// Whether a tap at (x, y) would do something: the menu button, a mode
    /// button, the waterfall in HET or COMB mode (where a tap tunes), or a
    /// control on the settings screen.
    pub fn is_tappable(&self, x: i32, y: i32) -> bool {
        if self.cpu_test.running {
            return true;
        }
        let p = embedded_graphics::prelude::Point::new(x, y);
        match self.screen {
            Screen::ChimeTest => {
                return layout::CLOCK_BACK.contains(p) || (self.phone.is_some() && layout::CLOCK_SET.contains(p));
            }
            Screen::CpuTest => {
                return layout::CPU_TEST_BACK.contains(p)
                    || layout::CPU_TEST_START.contains(p)
                    || layout::CPU_TEST_CALLS.contains(p)
                    || (self.stages.running() && layout::CPU_TEST_VIEW.contains(p));
            }
            _ => {}
        }
        if self.screen == Screen::Settings {
            let pages = self.settings_pages().count();
            if (0..pages).any(|i| layout::tab(i, pages).contains(p)) {
                return true;
            }
            let page_button = match self.settings_page() {
                SettingsPage::Listen => false,
                SettingsPage::Record => {
                    self.guano_available && layout::panel_switch_row(layout::RECORD_GUANO).contains(p)
                }
                SettingsPage::Phone => {
                    (self.remote.available && layout::panel_switch_row(layout::PHONE_REMOTE).contains(p))
                        || layout::phone_time(self.remote.available).contains(p)
                }
                SettingsPage::Tests => self.debug_available && layout::TESTS_CPU_TEST.contains(p),
            };
            return page_button
                || match layout::settings_hit(x, y, self.page_settings()) {
                    Some(layout::SettingsHit::Minus(setting) | layout::SettingsHit::Plus(setting)) => {
                        setting != Setting::ListenTo || self.listen_has_choices()
                    }
                    hit => hit.is_some(),
                };
        }
        layout::MENU.contains(embedded_graphics::prelude::Point::new(x, y))
            || layout::mode_button_at(x, y, self.modes).is_some()
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

/// Add `step` to the start of `x`, fading to nothing, with `left` samples of a fade
/// over JOIN_FADE_SAMPLES still to come. Inlined so that it stays in ITCM on the X8.
#[inline(always)]
fn add_fading_step(x: &mut [f32], step: f32, mut left: u32) {
    for v in x.iter_mut().take(left as usize) {
        *v += step * left as f32 / JOIN_FADE_SAMPLES as f32;
        left -= 1;
    }
}

/// Map a level in dB to 0..=255 for the colour map. `scale` is 255 divided by
/// the range in dB.
#[inline]
fn level(db: f32, floor_db: f32, scale: f32) -> u8 {
    ((db - floor_db) * scale).clamp(0.0, 255.0) as u8
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
