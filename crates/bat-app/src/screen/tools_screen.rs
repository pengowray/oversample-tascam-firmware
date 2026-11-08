//! The tools screen, and the phone remote's notices on the main screen.

use super::{BODY, BODY_BOLD, CENTER, Fb, LEFT, SMALL, rounded, switch_control, text};
use crate::layout::{BACK, TOOLS_CHIME_TEST, TOOLS_CPU_TEST, TOOLS_GUANO, TOOLS_GUANO_SWITCH, TOOLS_REMOTE, TOOLS_REMOTE_SWITCH};
use crate::palette::*;
use crate::{BatApp, RemoteCommand, RemoteResult};

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
pub(super) const GUANO_NAME: &str = "GUANO metadata";
pub(super) const GUANO_HELP: [&str; 2] =
    ["Adds a timestamp and recorder model to each WAV file.", "Bat analysis apps read GUANO. Most audio apps ignore it."];

pub(super) fn draw(app: &BatApp, fb: Fb) {
    if app.remote_available() {
        remote_panel(app, fb);
    }
    rounded(fb, TOOLS_CHIME_TEST, 4, BUTTON);
    let (x, y) = (TOOLS_CHIME_TEST.top_left.x + 10, TOOLS_CHIME_TEST.top_left.y);
    text(fb, &BODY_BOLD, CHIME_TEST_NAME, x, y + 19, LEFT, TEXT);
    text(fb, &SMALL, CHIME_TEST_HELP, x, y + 37, LEFT, TEXT_MUTED);
    if app.debug_available() {
        rounded(fb, TOOLS_CPU_TEST, 4, BUTTON);
        let (x, y) = (TOOLS_CPU_TEST.top_left.x + 10, TOOLS_CPU_TEST.top_left.y);
        text(fb, &BODY_BOLD, CPU_TEST_NAME, x, y + 19, LEFT, TEXT);
        for (line, y) in CPU_TEST_HELP.iter().zip([y + 37, y + 51]) {
            text(fb, &SMALL, *line, x, y, LEFT, TEXT_MUTED);
        }
    }
    if app.guano_available() {
        rounded(fb, TOOLS_GUANO, 6, PANEL);
        let (x, y) = (TOOLS_GUANO.top_left.x + 10, TOOLS_GUANO.top_left.y);
        text(fb, &BODY, GUANO_NAME, x, y + 20, LEFT, TEXT);
        switch_control(fb, TOOLS_GUANO_SWITCH, app.guano());
        for (line, y) in GUANO_HELP.iter().zip([y + 44, y + 58]) {
            text(fb, &SMALL, *line, x, y, LEFT, TEXT_MUTED);
        }
    }
    rounded(fb, BACK, 4, BUTTON);
    text(fb, &BODY_BOLD, "Back", BACK.center().x, BACK.top_left.y + 28, CENTER, TEXT);
}

fn remote_panel(app: &BatApp, fb: Fb) {
    rounded(fb, TOOLS_REMOTE, 6, PANEL);
    let (x, y) = (TOOLS_REMOTE.top_left.x + 10, TOOLS_REMOTE.top_left.y);
    text(fb, &BODY, REMOTE_NAME, x, y + 20, LEFT, TEXT);
    switch_control(fb, TOOLS_REMOTE_SWITCH, app.remote_on());
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
    let inside = TOOLS_REMOTE.size.width as i32 - 20;
    for line in REMOTE_HELP.iter().chain(&CPU_TEST_HELP).chain(&GUANO_HELP).chain([&REMOTE_BEEPS, &CHIME_TEST_HELP]) {
        add(&SMALL, line, inside);
    }
    add(&BODY, REMOTE_NAME, 240);
    add(&BODY, GUANO_NAME, 240);
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
