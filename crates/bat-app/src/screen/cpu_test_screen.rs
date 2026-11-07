//! The CPU test's results, and what the main screen shows while it runs.

use embedded_graphics::pixelcolor::Rgb565;

use super::{BODY_BOLD, CENTER, Fb, LEFT, RIGHT, SMALL, hline, no_input_notice, rate_warning, rounded, switch_control, text};
use crate::cpu_test::Row;
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

pub(super) const CALLS_LINES: [&str; 2] = ["Synthetic calls make every run sound the same.", "10 a second. Heard in headphones. Not recorded."];
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

/// What the recorder was doing for every row.
fn every_row(state: Recorder) -> &'static str {
    match state {
        Recorder::Recording => "Recorder was recording for every row.",
        Recorder::Standby => "Recorder was in standby for every row.",
        Recorder::Paused => "Recorder was paused for every row.",
        Recorder::Stopped => "Recorder was stopped for every row.",
    }
}

/// The line above the table when the recorder's state changed during the test.
const STATE_CHANGED: &str = "Recorder state changed. * differs from row 1.";

fn group_heading(row: &Row) -> &'static str {
    match (row.decoders, row.volume) {
        (true, AutoVolume::Off) => "Auto volume off, chime decoders running",
        (true, AutoVolume::On) => "Auto volume on, chime decoders running",
        (true, AutoVolume::BatsOnly) => "Auto volume Bats only, chime decoders running",
        (false, AutoVolume::Off) => "Auto volume off",
        (false, AutoVolume::On) => "Auto volume on",
        (false, AutoVolume::BatsOnly) => "Auto volume Bats only",
    }
}

/// A row's name.
fn row_name(row: &Row) -> (&'static str, &'static str) {
    let suffix = match (row.has_stereo, row.one_side) {
        (false, _) => "",
        (true, false) => " stereo",
        (true, true) => " mono",
    };
    (row.mode.label(), suffix)
}

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
            Recorder::Recording => "Recording. The test runs while recording.",
            Recorder::Standby => "Record standby. Press REC to test while recording.",
            Recorder::Paused => "Recording paused. Press REC to test while recording.",
            Recorder::Stopped => "Recorder stopped. Press REC to test while recording.",
        };
        text(fb, &SMALL, line, LABEL_X, y, LEFT, TEXT);
        return;
    };
    let same = rows.iter().filter_map(|row| row.result).all(|result| result.recorder == first.recorder);
    let x = match (same, first.recorder) {
        (true, Some(state)) => text(fb, &SMALL, every_row(state), LABEL_X, y, LEFT, TEXT),
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
    text(fb, &SMALL, "frames", LOST_X - 18, CPU_TEST_GROUPS, LEFT, TEXT_MUTED);
    for (name, x) in COLUMNS {
        text(fb, &SMALL, name, x, CPU_TEST_COLUMNS, RIGHT, TEXT_MUTED);
    }
    hline(fb, LABEL_X, SKIP_X, CPU_TEST_COLUMNS + 2, LINE);
    let rows = app.cpu_test_rows();
    let first_state = rows.iter().find_map(|row| row.result).and_then(|result| result.recorder);
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
            if result.recorder != first_state {
                text(fb, &SMALL, "*", x + 1, y, LEFT, METER_YELLOW);
            }
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
            problem_count(fb, result.skip_percent, SKIP_X, y);
        }
        y += CPU_TEST_ROW_H;
    }
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

    let (x, y) = (CPU_TEST_CALLS.top_left.x + 10, CPU_TEST_CALLS.top_left.y);
    rounded(fb, CPU_TEST_CALLS, 4, PANEL);
    text(fb, &SMALL, CALLS_LINES[0], x, y + 12, LEFT, TEXT);
    text(fb, &SMALL, CALLS_LINES[1], x, y + 25, LEFT, TEXT_MUTED);
    switch_control(fb, CPU_TEST_CALLS_SWITCH, app.cpu_test.test_calls);

    rounded(fb, CPU_TEST_START, 4, ACCENT);
    let has_results = app.cpu_test.has_results();
    text(fb, &BODY_BOLD, if has_results { START_AGAIN } else { START }, CPU_TEST_START.center().x, CPU_TEST_START.top_left.y + 28, CENTER, TEXT);
}

pub(super) const RUNNING_HELP: &str = "Tap anywhere to stop the test.";

/// Where the value line goes on the main screen, while the test runs.
pub(super) fn running_line(app: &BatApp, fb: Fb) {
    let Some(row) = app.cpu_test.current() else { return };
    let (mode, suffix) = row_name(row);
    let left = app.cpu_test.seconds_left_in_row(app.samples, app.sample_rate as u64);
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
pub(crate) fn column_contents() -> std::vec::Vec<(i32, std::vec::Vec<std::string::String>)> {
    use std::string::ToString;
    use std::vec;
    use std::format;

    let problem = |n| format!("{}", Count { n, full_below: PROBLEM_FULL_BELOW });
    COLUMNS
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
        .collect()
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
