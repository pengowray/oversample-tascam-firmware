//! The settings screen's Record and Tests pages, and the phone remote's notices on the main
//! screen.

use embedded_graphics::primitives::Rectangle;

use super::{BODY, BODY_BOLD, Fb, LEFT, SMALL, outline_selected, rounded, switch_control, text};
use crate::layout::{RECORD_GUANO, RECORD_MIX_FILE, RECORD_REMOTE, TESTS_CHIME_TEST, TESTS_CPU_TEST, TESTS_DEBUG, panel_switch};
use crate::palette::*;
use crate::{BatApp, RemoteCommand, RemoteResult, Setting};

/// In the title bar of the main screen while the remote's switch is on. Muted while it waits for
/// a tone, and in the accent colour while the chime decoders run.
pub(super) const TITLE_LABEL: &str = "REMOTE";

pub(super) const REMOTE_NAME: &str = "Phone remote (experimental)";
pub(super) const REMOTE_HELP: [&str; 2] =
    ["Phone chimes start or stop recording about 4 s later.", "BAT DETECTOR only. Almost no CPU until the phone plays."];
pub(super) const REMOTE_BEEPS: &str = "Two notes up: started. Two down: stopped. One low: failed.";
pub(super) const CHIME_TEST_NAME: &str = "Chime test";
pub(super) const CHIME_TEST_HELP: &str = "Shows if the X8 reads the phone's chimes, and the level.";
pub(super) const CPU_TEST_NAME: &str = "CPU test";
pub(super) const CPU_TEST_HELP: [&str; 2] =
    ["Runs every listening mode with each Auto volume choice,", "8 s each, up to 3.5 min. Lists CPU load and lost input."];
pub(super) const MIX_FILE_NAME: &str = "MIX file";
pub(super) const MIX_FILE_HELP: &str = "Needed to play a take from BROWSE.";
/// The MIX file's help when MANUAL's MASTER has REC READY off.
pub(crate) const MIX_FILE_OFF_IN_MANUAL: &str = "REC READY is off for MASTER in MANUAL.";
pub(super) const DEBUG_NAME: &str = "Debug info";
pub(super) const DEBUG_HELP: &str = "Shows CPU and input figures over the last call panel.";
pub(super) const GUANO_NAME: &str = "GUANO metadata";
pub(super) const GUANO_HELP: [&str; 2] =
    ["Adds a timestamp and recorder model to each WAV file.", "Bat analysis apps read GUANO. Most audio apps ignore it."];

/// A panel with a name, a switch at the right of the name, and help lines under it. Muted when
/// the switch can't be changed here.
fn switch_panel(fb: Fb, panel: Rectangle, name: &str, help: &[&str], on: bool, muted: bool, selected: bool) {
    rounded(fb, panel, 6, PANEL);
    if selected {
        outline_selected(fb, panel);
    }
    let (x, y) = (panel.top_left.x + 10, panel.top_left.y);
    text(fb, &BODY, name, x, y + 20, LEFT, if muted { TEXT_MUTED } else { TEXT });
    switch_control(fb, panel_switch(panel), on);
    for (line, y) in help.iter().zip([y + 44, y + 58]) {
        text(fb, &SMALL, *line, x, y, LEFT, TEXT_MUTED);
    }
}

/// A button that opens a test, with its help inside.
fn test_button(fb: Fb, button: Rectangle, name: &str, help: &[&str]) {
    rounded(fb, button, 4, BUTTON);
    let (x, y) = (button.top_left.x + 10, button.top_left.y);
    text(fb, &BODY_BOLD, name, x, y + 19, LEFT, TEXT);
    for (line, y) in help.iter().zip([y + 37, y + 51]) {
        text(fb, &SMALL, *line, x, y, LEFT, TEXT_MUTED);
    }
}

/// What a take's files get, and the phone remote. Each panel is shown only on a host that offers
/// it, in a fixed place.
pub(super) fn record(app: &BatApp, fb: Fb) {
    if app.mix_file_available() {
        let selected = app.selected_setting() == Setting::MixFile;
        if app.manual_mix_file() {
            switch_panel(fb, RECORD_MIX_FILE, MIX_FILE_NAME, &[MIX_FILE_HELP], app.mix_file(), false, selected);
        } else {
            // MANUAL decides: the switch shows what a take will get, and taps do nothing.
            switch_panel(fb, RECORD_MIX_FILE, MIX_FILE_NAME, &[MIX_FILE_OFF_IN_MANUAL], false, true, selected);
        }
    }
    if app.guano_available() {
        switch_panel(fb, RECORD_GUANO, GUANO_NAME, &GUANO_HELP, app.guano(), false, false);
    }
    if app.remote_available() {
        remote_panel(app, fb);
    }
}

/// The debug figures' switch and the two tests.
pub(super) fn tests(app: &BatApp, fb: Fb) {
    if app.debug_available() {
        let selected = app.selected_setting() == Setting::DebugInfo;
        switch_panel(fb, TESTS_DEBUG, DEBUG_NAME, &[DEBUG_HELP], app.debug_info(), false, selected);
    }
    test_button(fb, TESTS_CHIME_TEST, CHIME_TEST_NAME, &[CHIME_TEST_HELP]);
    if app.debug_available() {
        test_button(fb, TESTS_CPU_TEST, CPU_TEST_NAME, &CPU_TEST_HELP);
    }
}

fn remote_panel(app: &BatApp, fb: Fb) {
    rounded(fb, RECORD_REMOTE, 6, PANEL);
    let (x, y) = (RECORD_REMOTE.top_left.x + 10, RECORD_REMOTE.top_left.y);
    text(fb, &BODY, REMOTE_NAME, x, y + 20, LEFT, TEXT);
    switch_control(fb, panel_switch(RECORD_REMOTE), app.remote_on());
    for (line, y) in REMOTE_HELP.iter().zip([y + 44, y + 58]) {
        text(fb, &SMALL, *line, x, y, LEFT, TEXT_MUTED);
    }
    text(fb, &SMALL, REMOTE_BEEPS, x, y + 72, LEFT, TEXT_MUTED);
    let remote = &app.remote;
    text(
        fb,
        &SMALL,
        format_args!("Tones heard {}. Chimes read {}, failed {}.", remote.tones, remote.chimes_read, remote.chimes_failed),
        x,
        y + 92,
        LEFT,
        TEXT,
    );
    match remote.last {
        Some(last) => {
            let name = match last.command {
                RemoteCommand::Start => "Start",
                RemoteCommand::Stop => "Stop",
            };
            let seconds = app.samples.saturating_sub(last.read_at) / app.sample_rate as u64;
            let line = format_args!("Last: {name}, {seconds} s ago, carrier {:.0} dBFS", last.level_dbfs);
            text(fb, &SMALL, line, x, y + 106, LEFT, TEXT);
        }
        None => {
            text(fb, &SMALL, "No command read yet", x, y + 106, LEFT, TEXT_MUTED);
        }
    }
}

/// A command's outcome, in the corner of the waterfall.
pub(super) fn result_lines(result: RemoteResult) -> &'static [&'static str] {
    match result {
        RemoteResult::Started => &["Phone: recording started"],
        RemoteResult::AlreadyRecording => &["Phone asked to start.", "Already recording."],
        RemoteResult::Stopped => &["Phone: recording stopped"],
        RemoteResult::AlreadyStopped => &["Phone asked to stop.", "Already stopped."],
        RemoteResult::HoldOn(RemoteCommand::Start) => &["Cannot start: HOLD is on.", "Slide HOLD off."],
        RemoteResult::HoldOn(RemoteCommand::Stop) => &["Cannot stop: HOLD is on.", "Slide HOLD off."],
        RemoteResult::GuideOn => &["Cannot start: recording guide", "is on. Turn it off in MANUAL."],
        RemoteResult::PageOpen => &["Cannot start: MANUAL settings", "page open. Press PLAY, close it."],
        RemoteResult::NoResponse => &["Recorder did not respond to", "the phone's command within 5 s."],
    }
}

/// Test helpers: each line that has a limit, with its width and the room for it.
#[cfg(test)]
pub(crate) fn lines_with_widths() -> std::vec::Vec<(std::string::String, i32, i32)> {
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
    let inside = RECORD_REMOTE.size.width as i32 - 20;
    for line in REMOTE_HELP.iter().chain(&CPU_TEST_HELP).chain(&GUANO_HELP).chain([&REMOTE_BEEPS, &CHIME_TEST_HELP]) {
        add(&SMALL, line, inside);
    }
    for line in [MIX_FILE_HELP, MIX_FILE_OFF_IN_MANUAL, DEBUG_HELP] {
        add(&SMALL, line, inside);
    }
    // The name's line ends at the switch.
    for name in [REMOTE_NAME, GUANO_NAME, MIX_FILE_NAME, DEBUG_NAME] {
        add(&BODY, name, 240);
    }
    add(&SMALL, "Tones heard 100. Chimes read 100, failed 100.", inside);
    add(&SMALL, "Last: Start, 3600 s ago, carrier -100 dBFS", inside);
    for result in [
        RemoteResult::Started,
        RemoteResult::AlreadyRecording,
        RemoteResult::Stopped,
        RemoteResult::AlreadyStopped,
        RemoteResult::HoldOn(RemoteCommand::Start),
        RemoteResult::HoldOn(RemoteCommand::Stop),
        RemoteResult::GuideOn,
        RemoteResult::PageOpen,
        RemoteResult::NoResponse,
    ] {
        for line in result_lines(result) {
            add(&BODY, line, crate::layout::WF_W as i32 - 20);
        }
    }
    lines
}
