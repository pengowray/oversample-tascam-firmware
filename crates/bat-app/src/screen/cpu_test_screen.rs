//! The CPU test's results, and what the main screen shows while it runs.

use embedded_graphics::pixelcolor::Rgb565;

use super::{
    BODY, BODY_BOLD, CENTER, Fb, LEFT, RIGHT, SMALL, hline, no_input_notice, rate_warning, rounded, switch_control, text,
};
use crate::cpu_test::{End, Log, MEASURE_S, Phase, RowResult, SETTLE_S, View, no_take_reason, every_row, group_heading, mix_note, row_name};
use crate::layout::*;
use crate::palette::*;
use crate::{AutoVolume, BatApp, Recorder};

pub(super) const TITLE: &str = "CPU TEST";

/// The right edge of each column of figures, in table order. Each column is as wide as its
/// heading or its widest value, whichever is wider, with `COLUMN_GAP` between columns. On B239's
/// photo `max` ran into `draw`, and `drop`'s 0 into `ring`'s 2644. A test measures the widths.
const DSP_X: i32 = 98;
const MAX_X: i32 = 120;
const DRAW_X: i32 = 146;
/// The spare pixels go between columns whose values have two or more digits.
const IRQ_X: i32 = 162;
const HOOK_X: i32 = 188;
const DRY_X: i32 = 209;
const LOST_X: i32 = 230;
const DROP_X: i32 = 255;
const RING_X: i32 = 284;
const SKIP_X: i32 = 314;
#[cfg(test)]
pub(crate) const COLUMN_GAP: i32 = 3;

const LABEL_X: i32 = 6;
const ROW_INDENT: i32 = 4;

pub(super) const CALLS_LINES: [&str; 2] =
    ["Synthetic calls make every run sound the same.", "10 a second. Heard in headphones. Not recorded."];
pub(super) const START: &str = "Start test";
pub(super) const START_AGAIN: &str = "Test again";
pub(super) const COLUMNS: [(&str, i32); 10] = [
    ("dsp", DSP_X),
    ("max", MAX_X),
    ("draw", DRAW_X),
    ("irq", IRQ_X),
    ("hook", HOOK_X),
    ("dry", DRY_X),
    ("lost", LOST_X),
    ("drop", DROP_X),
    ("ring", RING_X),
    ("skip%", SKIP_X),
];

/// The Stages table: `dsp` again, the app's stages in the order they run (see `Stage`), the rest
/// of the DSP task's time, and `draw` split into drawing the frame and copying it to the
/// display. Laid out as the Totals table is, with more room after `dsp` and before `frame`.
const FRAME_X: i32 = 288;
const FRAME_LEFT: i32 = FRAME_X - 26;
pub(super) const STAGE_COLUMNS: [(&str, i32); 11] = [
    ("dsp", DSP_X),
    ("in", 116),
    ("dec", 135),
    ("ana", 154),
    ("te", 169),
    ("mode", 197),
    ("vol", 214),
    ("out", 232),
    ("rest", 253),
    ("frame", FRAME_X),
    ("copy", SKIP_X),
];
pub(super) const VIEW_STAGES: &str = "Stages";
pub(super) const VIEW_TOTALS: &str = "Totals";

/// The line above the table when the recorder's state changed during the test.
const STATE_CHANGED: &str = "Recorder state changed. * differs from row 1.";

/// Whether the mode's own label and suffix fit before the first column of figures.
#[cfg(test)]
pub(crate) const NAME_ROOM: i32 = DSP_X - 17 - LABEL_X - ROW_INDENT;

/// A count in full below `full_below`, then in thousands, then in millions. `ring` has room for 4
/// digits, and the counts that should be 0 for 3 characters: `999`, `99k`, `.5M` or `3M`. Those
/// reach a million frames only when a row loses most of its input.
struct Count {
    n: u32,
    full_below: u32,
}

impl core::fmt::Display for Count {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.n {
            n if n < self.full_below => write!(f, "{n}"),
            n @ 0..100_000 => write!(f, "{}k", n / 1_000),
            n @ 100_000..1_000_000 => write!(f, ".{}M", n / 100_000),
            n => write!(f, "{}M", n / 1_000_000),
        }
    }
}

const RING_FULL_BELOW: u32 = 10_000;
const PROBLEM_FULL_BELOW: u32 = 1_000;

/// A count of something that should be 0: yellow when it is not, so a row with a problem shows.
fn problem_count(fb: Fb, value: u32, x: i32, y: i32) {
    let colour: Rgb565 = if value == 0 { TEXT_MUTED } else { METER_YELLOW };
    text(fb, &SMALL, format_args!("{}", Count { n: value, full_below: PROBLEM_FULL_BELOW }), x, y, RIGHT, colour);
}

/// The recorder's state for the whole test, or the first row's state if it changed.
fn summary(app: &BatApp, fb: Fb) {
    let rows = app.cpu_test_rows();
    let ran = rows.iter().filter(|row| row.result.is_some()).count();
    let y = CPU_TEST_SUMMARY;
    let Some(first) = rows.iter().find_map(|row| row.result) else {
        let state = app.recorder().0;
        let line = match state {
            Recorder::Recording if app.take_mix_file() == Some(true) => {
                "Recording, MIX file on. The test runs while recording."
            }
            Recorder::Recording if app.take_mix_file() == Some(false) => {
                "Recording, MIX file off. The test runs while recording."
            }
            Recorder::Recording => "Recording. The test runs while recording.",
            Recorder::Standby | Recorder::Stopped if app.cpu_test_would_record() => match state {
                Recorder::Standby => "Tests in record standby, then while recording a test take.",
                _ => "Tests with the recorder stopped, then while recording a test take.",
            },
            Recorder::Standby => "Record standby. Press REC to test while recording.",
            Recorder::Paused => "Recording paused. Press REC to test while recording.",
            Recorder::Stopped => "Recorder stopped. Press REC to test while recording.",
        };
        text(fb, &SMALL, line, LABEL_X, y, LEFT, TEXT);
        return;
    };
    let same = rows
        .iter()
        .filter_map(|row| row.result)
        .all(|result| result.recorder == first.recorder && result.mix_file == first.mix_file);
    let x = match (same, first.recorder) {
        (true, Some(state)) => {
            let x = text(fb, &SMALL, every_row(state), LABEL_X, y, LEFT, TEXT);
            text(fb, &SMALL, mix_note(first.mix_file), x, y, LEFT, TEXT)
        }
        _ => text(fb, &SMALL, STATE_CHANGED, LABEL_X, y, LEFT, METER_YELLOW),
    };
    if ran < rows.len() {
        text(fb, &SMALL, format_args!(" Ended at {ran} of {}.", rows.len()), x, y, LEFT, METER_YELLOW);
    }
}

/// Above the row names, with the build number, so a photo of the results says which build
/// measured them.
const BUILD_PREFIX: &str = "Firmware B";

/// Where "% of CPU" starts.
const GROUPS_X: i32 = DSP_X - 16;

fn table(app: &BatApp, fb: Fb) {
    if let Some(build) = app.host_build() {
        text(fb, &SMALL, format_args!("{BUILD_PREFIX}{build}"), LABEL_X, CPU_TEST_GROUPS, LEFT, TEXT);
    }
    text(fb, &SMALL, "% of CPU", GROUPS_X, CPU_TEST_GROUPS, LEFT, TEXT_MUTED);
    let view = if app.stages_available() { app.bufs.cpu_test.view } else { View::Totals };
    let columns: &[(&str, i32)] = match view {
        View::Totals => {
            text(fb, &SMALL, "frames", LOST_X - 18, CPU_TEST_GROUPS, LEFT, TEXT_MUTED);
            &COLUMNS
        }
        View::Stages => {
            text(fb, &SMALL, "draw", FRAME_LEFT, CPU_TEST_GROUPS, LEFT, TEXT_MUTED);
            &STAGE_COLUMNS
        }
    };
    for &(name, x) in columns {
        text(fb, &SMALL, name, x, CPU_TEST_COLUMNS, RIGHT, TEXT_MUTED);
    }
    hline(fb, LABEL_X, SKIP_X, CPU_TEST_COLUMNS + 2, LINE);
    let rows = app.cpu_test_rows();
    let first = rows.iter().find_map(|row| row.result).map(|result| (result.recorder, result.mix_file));
    let mut y = CPU_TEST_FIRST_ROW;
    let mut heading = "";
    for row in rows {
        let group = group_heading(row);
        if group != heading {
            heading = group;
            text(fb, &SMALL, group, LABEL_X, y, LEFT, TEXT_MUTED);
            y += CPU_TEST_ROW_H;
        }
        let (mode, suffix) = row_name(row);
        let x = text(fb, &SMALL, mode, LABEL_X + ROW_INDENT, y, LEFT, TEXT);
        let x = text(fb, &SMALL, suffix, x, y, LEFT, TEXT);
        if let Some(result) = row.result {
            if Some((result.recorder, result.mix_file)) != first {
                text(fb, &SMALL, "*", x + 1, y, LEFT, METER_YELLOW);
            }
            match view {
                View::Totals => totals_row(fb, &result, y),
                View::Stages => stages_row(fb, &result, y),
            }
        }
        y += CPU_TEST_ROW_H;
    }
}

fn totals_row(fb: Fb, result: &RowResult, y: i32) {
    for (value, column) in [
        (result.dsp_percent, DSP_X),
        (result.dsp_max, MAX_X),
        (result.draw_percent, DRAW_X),
        (result.irq_percent, IRQ_X),
        (result.hook_percent, HOOK_X),
    ] {
        text(fb, &SMALL, format_args!("{value}"), column, y, RIGHT, TEXT);
    }
    problem_count(fb, result.dry, DRY_X, y);
    problem_count(fb, result.lost, LOST_X, y);
    problem_count(fb, result.dropped, DROP_X, y);
    let ring = Count { n: result.waiting_most, full_below: RING_FULL_BELOW };
    text(fb, &SMALL, format_args!("{ring}"), RING_X, y, RIGHT, TEXT_MUTED);
    problem_count(fb, result.skip_percent.into(), SKIP_X, y);
}

/// A row of the Stages table. `rest` is the DSP task's time outside the app's processing:
/// reading the input ring, queueing the output and the phone remote's buttons.
fn stages_row(fb: Fb, result: &RowResult, y: i32) {
    let Some(stages) = result.stages else { return };
    let rest = u32::from(result.dsp_percent).saturating_sub(stages.iter().map(|&s| u32::from(s)).sum());
    let frame = result.draw_percent.saturating_sub(result.copy_percent);
    let values = [result.dsp_percent.into()]
        .into_iter()
        .chain(stages.iter().map(|&s| u32::from(s)))
        .chain([rest, frame.into(), result.copy_percent.into()]);
    for (value, &(_, x)) in values.zip(STAGE_COLUMNS.iter()) {
        text(fb, &SMALL, format_args!("{value}"), x, y, RIGHT, TEXT);
    }
}

/// A tab's name for a pass: the recorder's state in its first row.
fn pass_name(state: Option<Recorder>) -> &'static str {
    match state {
        Some(Recorder::Recording) => "Recording",
        Some(Recorder::Standby) => "Standby",
        Some(Recorder::Paused) => "Paused",
        Some(Recorder::Stopped) => "Stopped",
        None => "Mixed",
    }
}

pub(super) const LOG_FILE: &str = "CPU_TEST.TXT";
pub(super) const LOG_SAVED: &str = "saved on the card";
pub(super) const LOG_WAITING: &str = "saved when a take ends";
pub(super) const LOG_WAITING_THIS: &str = "saved when this take ends";
pub(super) const NO_RECORDING_PASS: &str = "No recording pass: ";

/// Under the table: the tabs for the two passes, after a run that had both, or why the
/// recording pass did not run; then the log and whether it is on the card yet.
fn passes_and_log(app: &BatApp, fb: Fb) {
    let test = &app.bufs.cpu_test;
    if test.running {
        return;
    }
    if test.both_passes() {
        for (pass, tab) in CPU_TEST_PASS_TABS.iter().enumerate() {
            let state = test.pass_rows(pass).iter().find_map(|row| row.result).and_then(|result| result.recorder);
            // As the settings page's tabs: the pass shown has the accent fill.
            rounded(fb, *tab, 4, if pass == test.pass { ACCENT } else { BUTTON });
            text(fb, &BODY, pass_name(state), tab.center().x, tab.top_left.y + 19, CENTER, TEXT);
        }
    } else if let Some(End::NoTake(result)) = test.end {
        let x = text(fb, &SMALL, NO_RECORDING_PASS, LABEL_X, CPU_TEST_LOG_Y + 5, LEFT, METER_YELLOW);
        text(fb, &SMALL, format_args!("{}.", no_take_reason(result)), x, CPU_TEST_LOG_Y + 5, LEFT, METER_YELLOW);
        return;
    }
    let state = match test.log {
        Log::Waiting | Log::Due if app.is_recording() => LOG_WAITING_THIS,
        Log::Waiting | Log::Due => LOG_WAITING,
        Log::Saved => LOG_SAVED,
        Log::None if !test.has_results() => {
            // Before the first run: how long it takes, and the take it records.
            let minutes = (test.count as u64 * (SETTLE_S + MEASURE_S)).div_ceil(60);
            if app.cpu_test_would_record() {
                let line = format_args!("Takes about {} min, including a {minutes} min test take.", 2 * minutes);
                text(fb, &SMALL, line, LABEL_X, CPU_TEST_LOG_Y + 5, LEFT, TEXT_MUTED);
            } else {
                text(fb, &SMALL, format_args!("Takes about {minutes} min."), LABEL_X, CPU_TEST_LOG_Y + 5, LEFT, TEXT_MUTED);
            }
            return;
        }
        Log::None => return,
    };
    // Beside the tabs, or at the left with no tabs.
    let x = if test.both_passes() { CPU_TEST_LOG_X } else { LABEL_X };
    text(fb, &SMALL, LOG_FILE, x, CPU_TEST_LOG_Y, LEFT, TEXT);
    text(fb, &SMALL, state, x, CPU_TEST_LOG_Y + 12, LEFT, TEXT_MUTED);
}

pub(super) fn draw(app: &BatApp, fb: Fb) {
    rounded(fb, CPU_TEST_BACK, 4, BUTTON);
    text(fb, &BODY_BOLD, "Back", CPU_TEST_BACK.center().x, CPU_TEST_BACK.top_left.y + 28, CENTER, TEXT);
    if let Some(rate) = app.rate_warning() {
        rate_warning(fb, rate, app.input_kind());
        return;
    }
    if app.inputs()[0].is_none() {
        no_input_notice(fb);
        return;
    }
    summary(app, fb);
    table(app, fb);
    passes_and_log(app, fb);

    let (x, y) = (CPU_TEST_CALLS.top_left.x + 10, CPU_TEST_CALLS.top_left.y);
    rounded(fb, CPU_TEST_CALLS, 4, PANEL);
    text(fb, &SMALL, CALLS_LINES[0], x, y + 12, LEFT, TEXT);
    text(fb, &SMALL, CALLS_LINES[1], x, y + 25, LEFT, TEXT_MUTED);
    switch_control(fb, CPU_TEST_CALLS_SWITCH, app.bufs.cpu_test.test_calls);

    if app.stages_available() {
        rounded(fb, CPU_TEST_VIEW, 4, BUTTON);
        let label = match app.bufs.cpu_test.view {
            View::Totals => VIEW_STAGES,
            View::Stages => VIEW_TOTALS,
        };
        text(fb, &BODY_BOLD, label, CPU_TEST_VIEW.center().x, CPU_TEST_VIEW.top_left.y + 28, CENTER, TEXT);
    }
    rounded(fb, CPU_TEST_START, 4, ACCENT);
    let has_results = app.bufs.cpu_test.has_results();
    text(
        fb,
        &BODY_BOLD,
        if has_results { START_AGAIN } else { START },
        CPU_TEST_START.center().x,
        CPU_TEST_START.top_left.y + 28,
        CENTER,
        TEXT,
    );
}

pub(super) const RUNNING_HELP: &str = "Tap anywhere to stop the test.";
pub(super) const STARTING_TAKE: &str = "Starting a take for the recording pass...";

/// Where the value line goes on the main screen, while the test runs.
pub(super) fn running_line(app: &BatApp, fb: Fb) {
    let test = &app.bufs.cpu_test;
    if test.running && test.phase == Phase::StartingTake {
        text(fb, &SMALL, STARTING_TAKE, 8, VALUE_BASELINE - 12, LEFT, TEXT);
        text(fb, &SMALL, RUNNING_HELP, 8, VALUE_BASELINE, LEFT, TEXT_MUTED);
        return;
    }
    let Some(row) = test.current() else { return };
    let (mode, suffix) = row_name(row);
    let left = app.bufs.cpu_test.seconds_left_in_row(app.samples, app.sample_rate as u64);
    let volume = match row.volume {
        AutoVolume::Off => "off",
        AutoVolume::On => "on",
        AutoVolume::BatsOnly => "Bats only",
    };
    let decoders = if row.decoders { ", decoders" } else { "" };
    let line = format_args!("{mode}{suffix}, Auto volume {volume}{decoders}, {left} s left");
    text(fb, &SMALL, line, 8, VALUE_BASELINE - 12, LEFT, TEXT);
    text(fb, &SMALL, RUNNING_HELP, 8, VALUE_BASELINE, LEFT, TEXT_MUTED);
}

/// Test helpers: each column's right edge, and the widest heading or value it can show. The
/// shares of CPU stay below 100, since the DSP task sleeps after every pass.
#[cfg(test)]
/// For the Totals table, then the Stages table.
pub(crate) fn column_contents() -> std::vec::Vec<std::vec::Vec<(i32, std::vec::Vec<std::string::String>)>> {
    use std::format;
    use std::string::ToString;
    use std::vec;

    let problem = |n| format!("{}", Count { n, full_below: PROBLEM_FULL_BELOW });
    let totals = COLUMNS
        .iter()
        .map(|&(name, x)| {
            let values = match name {
                "dsp" | "max" | "draw" | "irq" | "hook" => vec!["99".to_string()],
                "ring" => vec![format!("{}", Count { n: 9_999, full_below: RING_FULL_BELOW })],
                "skip%" => vec!["100".to_string()],
                // 8 s of 192 kHz stereo is about 3 million frames.
                _ => vec![problem(999), problem(99_999), problem(999_999), problem(9_999_999)],
            };
            (x, std::iter::once(name.to_string()).chain(values).collect())
        })
        .collect();
    let stages = STAGE_COLUMNS.iter().map(|&(name, x)| (x, vec![name.to_string(), "99".to_string()])).collect();
    vec![totals, stages]
}

/// Test helpers: each line that has a limit, with its width and the room for it.
#[cfg(test)]
pub(crate) fn lines_with_widths() -> std::vec::Vec<(std::string::String, i32, i32)> {
    use std::format;
    use std::string::ToString;
    use std::vec::Vec;

    use embedded_graphics::prelude::Point;
    use u8g2_fonts::FontRenderer;
    use u8g2_fonts::types::VerticalPosition;

    let width = |font: &FontRenderer, line: &str| {
        font.get_rendered_dimensions(line, Point::zero(), VerticalPosition::Baseline).map_or(0, |d| d.advance.x)
    };
    let mut lines = Vec::new();
    let mut add = |font: &FontRenderer, line: &str, room: i32| lines.push((line.to_string(), width(font, line), room));
    let full = 308;
    for state in [Recorder::Recording, Recorder::Standby, Recorder::Paused, Recorder::Stopped] {
        add(&SMALL, &format!("{} Ended at 25 of 26.", every_row(state)), full);
    }
    add(&SMALL, &format!("{}{} Ended at 25 of 26.", every_row(Recorder::Recording), mix_note(Some(false))), full);
    add(&SMALL, "Recording, MIX file on. The test runs while recording.", full);
    add(&SMALL, "Recording, MIX file off. The test runs while recording.", full);
    add(&SMALL, &format!("{STATE_CHANGED} Ended at 25 of 26."), full);
    add(&SMALL, "Recording paused. Press REC to test while recording.", full);
    add(&SMALL, "Recorder stopped. Press REC to test while recording.", full);
    for line in CALLS_LINES {
        add(&SMALL, line, CPU_TEST_CALLS_SWITCH.top_left.x - 16 - 6);
    }
    for name in ["HET", "COMB", "1:1"] {
        add(&SMALL, &format!("{name} stereo*"), NAME_ROOM);
        add(&SMALL, &format!("{name} mono*"), NAME_ROOM);
    }
    add(&SMALL, "COMB stereo, Auto volume off, decoders, 8 s left", full);
    // Room for builds up to 999.
    add(&SMALL, &format!("{BUILD_PREFIX}999"), GROUPS_X - 4 - LABEL_X);
    add(&SMALL, RUNNING_HELP, full);
    lines
}
