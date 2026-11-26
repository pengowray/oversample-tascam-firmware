//! The CPU test: runs each listening mode with each Auto volume choice in turn on the main
//! screen, so the figures include drawing the waterfall, and keeps what the host measures for
//! each. The host passes its figures at every redraw with [`BatApp::cpu_test_figures`].
//!
//! Each row runs for `SETTLE_S` and then `MEASURE_S`. Only the second part counts. The app's
//! settings are put back when the test ends.
//!
//! [`BatApp::cpu_test_figures`]: crate::BatApp::cpu_test_figures

use core::f32::consts::{PI, TAU};

use crate::remote::RemoteResult;
use crate::stages::{Stage, StageClock};
use crate::{AutoVolume, Mode, MonitorSource, Recorder};

/// Seconds after a change of row that are left out, so the time expansion playback, the
/// filters and the host's one-second CPU figures have all moved to the new row.
pub(crate) const SETTLE_S: u64 = 2;
pub(crate) const MEASURE_S: u64 = 6;
/// The Auto volume choices the test runs. Bats only is left out: on B239 it measured within a
/// point of On in every row, both stopped and recording.
pub(crate) const VOLUMES: [AutoVolume; 2] = [AutoVolume::Off, AutoVolume::On];
/// HET, COMB, PITCH and 1:1 have two rows each, and FD and TE have one, for each of `VOLUMES`.
/// Two more rows run the chime decoders.
pub(crate) const MAX_ROWS: usize = 10 * VOLUMES.len() + 2;

/// A run of the test has up to two passes over the rows: the first in the recorder's state when
/// the test starts, then, when that state is record standby or stopped and the host can press
/// REC, a second while recording a take the test starts and stops itself.
pub(crate) const PASSES: usize = 2;
pub(crate) const FIRST_PASS: usize = 0;
pub(crate) const RECORDING_PASS: usize = 1;

/// Why a run ended before every row of every pass had run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum End {
    /// Stopped by a tap, HOME or a change of input.
    Stopped,
    /// The take for the second pass did not start.
    NoTake(RemoteResult),
}

/// The log of a run, which the host writes to the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Log {
    None,
    /// Ready for the host to take.
    Due,
    /// Taken; the host writes it as the next take's files close.
    Waiting,
    Saved,
}

/// What the run is doing between rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Rows,
    /// Waiting for the take the test asked for to start.
    StartingTake,
}

/// Frames in the chunks the app's analysis counts as skipped.
const SKIP_CHUNK_FRAMES: u64 = 256;

/// The host's figures at one moment. The counts keep rising for the life of the host, and the
/// test takes differences.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HostFigures {
    /// Shares of CPU, in percent, in the latest second the host has measured.
    pub irq_percent: u32,
    pub hook_percent: u32,
    pub dsp_percent: u32,
    pub draw_percent: u32,
    /// The part of `draw_percent` spent copying the finished frame to the display.
    pub copy_percent: u32,
    /// Changes each time the host publishes new shares.
    pub window: u32,
    pub dry: u32,
    pub lost: u32,
    pub dropped: u32,
    /// The most input frames waiting since the last call.
    pub waiting_most: u32,
}

/// What one row measured.
#[derive(Clone, Copy, Debug, PartialEq)]
/// Shares of CPU are bytes, which keeps two passes of results small.
pub struct RowResult {
    pub irq_percent: u8,
    pub hook_percent: u8,
    pub dsp_percent: u8,
    pub dsp_max: u8,
    pub draw_percent: u8,
    pub copy_percent: u8,
    /// Share of the CPU in each stage of the app's processing, in percent, when the host
    /// lends the app a clock (see [`Stage`]).
    pub stages: Option<[u8; Stage::COUNT]>,
    pub dry: u32,
    pub lost: u32,
    pub dropped: u32,
    /// Share of the row's time that the analysis was skipped, at most this much.
    pub skip_percent: u8,
    pub waiting_most: u32,
    /// The recorder's state, or `None` if it changed during the row.
    pub recorder: Option<Recorder>,
    /// Whether the take had a MIX file when the measuring started, or None with no take's files
    /// open.
    pub mix_file: Option<bool>,
}

/// One combination to run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Row {
    pub mode: Mode,
    pub volume: AutoVolume,
    /// Whether this mode can give each input its own ear here, so the row needs saying which.
    pub has_stereo: bool,
    /// With `has_stereo`, whether this row hears one input in both ears.
    pub one_side: bool,
    /// Whether the chime decoders run during this row.
    pub decoders: bool,
    pub result: Option<RowResult>,
}

impl Row {
    pub(crate) fn source(&self) -> MonitorSource {
        if self.one_side { MonitorSource::First } else { MonitorSource::Both }
    }
}

const EMPTY_ROW: Row =
    Row { mode: Mode::Het, volume: AutoVolume::Off, has_stereo: false, one_side: false, decoders: false, result: None };

/// The app's settings to put back after the test.
#[derive(Clone, Copy)]
pub(crate) struct Saved {
    pub(crate) mode: Mode,
    pub(crate) volume: AutoVolume,
    pub(crate) source: MonitorSource,
}

#[derive(Clone, Copy, Default)]
struct Sums {
    windows: u32,
    irq: u32,
    hook: u32,
    dsp: u32,
    draw: u32,
    copy: u32,
    dsp_max: u32,
    waiting_most: u32,
}

/// Which table the CPU test shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum View {
    /// The shares of CPU, and input lost.
    Totals,
    /// The app's processing by stage, and the drawing split from the copy to the display.
    Stages,
}

pub(crate) struct CpuTest {
    /// The rows of each pass, with that pass's results.
    pub(crate) passes: [[Row; MAX_ROWS]; PASSES],
    pub(crate) count: usize,
    /// The pass running, or the one the screen shows.
    pub(crate) pass: usize,
    /// Which passes this run has.
    pub(crate) runs: [bool; PASSES],
    pub(crate) phase: Phase,
    /// The test started the take the second pass records, so it stops it.
    pub(crate) owns_take: bool,
    pub(crate) end: Option<End>,
    pub(crate) log: Log,
    /// The row running, or the number of rows that finished.
    pub(crate) at: usize,
    pub(crate) running: bool,
    pub(crate) test_calls: bool,
    pub(crate) view: View,
    pub(crate) saved: Option<Saved>,
    row_start: u64,
    measuring: bool,
    /// Where the measuring started: the first figures after `SETTLE_S`, when the counts were
    /// taken.
    measure_start: u64,
    start_figures: HostFigures,
    start_skipped: u32,
    start_stages: [u64; Stage::COUNT],
    sums: Sums,
    last_window: u32,
    /// The state at the start of the measuring, and whether it has stayed the same.
    recorder: Option<Recorder>,
    mix_file: Option<bool>,
    calls: TestCalls,
}

impl CpuTest {
    pub(crate) const fn new() -> Self {
        Self {
            passes: [[EMPTY_ROW; MAX_ROWS]; PASSES],
            count: 0,
            pass: FIRST_PASS,
            runs: [true, false],
            phase: Phase::Rows,
            owns_take: false,
            end: None,
            log: Log::None,
            at: 0,
            running: false,
            test_calls: true,
            view: View::Totals,
            saved: None,
            row_start: 0,
            measuring: false,
            measure_start: 0,
            start_figures: HostFigures {
                irq_percent: 0,
                hook_percent: 0,
                dsp_percent: 0,
                draw_percent: 0,
                copy_percent: 0,
                window: 0,
                dry: 0,
                lost: 0,
                dropped: 0,
                waiting_most: 0,
            },
            start_skipped: 0,
            start_stages: [0; Stage::COUNT],
            sums: Sums { windows: 0, irq: 0, hook: 0, dsp: 0, draw: 0, copy: 0, dsp_max: 0, waiting_most: 0 },
            last_window: 0,
            recorder: None,
            mix_file: None,
            calls: TestCalls::new(),
        }
    }

    /// Lay out the rows for these modes, and PITCH even where it is not offered (the X8 hides
    /// it, and the test measures how far it is from keeping up): the Auto volume choices in
    /// order, then the chime decoders on the two modes where the CPU is shortest.
    pub(crate) fn plan(&mut self, modes: &[Mode], two_inputs: bool) {
        let mut count = 0;
        let rows = &mut self.passes[FIRST_PASS];
        let mut push = |row: Row| {
            rows[count] = row;
            count += 1;
        };
        let has_stereo = |mode: Mode| two_inputs && matches!(mode, Mode::Het | Mode::Comb | Mode::Pitch | Mode::Thru);
        let tested = |mode: &&Mode| modes.contains(mode) || **mode == Mode::Pitch;
        for volume in VOLUMES {
            for &mode in Mode::ALL.iter().filter(tested) {
                let row = Row { mode, volume, has_stereo: has_stereo(mode), ..EMPTY_ROW };
                push(row);
                if row.has_stereo {
                    push(Row { one_side: true, ..row });
                }
            }
        }
        // The decoders with the heaviest rows: on B239's unit run, FD and COMB stereo with Auto
        // volume on. The decoders added the same 26 or 27 dsp points to COMB stereo and TE there.
        for mode in [Mode::Fd, Mode::Comb] {
            if modes.contains(&mode) {
                push(Row { mode, volume: AutoVolume::On, has_stereo: has_stereo(mode), decoders: true, ..EMPTY_ROW });
            }
        }
        self.passes[RECORDING_PASS] = self.passes[FIRST_PASS];
        self.count = count;
        self.at = 0;
        self.pass = FIRST_PASS;
    }

    /// The rows of the pass running, or of the pass shown.
    pub(crate) fn rows(&self) -> &[Row] {
        &self.passes[self.pass][..self.count]
    }

    /// The rows of `pass`.
    pub(crate) fn pass_rows(&self, pass: usize) -> &[Row] {
        &self.passes[pass][..self.count]
    }

    pub(crate) fn has_results(&self) -> bool {
        self.passes.iter().any(|rows| rows[..self.count].iter().any(|row| row.result.is_some()))
    }

    /// Whether the second pass has results, so the screen offers both.
    pub(crate) fn both_passes(&self) -> bool {
        self.pass_rows(RECORDING_PASS).iter().any(|row| row.result.is_some())
    }

    /// The row running, while the test runs.
    pub(crate) fn current(&self) -> Option<&Row> {
        if self.running && self.phase == Phase::Rows { self.rows().get(self.at) } else { None }
    }

    /// Rows finished and rows in all, over every pass of this run.
    pub(crate) fn progress(&self) -> (usize, usize) {
        let passes = self.runs.iter().filter(|&&run| run).count();
        let before = if self.pass == RECORDING_PASS && self.runs[FIRST_PASS] { self.count } else { 0 };
        (before + self.at, passes * self.count)
    }

    pub(crate) fn begin_row(&mut self, now: u64) {
        self.row_start = now;
        self.measuring = false;
    }

    pub(crate) fn seconds_left_in_row(&self, now: u64, rate_hz: u64) -> u64 {
        let end = self.row_start + (SETTLE_S + MEASURE_S) * rate_hz;
        end.saturating_sub(now).div_ceil(rate_hz)
    }

    /// Take the host's figures. Returns true when the row has finished.
    pub(crate) fn figures(
        &mut self,
        figures: HostFigures,
        (recorder, mix_file): (Recorder, Option<bool>),
        skipped: u32,
        stages: &StageClock,
        now: u64,
        rate_hz: u64,
    ) -> bool {
        let elapsed = now.saturating_sub(self.row_start);
        if !self.measuring {
            if elapsed >= SETTLE_S * rate_hz {
                self.measuring = true;
                self.measure_start = now;
                self.start_figures = figures;
                self.start_skipped = skipped;
                self.start_stages = stages.totals;
                self.sums = Sums::default();
                self.last_window = figures.window;
                self.recorder = Some(recorder);
                self.mix_file = mix_file;
            }
            return false;
        }
        if self.recorder != Some(recorder) {
            self.recorder = None;
        }
        self.sums.waiting_most = self.sums.waiting_most.max(figures.waiting_most);
        if figures.window != self.last_window {
            self.last_window = figures.window;
            let sums = &mut self.sums;
            sums.windows += 1;
            sums.irq += figures.irq_percent;
            sums.hook += figures.hook_percent;
            sums.dsp += figures.dsp_percent;
            sums.draw += figures.draw_percent;
            sums.copy += figures.copy_percent;
            sums.dsp_max = sums.dsp_max.max(figures.dsp_percent);
        }
        if elapsed < (SETTLE_S + MEASURE_S) * rate_hz {
            return false;
        }
        let sums = self.sums;
        let average = |sum: u32| (sum + sums.windows / 2).checked_div(sums.windows).unwrap_or(0).min(100) as u8;
        let measured_frames = now.saturating_sub(self.measure_start).max(1);
        let skipped_frames = skipped.wrapping_sub(self.start_skipped) as u64 * SKIP_CHUNK_FRAMES;
        let from = self.start_figures;
        // Ticks in a stage, against the ticks in the measured time.
        let stage_shares = stages.running().then(|| {
            let ticks_measured = (measured_frames * stages.per_second as u64 / rate_hz).max(1);
            core::array::from_fn(|i| {
                let ticks = stages.totals[i].wrapping_sub(self.start_stages[i]);
                ((ticks * 100 + ticks_measured / 2) / ticks_measured).min(100) as u8
            })
        });
        let result = RowResult {
            irq_percent: average(sums.irq),
            hook_percent: average(sums.hook),
            dsp_percent: average(sums.dsp),
            dsp_max: sums.dsp_max.min(100) as u8,
            draw_percent: average(sums.draw),
            copy_percent: average(sums.copy),
            stages: stage_shares,
            dry: figures.dry.wrapping_sub(from.dry),
            lost: figures.lost.wrapping_sub(from.lost),
            dropped: figures.dropped.wrapping_sub(from.dropped),
            skip_percent: (skipped_frames * 100 / measured_frames).min(100) as u8,
            waiting_most: sums.waiting_most,
            recorder: self.recorder,
            mix_file: self.mix_file,
        };
        if let Some(row) = self.passes[self.pass][..self.count].get_mut(self.at) {
            row.result = Some(result);
        }
        true
    }

    /// Add the test calls to the two blocks.
    pub(crate) fn add_calls(&mut self, rate_hz: f32, first: &mut [f32], second: &mut [f32]) {
        self.calls.add(rate_hz, first, second);
    }
}

/// What the recorder was doing for every row.
pub(crate) fn every_row(state: Recorder) -> &'static str {
    match state {
        Recorder::Recording => "Recording for every row.",
        Recorder::Standby => "Record standby for every row.",
        Recorder::Paused => "Recording paused for every row.",
        Recorder::Stopped => "Recorder stopped for every row.",
    }
}

/// After the recorder's state while recording: whether the take writes a MIX file, which costs CPU.
pub(crate) fn mix_note(mix_file: Option<bool>) -> &'static str {
    match mix_file {
        Some(true) => " MIX file on.",
        Some(false) => " MIX file off.",
        None => "",
    }
}

/// Why the take for the second pass did not start.
pub(crate) fn no_take_reason(result: RemoteResult) -> &'static str {
    match result {
        RemoteResult::GuideOn => "the recording guide is on",
        RemoteResult::PageOpen => "a settings page is open in MANUAL",
        RemoteResult::NoResponse => "the recorder did not start",
        _ => "the take did not start",
    }
}

/// What the host tells the log about itself.
pub(crate) struct LogInfo {
    pub(crate) build: Option<u32>,
    pub(crate) rate_hz: u32,
    pub(crate) two_inputs: bool,
}

/// Room for a row's name in the log, with its marks.
const LOG_NAME: usize = 24;
/// The log's column headings, laid out for `LOG_WIDTHS`.
const LOG_COLUMNS: &str = concat!(
    "                         ------- % of CPU -------- ----------------- frames ------------------ ---------- % of CPU by stage ---------- -- draw --\n",
    "                          dsp  max draw  irq hook        dry     lost     drop     ring skip%    in dec ana  te mode vol out rest   frame copy\n",
);
/// Each column's width, with the space before it, in `LOG_COLUMNS` order.
const LOG_WIDTHS: [usize; 20] = [5, 5, 5, 5, 5, 11, 9, 9, 9, 6, 6, 4, 4, 4, 5, 4, 4, 5, 8, 5];

/// A row's figures in the log, every count in full. Without a stage clock the stage columns
/// show "-".
fn log_row(w: &mut dyn core::fmt::Write, r: &RowResult) -> core::fmt::Result {
    let totals = [
        r.dsp_percent.into(),
        r.dsp_max.into(),
        r.draw_percent.into(),
        r.irq_percent.into(),
        r.hook_percent.into(),
        r.dry,
        r.lost,
        r.dropped,
        r.waiting_most,
        r.skip_percent.into(),
    ];
    let mut stages = [None; 8];
    if let Some(shares) = r.stages {
        let sum: u32 = shares.iter().map(|&s| u32::from(s)).sum();
        for (out, &share) in stages.iter_mut().zip(&shares) {
            *out = Some(u32::from(share));
        }
        stages[7] = Some(u32::from(r.dsp_percent).saturating_sub(sum));
    }
    let draw = [Some(u32::from(r.draw_percent.saturating_sub(r.copy_percent))), Some(r.copy_percent.into())];
    let values = totals.into_iter().map(Some).chain(stages).chain(draw);
    for (value, width) in values.zip(LOG_WIDTHS) {
        // Right-aligned by hand: core::fmt's widths cost a few KB of code at each call.
        let mut digits = [0u8; 10];
        let mut at = digits.len();
        match value {
            Some(mut v) => loop {
                at -= 1;
                digits[at] = b'0' + (v % 10) as u8;
                v /= 10;
                if v == 0 {
                    break;
                }
            },
            None => {
                at -= 1;
                digits[at] = b'-';
            }
        }
        spaces(w, width.saturating_sub(digits.len() - at))?;
        w.write_str(core::str::from_utf8(&digits[at..]).unwrap_or(""))?;
    }
    Ok(())
}

fn spaces(w: &mut dyn core::fmt::Write, n: usize) -> core::fmt::Result {
    (0..n).try_for_each(|_| w.write_str(" "))
}

impl CpuTest {
    /// The run as plain text, for `CPU_TEST.TXT`: every pass with both tables side by side, and
    /// counts in full. Integers only: formatting a float would pull in about 19 KB of code.
    pub(crate) fn write_log(&self, w: &mut dyn core::fmt::Write, info: &LogInfo) -> core::fmt::Result {
        write!(w, "CPU test")?;
        if let Some(build) = info.build {
            write!(w, ", firmware B{build}")?;
        }
        let inputs = if info.two_inputs { "2 inputs" } else { "1 input" };
        let calls = if self.test_calls { "on" } else { "off" };
        writeln!(w, ", {} Hz, {inputs}, synthetic calls {calls}", info.rate_hz)?;
        writeln!(w, "Each row: {SETTLE_S} s to settle, then {MEASURE_S} s measured.")?;
        writeln!(w, "dsp, draw and the stages are elapsed time, so they include time spent in interrupts.")?;
        let (done, all) = self.progress_done();
        match self.end {
            Some(End::Stopped) => writeln!(w, "Stopped after {done} of {all} rows.")?,
            Some(End::NoTake(result)) => writeln!(w, "No recording pass: {}.", no_take_reason(result))?,
            None => {}
        }
        for pass in 0..PASSES {
            let rows = self.pass_rows(pass);
            let Some(first) = rows.iter().find_map(|row| row.result) else { continue };
            writeln!(w)?;
            match first.recorder {
                Some(state) => writeln!(w, "{}{}", every_row(state), mix_note(first.mix_file))?,
                None => writeln!(w, "Recorder state changed during row 1.")?,
            }
            w.write_str(LOG_COLUMNS)?;
            let mut heading = "";
            for row in rows {
                let group = group_heading(row);
                if group != heading {
                    heading = group;
                    w.write_str(group)?;
                    w.write_str("\n")?;
                }
                let star = match row.result {
                    Some(result) if (result.recorder, result.mix_file) != (first.recorder, first.mix_file) => " *",
                    _ => "",
                };
                let (mode, suffix) = row_name(row);
                for part in ["  ", mode, suffix, star] {
                    w.write_str(part)?;
                }
                spaces(w, LOG_NAME.saturating_sub(2 + mode.len() + suffix.len() + star.len()))?;
                match row.result {
                    Some(r) => log_row(w, &r)?,
                    None => w.write_str(" not run")?,
                }
                w.write_str("\n")?;
            }
            if rows.iter().any(|row| row.result.is_some_and(|r| (r.recorder, r.mix_file) != (first.recorder, first.mix_file))) {
                writeln!(w, "* The recorder's state differed from the pass's first row.")?;
            }
        }
        writeln!(w)
    }

    /// Rows finished and rows planned, over the passes this run has.
    fn progress_done(&self) -> (usize, usize) {
        let done = (0..PASSES).map(|p| self.pass_rows(p).iter().filter(|row| row.result.is_some()).count()).sum();
        (done, self.runs.iter().filter(|&&run| run).count() * self.count)
    }
}

/// The heading over a group of rows: Auto volume, and whether the chime decoders run.
pub(crate) fn group_heading(row: &Row) -> &'static str {
    match (row.decoders, row.volume) {
        (true, AutoVolume::Off) => "Auto volume off, chime decoders running",
        (true, AutoVolume::On) => "Auto volume on, chime decoders running",
        (true, AutoVolume::BatsOnly) => "Auto volume Bats only, chime decoders running",
        (false, AutoVolume::Off) => "Auto volume off",
        (false, AutoVolume::On) => "Auto volume on",
        (false, AutoVolume::BatsOnly) => "Auto volume Bats only",
    }
}

/// A row's name: the mode, then stereo or mono where the mode has both.
pub(crate) fn row_name(row: &Row) -> (&'static str, &'static str) {
    let suffix = match (row.has_stereo, row.one_side) {
        (false, _) => "",
        (true, false) => " stereo",
        (true, true) => " mono",
    };
    (row.mode.label(), suffix)
}

/// Synthetic bat calls, so that every run of the test hears the same sound and time expansion
/// keeps playing: 5 ms sweeps from 60 to 40 kHz at -40 dBFS, ten a second. These figures were
/// chosen to be classed as bat calls, which `tests/cpu_test.rs` checks.
struct TestCalls {
    /// Frames since the start of the latest call.
    at: u32,
    phase: f32,
}

const CALL_PERIOD_S: f32 = 0.1;
const CALL_LENGTH_S: f32 = 0.005;
const CALL_FROM_HZ: f32 = 60_000.0;
const CALL_TO_HZ: f32 = 40_000.0;
const CALL_AMPLITUDE: f32 = 0.01;

impl TestCalls {
    const fn new() -> Self {
        Self { at: 0, phase: 0.0 }
    }

    /// Not inlined, and named so that it stays out of ITCM: it runs only during the test.
    #[inline(never)]
    fn add(&mut self, rate_hz: f32, first: &mut [f32], second: &mut [f32]) {
        let period = (CALL_PERIOD_S * rate_hz) as u32;
        let length = (CALL_LENGTH_S * rate_hz) as u32;
        for (a, b) in first.iter_mut().zip(second.iter_mut()) {
            if self.at < length {
                let t = self.at as f32 / length as f32;
                let hz = CALL_FROM_HZ + (CALL_TO_HZ - CALL_FROM_HZ) * t;
                self.phase = (self.phase + TAU * hz / rate_hz) % TAU;
                let rise = libm::sinf(PI * t);
                let x = CALL_AMPLITUDE * rise * rise * libm::sinf(self.phase);
                *a += x;
                *b += x;
            }
            self.at += 1;
            if self.at >= period {
                self.at = 0;
            }
        }
    }
}
