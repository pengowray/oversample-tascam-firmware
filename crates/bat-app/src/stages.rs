//! Where the app's processing time goes, by stage, for the CPU test's Stages table.
//!
//! The host lends the app a counter that its own clock moves on: on the X8, the audio blocks,
//! 48,000 a second, which the audio interrupt counts. The app adds up the counter's steps
//! during each stage. One stage of one chunk takes a few microseconds, less than one step, but
//! the steps fall at random points in the stages, so over the thousands of chunks in a CPU test
//! row the sums come out right. Like `dsp`, the sums include the time the interrupts take
//! while a stage runs.

use core::sync::atomic::{AtomicU32, Ordering::Relaxed};

/// The parts of [`BatApp::process`](crate::BatApp::process), in the order they run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// The meters, the mix of the two inputs and the fade after lost input.
    Input,
    /// The chime decoders, for the phone remote or the chime test.
    Decoders,
    /// The spectrum, the detector, the waterfall and the snapshots.
    Analysis,
    /// Time expansion's recording, which runs in every mode, and its playback.
    TeRecord,
    /// The listening mode.
    Mode,
    /// Auto volume and its gate.
    Volume,
    /// The fades after a change between one chain and two, the remote's beeps and the copies to
    /// the output.
    Output,
}

impl Stage {
    pub const COUNT: usize = 7;
}

pub(crate) struct StageClock {
    ticks: Option<&'static AtomicU32>,
    pub(crate) per_second: u32,
    /// Steps of the counter in each stage, since the clock was set.
    pub(crate) totals: [u64; Stage::COUNT],
    mark: u32,
}

impl StageClock {
    pub(crate) const fn new() -> Self {
        Self { ticks: None, per_second: 0, totals: [0; Stage::COUNT], mark: 0 }
    }

    pub(crate) fn set(&mut self, ticks: &'static AtomicU32, per_second: u32) {
        self.ticks = Some(ticks);
        self.per_second = per_second;
    }

    pub(crate) fn running(&self) -> bool {
        self.ticks.is_some() && self.per_second > 0
    }

    #[inline(always)]
    pub(crate) fn start(&mut self) {
        if let Some(ticks) = self.ticks {
            self.mark = ticks.load(Relaxed);
        }
    }

    /// The time since the last `start` or `lap` was spent in `stage`.
    #[inline(always)]
    pub(crate) fn lap(&mut self, stage: Stage) {
        if let Some(ticks) = self.ticks {
            let now = ticks.load(Relaxed);
            self.totals[stage as usize] += now.wrapping_sub(self.mark) as u64;
            self.mark = now;
        }
    }
}
