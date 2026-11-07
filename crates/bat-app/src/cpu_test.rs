//! The CPU test: runs each listening mode with each Auto volume choice in turn on the main
//! screen, so the figures include drawing the waterfall, and keeps what the host measures for
//! each. The host passes its figures at every redraw with [`BatApp::cpu_test_figures`].
//!
//! Each row runs for `SETTLE_S` and then `MEASURE_S`. Only the second part counts. The app's
//! settings are put back when the test ends.
//!
//! [`BatApp::cpu_test_figures`]: crate::BatApp::cpu_test_figures

use core::f32::consts::{PI, TAU};

use crate::stages::{Stage, StageClock};
use crate::{AutoVolume, Mode, MonitorSource, Recorder};

/// Seconds after a change of row that are left out, so the time expansion playback, the
/// filters and the host's one-second CPU figures have all moved to the new row.
pub(crate) const SETTLE_S: u64 = 2;
pub(crate) const MEASURE_S: u64 = 6;
/// HET, COMB and 1:1 have two rows each, and FD, TE and PITCH have one, for each of the three
/// Auto volume choices. Two more rows run the chime decoders.
pub(crate) const MAX_ROWS: usize = 9 * 3 + 2;

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
pub struct RowResult {
    pub irq_percent: u32,
    pub hook_percent: u32,
    pub dsp_percent: u32,
    pub dsp_max: u32,
    pub draw_percent: u32,
    pub copy_percent: u32,
    /// Share of the CPU in each stage of the app's processing, in percent, when the host
    /// lends the app a clock (see [`Stage`]).
    pub stages: Option<[u32; Stage::COUNT]>,
    pub dry: u32,
    pub lost: u32,
    pub dropped: u32,
    /// Share of the row's time that the analysis was skipped, at most this much.
    pub skip_percent: u32,
    pub waiting_most: u32,
    /// The recorder's state, or `None` if it changed during the row.
    pub recorder: Option<Recorder>,
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
    pub(crate) rows: [Row; MAX_ROWS],
    pub(crate) count: usize,
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
    calls: TestCalls,
}

impl CpuTest {
    pub(crate) const fn new() -> Self {
        Self {
            rows: [EMPTY_ROW; MAX_ROWS],
            count: 0,
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
            calls: TestCalls::new(),
        }
    }

    /// Lay out the rows for these modes: the Auto volume choices in order, then the chime
    /// decoders on the two modes where the CPU is shortest.
    pub(crate) fn plan(&mut self, modes: &[Mode], two_inputs: bool) {
        let mut count = 0;
        let mut push = |row: Row| {
            self.rows[count] = row;
            count += 1;
        };
        let has_stereo = |mode: Mode| two_inputs && matches!(mode, Mode::Het | Mode::Comb | Mode::Thru);
        for volume in AutoVolume::ALL {
            for &mode in modes {
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
        self.count = count;
        self.at = 0;
    }

    pub(crate) fn has_results(&self) -> bool {
        self.rows[..self.count].iter().any(|row| row.result.is_some())
    }

    /// The row running, while the test runs.
    pub(crate) fn current(&self) -> Option<&Row> {
        if self.running { self.rows[..self.count].get(self.at) } else { None }
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
        recorder: Recorder,
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
        let average = |sum: u32| if sums.windows == 0 { 0 } else { (sum + sums.windows / 2) / sums.windows };
        let measured_frames = now.saturating_sub(self.measure_start).max(1);
        let skipped_frames = skipped.wrapping_sub(self.start_skipped) as u64 * SKIP_CHUNK_FRAMES;
        let from = self.start_figures;
        // Ticks in a stage, against the ticks in the measured time.
        let stage_shares = stages.running().then(|| {
            let ticks_measured = (measured_frames * stages.per_second as u64 / rate_hz).max(1);
            core::array::from_fn(|i| {
                let ticks = stages.totals[i].wrapping_sub(self.start_stages[i]);
                ((ticks * 100 + ticks_measured / 2) / ticks_measured).min(100) as u32
            })
        });
        let result = RowResult {
            irq_percent: average(sums.irq),
            hook_percent: average(sums.hook),
            dsp_percent: average(sums.dsp),
            dsp_max: sums.dsp_max,
            draw_percent: average(sums.draw),
            copy_percent: average(sums.copy),
            stages: stage_shares,
            dry: figures.dry.wrapping_sub(from.dry),
            lost: figures.lost.wrapping_sub(from.lost),
            dropped: figures.dropped.wrapping_sub(from.dropped),
            skip_percent: (skipped_frames * 100 / measured_frames).min(100) as u32,
            waiting_most: sums.waiting_most,
            recorder: self.recorder,
        };
        if let Some(row) = self.rows.get_mut(self.at) {
            row.result = Some(result);
        }
        true
    }

    /// Add the test calls to the two blocks.
    pub(crate) fn add_calls(&mut self, rate_hz: f32, first: &mut [f32], second: &mut [f32]) {
        self.calls.add(rate_hz, first, second);
    }
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
